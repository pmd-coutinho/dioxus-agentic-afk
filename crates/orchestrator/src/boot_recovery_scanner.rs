//! Boot recovery scanner (ADR-0042 S2).
//!
//! Runs once at server boot, after schema migrations and before HTTP
//! routes bind, and reconciles every `plan_run_phase_outputs` row left in
//! `in_flight` or `interrupted` from a previous orchestrator process that
//! was killed mid-flight (crash, OOM, hard SIGKILL, or the
//! [`crate::shutdown_coordinator`] mark-and-kill path that ADR-0042 S1
//! installs).
//!
//! Per-row transitions (single fixed table):
//!
//! | row state            | assignment status            | action                                                |
//! |----------------------|------------------------------|-------------------------------------------------------|
//! | `in_flight`/`interrupted` row with `assignment_id` | non-terminal except `merge_staged` | block assignment w/ `BlockReason::OrchestratorRestart`, mark row `interrupted`, write `AssignmentBlockedOnRestart` activity |
//! | same                                              | `merge_staged`                     | leave assignment status untouched, mark row `interrupted` |
//! | same                                              | already terminal (`blocked` / `merged`) | mark row `interrupted` (no double-block, no second activity) |
//! | row with `assignment_id IS NULL` (planning row)   | n/a                                | mark row `interrupted` |
//!
//! After per-row processing every touched **Plan Run** is re-evaluated:
//! if every assignment under it is terminal (`merged` / `blocked`), the
//! Plan Run transitions to `finished` (a new fourth terminal alongside
//! `succeeded` / `succeeded_empty` / `failed`; the dashboard surfaces it
//! in S3).
//!
//! The scanner is **idempotent**: running it a second time on the same
//! database is a no-op for assignments that already reached `blocked`
//! during the first pass — the row outcome is already `interrupted` and
//! the assignment-status guard skips re-blocking, so no duplicate
//! `AssignmentBlockedOnRestart` activity entries are written.

use std::collections::BTreeSet;

use agentic_afk_contracts::{BlockReason, IssueAssignmentResponse};
use agentic_afk_persistence::{
    self as persistence, Db, InFlightPhaseRowSummary, PersistenceError,
};

use crate::coordinator::EventPublisher;

/// New Activity kind written for every assignment recovered by the
/// scanner. Matches ADR-0040's pause/resume convention of one Activity
/// per state transition. Surfaced in the Dashboard by S3.
pub const ACTIVITY_KIND_ASSIGNMENT_BLOCKED_ON_RESTART: &str = "assignment_blocked_on_restart";

/// Terminal label written to `plan_runs.state` when the scanner sweeps a
/// previously-`running` Plan Run whose every assignment is now terminal.
/// New value (ADR-0042) — the dashboard renders unknown states with a
/// neutral pill until S3 maps it.
pub const PLAN_RUN_TERMINAL_FINISHED: &str = "finished";

/// Event-publisher shim used at server boot, when the event bus is
/// either not yet wired or scoped per-Project and inappropriate for the
/// cross-Project recovery sweep. Activity rows are still written through
/// the persistence layer directly inside [`run`] so the audit log is
/// complete; SSE deltas for live dashboards land in S3 (ADR-0042 S3).
pub struct NoopEventPublisher;

impl EventPublisher for NoopEventPublisher {
    fn plan_run_started(
        &self,
        _project_id: &str,
        _plan_run: agentic_afk_contracts::PlanRunResponse,
    ) {
    }
    fn plan_run_completed(
        &self,
        _project_id: &str,
        _plan_run: agentic_afk_contracts::PlanRunResponse,
    ) {
    }
    fn plan_run_phase_completed(
        &self,
        _project_id: &str,
        _plan_run_id: &str,
        _phase_output: agentic_afk_contracts::PhaseOutputResponse,
    ) {
    }
    fn assignment_created(
        &self,
        _project_id: &str,
        _assignment: agentic_afk_contracts::IssueAssignmentResponse,
    ) {
    }
    fn assignment_status_changed(
        &self,
        _project_id: &str,
        _assignment: agentic_afk_contracts::IssueAssignmentResponse,
    ) {
    }
    fn record_activity(
        &self,
        _project_id: &str,
        _assignment_id: Option<&str>,
        _kind: &str,
        _detail: Option<&str>,
    ) {
    }
}

/// What the scanner did during one boot. Returned for INFO logging at
/// the call site and for integration-test observation.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct RecoveryReport {
    pub rows_scanned: usize,
    pub assignments_blocked: usize,
    pub plan_runs_finished: usize,
    pub planning_rows_recovered: usize,
}

