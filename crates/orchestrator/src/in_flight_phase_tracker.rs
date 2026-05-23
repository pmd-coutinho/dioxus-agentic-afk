//! In-flight phase tracker (ADR-0042 S1).
//!
//! Wraps every Codex phase launch in a single deep [`run`] entrypoint that
//! persists a `plan_run_phase_outputs` row with `outcome = 'in_flight'`
//! *before* the **Codex Sandbox** spawns, stamps the captured child PID +
//! spawn timestamp into the same row immediately after spawn (so the
//! [`crate::shutdown_coordinator::ShutdownCoordinator`] can SIGTERM the
//! captured PID), and replaces the row's `outcome` + `body_json` with the
//! terminal result on completion — producing **exactly one row per phase
//! invocation** rather than the prior pattern of a single completion-only
//! row that vanished if the orchestrator was killed mid-flight.
//!
//! The tracker hides the row ordering, PID capture, and outcome-vs-body
//! pairing from its callers; the four coordinator-side call sites
//! (planning, implementation, review, merge) hand it a spawn closure and
//! a typed terminal body and get back the spawn closure's value.

use std::sync::Arc;

use agentic_afk_contracts::PhaseOutputBody;
use agentic_afk_persistence::{
    Db, PersistenceError, complete_in_flight_phase_output, insert_in_flight_phase_output,
    record_in_flight_phase_process,
};
use tokio::sync::Mutex;

/// Identifies the phase a tracked Codex Sandbox launch belongs to.
/// `phase` mirrors the `plan_run_phase_outputs.phase` column and must be
/// one of `planning`, `implementation`, `review`, `merge` — the four
/// Codex phase call sites listed in ADR-0042 S1.
#[derive(Clone, Debug)]
pub struct PhaseLocator {
    pub plan_run_id: String,
    pub assignment_id: Option<String>,
    pub phase: &'static str,
}

/// Terminal result reported by a tracked spawn closure. The closure
/// converts its parsed phase output into a typed [`PhaseOutputBody`] and
/// a wire-encoded `outcome` string (`succeeded` / `ready_for_review` /
/// `merged` / etc.); the tracker writes both back to the in-flight row.
pub struct PhaseRunResult<T> {
    pub value: T,
    pub outcome: String,
    pub body: PhaseOutputBody,
}

/// Handed to the spawn closure so it can stamp the captured PID + spawn
/// timestamp onto the in-flight row *immediately after* the Codex child
/// is spawned, **before** any blocking wait. The recorder is
/// idempotent — the closure may call it once, never (PID capture
/// failed), or repeatedly without corrupting the audit log.
#[derive(Clone)]
pub struct PhaseProcessRecorder {
    db: Db,
    row_id: String,
    recorded: Arc<Mutex<bool>>,
}

impl PhaseProcessRecorder {
    pub async fn record(
        &self,
        process_id: u32,
        process_started_at: &str,
    ) -> Result<(), PersistenceError> {
        let mut flag = self.recorded.lock().await;
        if *flag {
            return Ok(());
        }
        record_in_flight_phase_process(&self.db, &self.row_id, process_id, process_started_at)
            .await?;
        *flag = true;
        Ok(())
    }

    /// Row id of the in-flight `plan_run_phase_outputs` entry. Exposed so
    /// integration tests can observe the pre-spawn row by id without
    /// re-querying the entire in-flight set.
    pub fn row_id(&self) -> &str {
        &self.row_id
    }
}

