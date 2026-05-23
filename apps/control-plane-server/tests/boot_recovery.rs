//! End-to-end recovery test for ADR-0042 S2.
//!
//! Simulates an orchestrator that was killed mid-implementation: an Issue
//! Assignment sits at `implementing`, an in-flight implementation phase
//! row points at it, and the orchestrator process has gone away. The
//! BootRecoveryScanner running once at startup must:
//!   1. Block the assignment with BlockReason::OrchestratorRestart.
//!   2. Write exactly one AssignmentBlockedOnRestart Activity entry.
//!   3. Transition the owning Plan Run to `finished` because every
//!      assignment under it is now terminal.

use agentic_afk_contracts::{BlockReason, CreateProjectRequest, IssueSource, SourceIssueSnapshot};
use agentic_afk_orchestrator::boot_recovery_scanner::{
    self, ACTIVITY_KIND_ASSIGNMENT_BLOCKED_ON_RESTART, NoopEventPublisher,
    PLAN_RUN_TERMINAL_FINISHED,
};
use agentic_afk_persistence::{
    self as persistence, connect_in_memory, create_plan_run, create_plan_run_assignment,
    create_project, insert_in_flight_phase_output, list_project_activity, migrate,
    set_assignment_status,
};

#[tokio::test]
async fn killed_mid_implementation_recovers_to_blocked_finished_with_activity() {
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
    let plan_run = create_plan_run(&db, &project.id.0, "main", "baseline-sha")
        .await
        .unwrap();

    let assignment = create_plan_run_assignment(
        &db,
        &plan_run.id,
        &project.id.0,
        &IssueSource {
            kind: "local_markdown".into(),
            locator: "docs/".into(),
        },
        &SourceIssueSnapshot {
            source_id: "issue-99".into(),
            title: "Killed during implementation".into(),
            readiness: "ready".into(),
            lifecycle_status: "ready".into(),
            parent_issue: None,
            issue_dependencies: vec![],
            source_order: 1,
            raw_text: "raw".into(),
        },
        "agent/issue-99",
        "selected by planner",
    )
    .await
    .unwrap();
    set_assignment_status(&db, &assignment.id, "implementing", None)
        .await
        .unwrap();
    // Pre-spawn `in_flight` row, simulating the moment between
    // InFlightPhaseTracker::start() and the killed Codex child's exit.
    let _ = insert_in_flight_phase_output(
        &db,
        &plan_run.id,
        Some(&assignment.id),
        "implementation",
    )
    .await
    .unwrap();

    // Run the scanner — emulating what serve() does between migrate()
    // and TcpListener::bind() on the next boot.
    let report = boot_recovery_scanner::run(&db, &NoopEventPublisher)
        .await
        .unwrap();
    assert_eq!(report.rows_scanned, 1);
    assert_eq!(report.assignments_blocked, 1);
    assert_eq!(report.plan_runs_finished, 1);

    let recovered = persistence::get_issue_assignment_public(&db, &assignment.id)
        .await
        .unwrap();
    assert_eq!(recovered.status, "blocked");
    assert_eq!(
        recovered.block_reason.as_ref().map(|r| r.kind),
        Some(BlockReason::OrchestratorRestart),
    );

    let activities = list_project_activity(&db, &project.id.0, 10).await.unwrap();
    let matching: Vec<_> = activities
        .iter()
        .filter(|a| a.kind == ACTIVITY_KIND_ASSIGNMENT_BLOCKED_ON_RESTART)
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "exactly one AssignmentBlockedOnRestart Activity entry per recovered assignment"
    );
    assert_eq!(matching[0].assignment_id.as_deref(), Some(assignment.id.as_str()));

    let finished_run = persistence::get_plan_run(&db, &plan_run.id).await.unwrap();
    assert_eq!(finished_run.state, PLAN_RUN_TERMINAL_FINISHED);

    // Idempotence: a second scan in the same process is a no-op.
    let second = boot_recovery_scanner::run(&db, &NoopEventPublisher)
        .await
        .unwrap();
    assert_eq!(second.assignments_blocked, 0);
    assert_eq!(second.plan_runs_finished, 0);
    let activities_after = list_project_activity(&db, &project.id.0, 10).await.unwrap();
    let matching_after = activities_after
        .iter()
        .filter(|a| a.kind == ACTIVITY_KIND_ASSIGNMENT_BLOCKED_ON_RESTART)
        .count();
    assert_eq!(matching_after, 1, "no duplicate Activity on re-scan");
}