/// Scan once. Synchronous in effect: returns when every row has been
/// transitioned and every touched Plan Run re-evaluated. `events` is
/// best-effort — recovery succeeds even if SSE/event publishing has not
/// started yet (the rows are durable, so a later dashboard reconnect
/// will see the recovered state on the next snapshot fetch).
pub async fn run(
    db: &Db,
    events: &dyn EventPublisher,
) -> Result<RecoveryReport, PersistenceError> {
    let rows = persistence::list_in_flight_phase_rows(db).await?;
    let mut report = RecoveryReport {
        rows_scanned: rows.len(),
        ..RecoveryReport::default()
    };
    let mut touched_plan_runs: BTreeSet<String> = BTreeSet::new();

    for row in rows {
        touched_plan_runs.insert(row.plan_run_id.clone());
        process_row(db, events, row, &mut report).await?;
    }

    for plan_run_id in touched_plan_runs {
        if maybe_finish_plan_run(db, events, &plan_run_id).await? {
            report.plan_runs_finished += 1;
        }
    }

    if report.rows_scanned > 0 {
        eprintln!(
            "boot recovery: scanned {} non-terminal phase row(s); blocked {} assignment(s); finished {} Plan Run(s); recovered {} planning row(s)",
            report.rows_scanned,
            report.assignments_blocked,
            report.plan_runs_finished,
            report.planning_rows_recovered,
        );
    }
    Ok(report)
}

async fn process_row(
    db: &Db,
    events: &dyn EventPublisher,
    row: InFlightPhaseRowSummary,
    report: &mut RecoveryReport,
) -> Result<(), PersistenceError> {
    eprintln!(
        "boot recovery: recovering phase row {} (phase={}, outcome={}, assignment={:?})",
        row.id, row.phase, row.outcome, row.assignment_id
    );
    match row.assignment_id.as_deref() {
        None => {
            // Planning row: nothing to block, just mark the row
            // interrupted (no-op if already interrupted) so a subsequent
            // scan does not re-process it.
            persistence::mark_phase_row_interrupted(db, &row.id).await?;
            report.planning_rows_recovered += 1;
        }
        Some(assignment_id) => {
            let assignment =
                persistence::get_issue_assignment_public(db, assignment_id).await?;
            match assignment.status.as_str() {
                "merge_staged" => {
                    // ADR-0042: a staged push is itself a recoverable
                    // state via Retry Push / Abandon Staged (ADR-0037).
                    // Leave the assignment untouched and only rewrite
                    // the phase row so the next scan does not re-process
                    // it.
                    persistence::mark_phase_row_interrupted(db, &row.id).await?;
                }
                "merged" | "blocked" => {
                    // Already terminal — likely a prior recovery pass
                    // already blocked this assignment. Just sweep the
                    // row outcome forward; do *not* write a duplicate
                    // Activity entry. Idempotent.
                    persistence::mark_phase_row_interrupted(db, &row.id).await?;
                }
                _ => {
                    let blocked = persistence::record_blocked_with_kind(
                        db,
                        assignment_id,
                        BlockReason::OrchestratorRestart,
                        Some("orchestrator restarted while this phase was running"),
                    )
                    .await?;
                    persistence::mark_phase_row_interrupted(db, &row.id).await?;
                    write_block_activity(db, events, &blocked, &row.phase).await;
                    let project_id = blocked.project_id.0.clone();
                    events.assignment_status_changed(&project_id, blocked);
                    report.assignments_blocked += 1;
                }
            }
        }
    }
    Ok(())
}

async fn write_block_activity(
    db: &Db,
    events: &dyn EventPublisher,
    assignment: &IssueAssignmentResponse,
    phase: &str,
) {
    let detail = format!(
        "blocked on orchestrator restart during {phase} phase (source={source_id})",
        source_id = assignment.source_id
    );
    // Best-effort: record_project_activity write directly so the row
    // lands even if the event publisher is not yet wired (boot runs
    // before HTTP serve).
    let _ = persistence::record_project_activity(
        db,
        &assignment.project_id.0,
        Some(&assignment.id),
        ACTIVITY_KIND_ASSIGNMENT_BLOCKED_ON_RESTART,
        Some(&detail),
    )
    .await;
    // Publish the same kind through the trait so a long-lived dashboard
    // tab open across the recovery boundary still receives an SSE delta
    // (S3 wires the renderer for this kind).
    events.record_activity(
        &assignment.project_id.0,
        Some(&assignment.id),
        ACTIVITY_KIND_ASSIGNMENT_BLOCKED_ON_RESTART,
        Some(&detail),
    );
}