/// Errors returned by [`run`]. `Phase` wraps the spawn closure's error
/// unchanged so callers can keep their existing error taxonomy; the
/// tracker still writes a `failed` row to the audit log before bubbling
/// the error up so the failure is durably recorded.
#[derive(Debug, thiserror::Error)]
pub enum TrackerError<E> {
    #[error("phase failed: {0}")]
    Phase(E),
    #[error("persistence error: {0}")]
    Persistence(#[from] PersistenceError),
}

/// Run a Codex phase under the tracker. Order of operations (ADR-0042 S1):
///   1. INSERT in_flight row before `spawn_and_wait` runs.
///   2. Invoke `spawn_and_wait` with a [`PhaseProcessRecorder`] so the
///      closure can stamp the captured PID + start timestamp onto the row
///      before it blocks on the Codex child's exit.
///   3. On `Ok(PhaseRunResult { outcome, body, value })`: UPDATE the row
///      to the reported outcome + body, return `value`.
///   4. On `Err(e)`: UPDATE the row to `failed` with
///      [`PhaseOutputBody::Failed`] carrying `e`'s display, then bubble
///      the original error wrapped in [`TrackerError::Phase`].
pub async fn run<F, Fut, T, E>(
    db: &Db,
    locator: PhaseLocator,
    spawn_and_wait: F,
) -> Result<T, TrackerError<E>>
where
    F: FnOnce(PhaseProcessRecorder) -> Fut,
    Fut: std::future::Future<Output = Result<PhaseRunResult<T>, E>>,
    E: std::fmt::Display,
{
    let row_id = insert_in_flight_phase_output(
        db,
        &locator.plan_run_id,
        locator.assignment_id.as_deref(),
        locator.phase,
    )
    .await?;
    let recorder = PhaseProcessRecorder {
        db: db.clone(),
        row_id: row_id.clone(),
        recorded: Arc::new(Mutex::new(false)),
    };
    match spawn_and_wait(recorder).await {
        Ok(PhaseRunResult {
            value,
            outcome,
            body,
        }) => {
            complete_in_flight_phase_output(db, &row_id, &outcome, &body).await?;
            Ok(value)
        }
        Err(err) => {
            let body = PhaseOutputBody::Failed {
                error: err.to_string(),
                problem_type: None,
            };
            complete_in_flight_phase_output(db, &row_id, "failed", &body).await?;
            Err(TrackerError::Phase(err))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_afk_contracts::{CreateProjectRequest, PlanningSelection};
    use agentic_afk_persistence::{
        InFlightPhaseRowSummary, connect_in_memory, create_plan_run, create_project,
        list_in_flight_phase_rows, migrate,
    };

    async fn setup() -> (Db, String) {
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
        (db, plan_run.id)
    }

    fn planning_body() -> PhaseOutputBody {
        PhaseOutputBody::Planning {
            selections: vec![PlanningSelection {
                source_issue_id: "1".into(),
                title: "t".into(),
                branch: "b".into(),
                selection_summary: String::new(),
            }],
            summary: "ok".into(),
            rejected_candidates: vec![],
        }
    }

    #[tokio::test]
    async fn pre_spawn_row_exists_before_closure_returns() {
        let (db, plan_run_id) = setup().await;
        let observed: Mutex<Vec<InFlightPhaseRowSummary>> = Mutex::new(vec![]);

        run(
            &db,
            PhaseLocator {
                plan_run_id: plan_run_id.clone(),
                assignment_id: None,
                phase: "planning",
            },
            |recorder| {
                let db = db.clone();
                let observed = &observed;
                async move {
                    // Observe the in-flight set from inside the spawn
                    // closure — this is the AC: "the row exists in the
                    // database immediately after spawning a Codex phase
                    // (verified by a test that observes the row before
                    // the spawn closure returns)".
                    let snapshot = list_in_flight_phase_rows(&db).await.unwrap();
                    *observed.lock().await = snapshot;
                    recorder.record(4242, "unix:1").await.unwrap();
                    Ok::<_, String>(PhaseRunResult {
                        value: (),
                        outcome: "succeeded".into(),
                        body: planning_body(),
                    })
                }
            },
        )
        .await
        .unwrap();

        let observed = observed.into_inner();
        assert_eq!(observed.len(), 1, "pre-spawn row visible inside closure");
        assert_eq!(observed[0].outcome, "in_flight");
        assert_eq!(observed[0].phase, "planning");

        // After completion the row is terminal — exactly one row per
        // phase invocation, no duplicates.
        let remaining = list_in_flight_phase_rows(&db).await.unwrap();
        assert!(remaining.is_empty());
    }

    #[tokio::test]
    async fn recorder_stamps_pid_on_in_flight_row() {
        let (db, plan_run_id) = setup().await;
        run(
            &db,
            PhaseLocator {
                plan_run_id: plan_run_id.clone(),
                assignment_id: None,
                phase: "planning",
            },
            |recorder| {
                let db = db.clone();
                async move {
                    recorder.record(9999, "unix:42").await.unwrap();
                    // Mid-flight: PID captured, row still in_flight.
                    let mid = list_in_flight_phase_rows(&db).await.unwrap();
                    assert_eq!(mid[0].process_id, Some(9999));
                    Ok::<_, String>(PhaseRunResult {
                        value: (),
                        outcome: "succeeded".into(),
                        body: planning_body(),
                    })
                }
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn closure_error_writes_failed_row_and_bubbles_error() {
        let (db, plan_run_id) = setup().await;
        let result: Result<(), _> = run(
            &db,
            PhaseLocator {
                plan_run_id: plan_run_id.clone(),
                assignment_id: None,
                phase: "implementation",
            },
            |_recorder| async { Err::<PhaseRunResult<()>, _>("codex spawn failed") },
        )
        .await;
        assert!(matches!(result, Err(TrackerError::Phase(_))));

        // Row landed as `failed`, not stuck at `in_flight`.
        let remaining = list_in_flight_phase_rows(&db).await.unwrap();
        assert!(remaining.is_empty(), "failed row no longer in_flight");
    }

    #[tokio::test]
    async fn recorder_record_is_idempotent() {
        let (db, plan_run_id) = setup().await;
        run(
            &db,
            PhaseLocator {
                plan_run_id,
                assignment_id: None,
                phase: "planning",
            },
            |recorder| {
                let db = db.clone();
                async move {
                    recorder.record(1, "unix:1").await.unwrap();
                    // Second call must not corrupt the row.
                    recorder.record(2, "unix:2").await.unwrap();
                    let mid = list_in_flight_phase_rows(&db).await.unwrap();
                    assert_eq!(mid[0].process_id, Some(1));
                    Ok::<_, String>(PhaseRunResult {
                        value: (),
                        outcome: "succeeded".into(),
                        body: planning_body(),
                    })
                }
            },
        )
        .await
        .unwrap();
    }
}
