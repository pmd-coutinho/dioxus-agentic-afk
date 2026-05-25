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

use agentic_afk_contracts::{PhaseOutputBody, PhaseOutputResponse};
use agentic_afk_persistence::{
    Db, PersistenceError, complete_in_flight_phase_output, insert_in_flight_phase_output,
    mark_phase_row_leaked, record_in_flight_phase_process,
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

    pub fn record_blocking(
        &self,
        process_id: u32,
        process_started_at: &str,
    ) -> Result<(), PersistenceError> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.record(process_id, process_started_at))
        })
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

/// Handle to a started but not-yet-completed in-flight phase row, returned
/// by [`start`]. Use when the row's terminal write happens further down a
/// flow than a single closure can express (e.g. the planning row, whose
/// typed body is populated *after* the planner result is validated into
/// **Planned Claims**). Dropping this handle without calling
/// [`TrackedPhase::complete`] or [`TrackedPhase::fail`] leaves the row at
/// `in_flight` so the [`crate::shutdown_coordinator::ShutdownCoordinator`]
/// or the **BootRecoveryScanner** can sweep it — preferable to a silently-
/// failed write, which would lose the audit trail.
pub struct TrackedPhase {
    db: Db,
    row_id: String,
    phase: &'static str,
    assignment_id: Option<String>,
    pub recorder: PhaseProcessRecorder,
    /// Set true by `complete` / `fail` before the value is dropped at the
    /// end of the method body. The Drop guard checks this flag to avoid a
    /// second UPDATE clobbering the terminal outcome the method just
    /// wrote. When the flag is false at Drop time the handle was abandoned
    /// (early `?` return or panic between the in-flight INSERT and the
    /// terminal UPDATE) and the row would otherwise leak at `in_flight`
    /// forever, so the guard schedules a best-effort UPDATE to `failed`.
    finalized: bool,
}

impl TrackedPhase {
    pub fn row_id(&self) -> &str {
        &self.row_id
    }

    /// Replace the in-flight row's `outcome` + `body_json` with the
    /// terminal result. Consumes the handle so a row cannot be completed
    /// twice. Returns a [`PhaseOutputResponse`] mirroring what the
    /// pre-tracker `record_*_phase_output_typed` helpers returned, so
    /// callers can keep feeding the existing event publishers unchanged.
    pub async fn complete(
        mut self,
        outcome: &str,
        body: PhaseOutputBody,
    ) -> Result<PhaseOutputResponse, PersistenceError> {
        complete_in_flight_phase_output(&self.db, &self.row_id, outcome, &body).await?;
        self.finalized = true;
        // PhaseOutputBody is a typed enum derived with serde — `to_value`
        // cannot fail. Unwrapping here is safer than introducing a sqlx
        // dep on the orchestrator crate solely to wrap the error.
        let body_json =
            serde_json::to_value(&body).expect("PhaseOutputBody serialization is infallible");
        Ok(PhaseOutputResponse {
            phase: self.phase.to_string(),
            outcome: outcome.to_string(),
            body_json,
            recorded_at: String::new(),
            assignment_id: self.assignment_id.clone(),
        })
    }

    /// Replace the in-flight row with a `failed` outcome carrying the
    /// supplied error string. Consumes the handle.
    pub async fn fail(
        self,
        error: impl std::fmt::Display,
    ) -> Result<PhaseOutputResponse, PersistenceError> {
        let body = PhaseOutputBody::Failed {
            error: error.to_string(),
            problem_type: None,
        };
        self.complete("failed", body).await
    }
}

/// Self-heal in-flight row leaks. If `complete` / `fail` never ran (early
/// `?` return, panic, future cancellation) the row would stay
/// `outcome='in_flight'` forever — the dashboard surfaces it as a stuck
/// phase and `BootRecoveryScanner` only runs at boot. The guard spawns a
/// best-effort UPDATE on the current tokio runtime stamping the row as
/// `failed` with a stable URN so operators can distinguish leak-recovered
/// rows from rows the phase code finalised itself.
impl Drop for TrackedPhase {
    fn drop(&mut self) {
        if self.finalized {
            return;
        }
        let db = self.db.clone();
        let row_id = self.row_id.clone();
        let phase = self.phase;
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                match mark_phase_row_leaked(&db, &row_id).await {
                    Ok(()) => eprintln!(
                        "[in_flight_phase_tracker] leak guard: swept abandoned in_flight row to failed (phase={phase} row_id={row_id})"
                    ),
                    Err(error) => eprintln!(
                        "[in_flight_phase_tracker] leak guard: failed to mark row failed (phase={phase} row_id={row_id}): {error}"
                    ),
                }
            });
        }
    }
}

/// Insert an in-flight `plan_run_phase_outputs` row and return a handle the
/// caller drives to terminal via [`TrackedPhase::complete`] or
/// [`TrackedPhase::fail`]. Prefer [`run`] when the terminal write can
/// happen inside a single closure; use `start` when the row body depends
/// on validation steps that happen after the Codex Sandbox returns.
pub async fn start(db: &Db, locator: PhaseLocator) -> Result<TrackedPhase, PersistenceError> {
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
    Ok(TrackedPhase {
        db: db.clone(),
        row_id,
        phase: locator.phase,
        assignment_id: locator.assignment_id,
        recorder,
        finalized: false,
    })
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
    let mut handle = start(db, locator).await?;
    let recorder = handle.recorder.clone();
    let row_id = handle.row_id.clone();
    match spawn_and_wait(recorder).await {
        Ok(PhaseRunResult {
            value,
            outcome,
            body,
        }) => {
            // Re-use the lower-level helper directly to avoid double-
            // serialising the body just to return a discarded
            // PhaseOutputResponse.
            complete_in_flight_phase_output(db, &row_id, &outcome, &body).await?;
            // Mark finalized before dropping so the leak-guard Drop impl
            // skips its UPDATE — the row already carries the terminal
            // outcome from the helper above.
            handle.finalized = true;
            drop(handle);
            Ok(value)
        }
        Err(err) => {
            handle.fail(&err).await?;
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
    async fn dropped_handle_self_heals_in_flight_row() {
        // Regression: a `start`-style handle abandoned without `complete` /
        // `fail` (e.g. an early `?` short-circuit between the in-flight
        // INSERT and the terminal UPDATE) used to leave the row stuck at
        // `outcome='in_flight'` forever, blocking the dashboard until the
        // next orchestrator boot ran `BootRecoveryScanner`. The Drop guard
        // now schedules an UPDATE to `failed` so the leak self-heals.
        let (db, plan_run_id) = setup().await;
        {
            let handle = start(
                &db,
                PhaseLocator {
                    plan_run_id: plan_run_id.clone(),
                    assignment_id: None,
                    phase: "planning",
                },
            )
            .await
            .unwrap();
            // Row present + in_flight before drop.
            let mid = list_in_flight_phase_rows(&db).await.unwrap();
            assert_eq!(mid.len(), 1);
            assert_eq!(mid[0].outcome, "in_flight");
            drop(handle);
        }
        // Drop's tokio::spawn ran on the same runtime; give the spawned
        // UPDATE a tick to land. Use a yield + short sleep so the test is
        // not racy on slow CI.
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let remaining = list_in_flight_phase_rows(&db).await.unwrap();
        assert!(
            remaining.is_empty(),
            "leak guard should have swept the abandoned row out of in_flight"
        );
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