async fn maybe_finish_plan_run(
    db: &Db,
    events: &dyn EventPublisher,
    plan_run_id: &str,
) -> Result<bool, PersistenceError> {
    let plan_run = match persistence::get_plan_run(db, plan_run_id).await {
        Ok(run) => run,
        Err(PersistenceError::NotFound(_)) => return Ok(false),
        Err(error) => return Err(error),
    };
    if plan_run.state != "running" {
        return Ok(false);
    }
    let all_terminal = plan_run
        .assignments
        .iter()
        .all(|a| matches!(a.status.as_str(), "merged" | "blocked"));
    if !all_terminal {
        return Ok(false);
    }
    let finished = persistence::finish_plan_run(db, plan_run_id, PLAN_RUN_TERMINAL_FINISHED).await?;
    events.plan_run_completed(&plan_run.project_id.0, finished);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_afk_contracts::{CreateProjectRequest, IssueSource, SourceIssueSnapshot};
    use agentic_afk_persistence::{
        connect_in_memory, create_plan_run, create_plan_run_assignment, create_project,
        insert_in_flight_phase_output, list_project_activity, migrate,
        set_assignment_status,
    };
    use std::sync::Mutex;

    struct CapturingEvents {
        assignment_changed: Mutex<Vec<IssueAssignmentResponse>>,
        plan_run_completed: Mutex<Vec<String>>,
        activities: Mutex<Vec<(String, Option<String>, String)>>,
    }

    impl CapturingEvents {
        fn new() -> Self {
            Self {
                assignment_changed: Mutex::new(Vec::new()),
                plan_run_completed: Mutex::new(Vec::new()),
                activities: Mutex::new(Vec::new()),
            }
        }
    }

    impl EventPublisher for CapturingEvents {
        fn plan_run_started(
            &self,
            _project_id: &str,
            _plan_run: agentic_afk_contracts::PlanRunResponse,
        ) {
        }
        fn plan_run_completed(
            &self,
            _project_id: &str,
            plan_run: agentic_afk_contracts::PlanRunResponse,
        ) {
            self.plan_run_completed.lock().unwrap().push(plan_run.id);
        }
        fn plan_run_phase_completed(
            &self,
            _project_id: &str,
            _plan_run_id: &str,
            _phase_output: agentic_afk_contracts::PhaseOutputResponse,
        ) {
        }
        fn assignment_created(
            &self,
            _project_id: &str,
            _assignment: IssueAssignmentResponse,
        ) {
        }
        fn assignment_status_changed(
            &self,
            _project_id: &str,
            assignment: IssueAssignmentResponse,
        ) {
            self.assignment_changed.lock().unwrap().push(assignment);
        }
        fn record_activity(
            &self,
            project_id: &str,
            assignment_id: Option<&str>,
            kind: &str,
            _detail: Option<&str>,
        ) {
            self.activities.lock().unwrap().push((
                project_id.to_string(),
                assignment_id.map(str::to_string),
                kind.to_string(),
            ));
        }
    }

    async fn setup() -> (Db, String, String) {
        let db = connect_in_memory().await.unwrap();
        migrate(&db).await.unwrap();
        let project = create_project(
            &db,
            &CreateProjectRequest {
                path: "/tmp".to_string(),
            },
        )
        .await
        .unwrap();
        let plan_run = create_plan_run(&db, &project.id.0, "main", "deadbeef")
            .await
            .unwrap();
        (db, project.id.0, plan_run.id)
    }

    fn fake_source() -> IssueSource {
        IssueSource {
            kind: "local_markdown".into(),
            locator: "docs/".into(),
        }
    }

    fn fake_issue(source_id: &str) -> SourceIssueSnapshot {
        SourceIssueSnapshot {
            source_id: source_id.to_string(),
            title: source_id.to_string(),
            readiness: "ready".into(),
            lifecycle_status: "ready".into(),
            parent_issue: None,
            issue_dependencies: vec![],
            source_order: 1,
            raw_text: "raw".into(),
        }
    }

    #[tokio::test]
    async fn in_flight_implementation_row_blocks_assignment_and_finishes_plan_run() {
        let (db, project_id, plan_run_id) = setup().await;
        let source = fake_source();
        let issue = fake_issue("issue-1");
        let assignment = create_plan_run_assignment(
            &db,
            &plan_run_id,
            &project_id,
            &source,
            &issue,
            "branch-1",
            "selected",
        )
        .await
        .unwrap();
        // Simulate orchestrator killed mid-implementation: assignment
        // sits at `implementing`, an in-flight implementation row
        // points at it.
        set_assignment_status(&db, &assignment.id, "implementing", None)
            .await
            .unwrap();
        let _row = insert_in_flight_phase_output(
            &db,
            &plan_run_id,
            Some(&assignment.id),
            "implementation",
        )
        .await
        .unwrap();

        let events = CapturingEvents::new();
        let report = run(&db, &events).await.unwrap();

        assert_eq!(report.rows_scanned, 1);
        assert_eq!(report.assignments_blocked, 1);
        assert_eq!(report.plan_runs_finished, 1);

        let assignment = persistence::get_issue_assignment_public(&db, &assignment.id)
            .await
            .unwrap();
        assert_eq!(assignment.status, "blocked");
        let kind = assignment
            .block_reason
            .as_ref()
            .map(|r| r.kind);
        assert_eq!(kind, Some(BlockReason::OrchestratorRestart));

        let activities = list_project_activity(&db, &project_id, 10).await.unwrap();
        assert!(
            activities
                .iter()
                .any(|a| a.kind == ACTIVITY_KIND_ASSIGNMENT_BLOCKED_ON_RESTART),
            "one AssignmentBlockedOnRestart activity per recovered assignment"
        );

        let plan_run = persistence::get_plan_run(&db, &plan_run_id).await.unwrap();
        assert_eq!(plan_run.state, PLAN_RUN_TERMINAL_FINISHED);
    }

    #[tokio::test]
    async fn in_flight_planning_row_finishes_plan_run_without_blocking_anything() {
        let (db, project_id, plan_run_id) = setup().await;
        let _ = insert_in_flight_phase_output(&db, &plan_run_id, None, "planning")
            .await
            .unwrap();

        let events = CapturingEvents::new();
        let report = run(&db, &events).await.unwrap();
        assert_eq!(report.assignments_blocked, 0);
        assert_eq!(report.planning_rows_recovered, 1);
        assert_eq!(report.plan_runs_finished, 1);

        let plan_run = persistence::get_plan_run(&db, &plan_run_id).await.unwrap();
        assert_eq!(plan_run.state, PLAN_RUN_TERMINAL_FINISHED);
        assert!(plan_run.assignments.is_empty());
        let _ = project_id; // capture for unused-warning silence
    }

    #[tokio::test]
    async fn merge_staged_assignment_is_preserved() {
        let (db, project_id, plan_run_id) = setup().await;
        let source = fake_source();
        let issue = fake_issue("issue-2");
        let assignment = create_plan_run_assignment(
            &db,
            &plan_run_id,
            &project_id,
            &source,
            &issue,
            "branch-2",
            "selected",
        )
        .await
        .unwrap();
        // ADR-0037: a staged push is itself recoverable. Recovery must
        // leave the assignment status alone.
        set_assignment_status(&db, &assignment.id, "merge_staged", None)
            .await
            .unwrap();
        let _row = insert_in_flight_phase_output(
            &db,
            &plan_run_id,
            Some(&assignment.id),
            "merge",
        )
        .await
        .unwrap();

        let events = CapturingEvents::new();
        let report = run(&db, &events).await.unwrap();
        assert_eq!(report.assignments_blocked, 0);
        // Plan Run not finished: merge_staged is non-terminal.
        assert_eq!(report.plan_runs_finished, 0);

        let assignment = persistence::get_issue_assignment_public(&db, &assignment.id)
            .await
            .unwrap();
        assert_eq!(assignment.status, "merge_staged");
    }

    #[tokio::test]
    async fn recovery_is_idempotent_and_does_not_double_block_or_double_activity() {
        let (db, project_id, plan_run_id) = setup().await;
        let source = fake_source();
        let issue = fake_issue("issue-3");
        let assignment = create_plan_run_assignment(
            &db,
            &plan_run_id,
            &project_id,
            &source,
            &issue,
            "branch-3",
            "selected",
        )
        .await
        .unwrap();
        set_assignment_status(&db, &assignment.id, "implementing", None)
            .await
            .unwrap();
        let _row = insert_in_flight_phase_output(
            &db,
            &plan_run_id,
            Some(&assignment.id),
            "implementation",
        )
        .await
        .unwrap();

        let events = CapturingEvents::new();
        let first = run(&db, &events).await.unwrap();
        let second = run(&db, &events).await.unwrap();

        assert_eq!(first.assignments_blocked, 1);
        assert_eq!(second.assignments_blocked, 0, "no double-block");
        assert_eq!(second.plan_runs_finished, 0, "already finished");

        let activities = list_project_activity(&db, &project_id, 10).await.unwrap();
        let count = activities
            .iter()
            .filter(|a| a.kind == ACTIVITY_KIND_ASSIGNMENT_BLOCKED_ON_RESTART)
            .count();
        assert_eq!(count, 1, "exactly one Activity entry per recovered assignment");
    }
}
