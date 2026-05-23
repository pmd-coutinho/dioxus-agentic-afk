//! Issue #55 / ADR-0038: Source-Issue-keyed re-enable with Lifecycle
//! `Ready` write-back.
//!
//! The use case clears local blocked state on the latest blocked
//! **Issue Assignment** for a given **Source Issue** (if one still
//! exists), and writes Lifecycle `Ready` back to the **Issue Source**.
//! Per ADR-0035 the write-back is best-effort: a failed write proceeds
//! locally and surfaces an **Activity** entry; the operator is never
//! blocked from clearing local state during an upstream outage.
//!
//! The local **Planning Snapshot** mirror is updated to `ready` so the
//! next **Plan Run** Planning Snapshot buckets the Source Issue as
//! `eligible` instead of `active` without waiting for a fresh
//! **Issue Source** sync.

use std::sync::Arc;

use agentic_afk_contracts::ProjectResponse;
use agentic_afk_persistence::{self as persistence, Db};

use crate::coordinator::{CoordinatorError, EventPublisher};
use crate::plan_run::LifecycleStatus;
use crate::plan_run::IssueLifecycleWriter;

/// Typed result of a [`re_enable_source_issue`] call. The HTTP handler
/// maps this directly into the wire response so the Dashboard can show
/// both halves (local clear vs. upstream write-back) and surface a
/// partial-success warning when the write-back arm failed.
#[derive(Clone, Debug)]
pub struct ReEnableOutcome {
    /// `true` if a blocked Issue Assignment for the Source Issue still
    /// existed locally and was cleared. `false` is a no-op (Plan Run
    /// cleanup already removed the row); the write-back arm still runs.
    pub local_cleared: bool,
    /// `Ok(())` if the Lifecycle `Ready` write reached the upstream
    /// Issue Source; `Err(error)` if the write-back failed. The local
    /// clear is not rolled back on `Err` per ADR-0035 — the operator
    /// keeps the local view and an Activity entry records the failure.
    pub writeback: Result<(), WritebackError>,
}

/// Wrapper around the upstream Lifecycle write-back failure surface.
/// Stored as a `String` so the Dashboard can render the operator-facing
/// detail without coupling to the orchestrator's internal error enum.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WritebackError(pub String);

impl std::fmt::Display for WritebackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WritebackError {}

/// Re-enable a blocked **Source Issue** for a later **Plan Run**.
///
/// - Looks up the latest blocked Issue Assignment for `(project, source_id)`
///   and clears its blocked state if found.
/// - Updates the local Planning Snapshot mirror for the Source Issue to
///   Lifecycle `Ready` so the next Plan Run does not still see it as
///   `Blocked` (which would bucket it as `active`).
/// - Writes Lifecycle `Ready` upstream via the injected
///   [`IssueLifecycleWriter`]. The write-back is best-effort per ADR-0035:
///   on failure, an Activity entry is recorded and the local state stays
///   cleared.
pub async fn re_enable_source_issue(
    db: &Db,
    events: &Arc<dyn EventPublisher>,
    lifecycle: &Arc<dyn IssueLifecycleWriter>,
    project: &ProjectResponse,
    source_id: &str,
) -> Result<ReEnableOutcome, CoordinatorError> {
    let project_id = project.id.0.as_str();

    let latest = persistence::latest_blocked_assignment_for_source(db, project_id, source_id)
        .await
        .map_err(CoordinatorError::from_persistence)?;

    let mut local_cleared = false;
    let mut cleared_assignment = None;
    if let Some(assignment) = latest {
        let updated =
            persistence::re_enable_blocked_assignment(db, &assignment.id)
                .await
                .map_err(CoordinatorError::from_persistence)?;
        local_cleared = true;
        cleared_assignment = Some(updated);
    }

    // Mirror the new Ready state into the local Planning Snapshot row so
    // the next Plan Run buckets the Source Issue as `eligible` rather
    // than re-using the stale `blocked` lifecycle. Missing snapshot rows
    // are treated as a no-op — sync-driven snapshots eventually
    // reconcile, and an empty snapshot has nothing to mirror against.
    let _ = persistence::set_planning_snapshot_lifecycle(
        db,
        project_id,
        source_id,
        LifecycleStatus::Ready.as_str(),
    )
    .await;

    let writeback = match lifecycle.write(source_id, LifecycleStatus::Ready) {
        Ok(()) => Ok(()),
        Err(error) => {
            let message = error.to_string();
            events.record_activity(
                project_id,
                cleared_assignment.as_ref().map(|a| a.id.as_str()),
                "lifecycle_writeback_failed",
                Some(&format!(
                    "ready Lifecycle Status write-back after re-enable failed: {message}"
                )),
            );
            Err(WritebackError(message))
        }
    };

    if let Some(assignment) = cleared_assignment {
        events.assignment_status_changed(project_id, assignment);
    }

    Ok(ReEnableOutcome {
        local_cleared,
        writeback,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan_run::FakeLifecycleWriter;
    use agentic_afk_contracts::{
        CreateProjectRequest, EnableIssueSourceRequest, IssueSource, SourceIssueSnapshot,
    };
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingPublisher {
        activities: Mutex<Vec<(Option<String>, String, Option<String>)>>,
        status_changes: Mutex<Vec<String>>,
    }

    impl EventPublisher for RecordingPublisher {
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
            assignment: agentic_afk_contracts::IssueAssignmentResponse,
        ) {
            self.status_changes.lock().unwrap().push(assignment.id);
        }
        fn record_activity(
            &self,
            _project_id: &str,
            assignment_id: Option<&str>,
            kind: &str,
            detail: Option<&str>,
        ) {
            self.activities.lock().unwrap().push((
                assignment_id.map(str::to_string),
                kind.to_string(),
                detail.map(str::to_string),
            ));
        }
    }

    async fn setup() -> (Db, ProjectResponse, IssueSource) {
        let db = persistence::connect_in_memory().await.unwrap();
        persistence::migrate(&db).await.unwrap();
        let dir = std::env::temp_dir().join(format!(
            "agentic-afk-reenable-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let project = persistence::create_project(
            &db,
            &CreateProjectRequest {
                path: dir.to_string_lossy().into_owned(),
            },
        )
        .await
        .unwrap();
        let source = IssueSource {
            kind: "github".into(),
            locator: "owner/repo".into(),
        };
        persistence::enable_issue_source(
            &db,
            &project.id.0,
            &EnableIssueSourceRequest {
                kind: source.kind.clone(),
                locator: source.locator.clone(),
            },
        )
        .await
        .unwrap();
        let project = persistence::get_project(&db, &project.id.0).await.unwrap();
        (db, project, source)
    }

    fn issue(source_id: &str, lifecycle: &str) -> SourceIssueSnapshot {
        SourceIssueSnapshot {
            source_id: source_id.into(),
            title: format!("issue {source_id}"),
            readiness: "ready".into(),
            lifecycle_status: lifecycle.into(),
            parent_issue: None,
            issue_dependencies: vec![],
            source_order: 1,
            raw_text: "raw".into(),
        }
    }

    async fn seed_blocked_assignment(
        db: &Db,
        project: &ProjectResponse,
        source: &IssueSource,
        source_id: &str,
    ) -> agentic_afk_contracts::IssueAssignmentResponse {
        let snapshot = issue(source_id, "blocked");
        let assignment =
            persistence::create_issue_assignment(db, &project.id.0, source, &snapshot, "agent/x")
                .await
                .unwrap();
        persistence::record_blocked_with_kind(
            db,
            &assignment.id,
            agentic_afk_contracts::BlockReason::ReviewRetryLimitExhausted,
            Some("Review Loop exhausted"),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn local_row_exists_and_writeback_ok_returns_both_cleared_and_ok() {
        let (db, project, source) = setup().await;
        // Seed a planning snapshot in `blocked` so the local mirror is
        // visible and we can assert it flips to `ready`.
        persistence::replace_planning_snapshot(
            &db,
            &project.id.0,
            &source,
            &[issue("42", "blocked")],
            "unix:1",
        )
        .await
        .unwrap();
        let blocked = seed_blocked_assignment(&db, &project, &source, "42").await;
        assert_eq!(blocked.status, "blocked");

        let writer = Arc::new(FakeLifecycleWriter::new()) as Arc<dyn IssueLifecycleWriter>;
        let events: Arc<dyn EventPublisher> = Arc::new(RecordingPublisher::default());

        let outcome = re_enable_source_issue(&db, &events, &writer, &project, "42")
            .await
            .expect("use case returns Ok");

        assert!(outcome.local_cleared, "local clear succeeded");
        assert!(outcome.writeback.is_ok(), "writeback succeeded");

        // The persisted Issue Assignment row should be cleared.
        let refreshed =
            persistence::get_assignment(&db, &blocked.id).await.unwrap();
        assert_ne!(refreshed.status, "blocked");
        assert!(refreshed.block_reason.is_none());

        // The local Planning Snapshot mirror flips to `ready` so the
        // next Plan Run buckets the Source Issue as `eligible`.
        let raw = persistence::get_planning_snapshot(&db, &project.id.0)
            .await
            .unwrap();
        let row = raw
            .issues
            .iter()
            .find(|i| i.source_id == "42")
            .expect("Source Issue still present in snapshot");
        assert_eq!(row.lifecycle_status, "ready");
    }

    #[tokio::test]
    async fn local_row_exists_and_writeback_fails_clears_locally_and_records_activity() {
        let (db, project, source) = setup().await;
        let blocked = seed_blocked_assignment(&db, &project, &source, "99").await;

        let writer =
            Arc::new(FakeLifecycleWriter::failing("gh down")) as Arc<dyn IssueLifecycleWriter>;
        let publisher = Arc::new(RecordingPublisher::default());
        let events: Arc<dyn EventPublisher> = publisher.clone();

        let outcome = re_enable_source_issue(&db, &events, &writer, &project, "99")
            .await
            .expect("use case still returns Ok even when writeback fails");

        assert!(outcome.local_cleared, "local clear still succeeds");
        let writeback_err = outcome
            .writeback
            .expect_err("writeback surface carries the failure");
        assert!(
            writeback_err.0.contains("gh down"),
            "writeback error surfaces the upstream message: {writeback_err}"
        );

        // The local Issue Assignment row was still cleared (ADR-0035:
        // best-effort write-back does not gate local recovery).
        let refreshed =
            persistence::get_assignment(&db, &blocked.id).await.unwrap();
        assert_ne!(refreshed.status, "blocked");

        // An Activity entry was recorded so the failure is visible in the
        // Dashboard rather than swallowed.
        let activities = publisher.activities.lock().unwrap().clone();
        assert!(
            activities
                .iter()
                .any(|(_, kind, _)| kind == "lifecycle_writeback_failed"),
            "lifecycle_writeback_failed Activity recorded: {activities:?}"
        );
    }

    #[tokio::test]
    async fn no_local_row_and_writeback_ok_still_writes_back_with_local_cleared_false() {
        // No Issue Assignment row exists for this Source Issue
        // (cleanup already deleted it), so `local_cleared` is `false`
        // but the upstream Lifecycle `Ready` write-back still runs so
        // the next Plan Run picks the Source Issue up again.
        let (db, project, _source) = setup().await;

        let writer = Arc::new(FakeLifecycleWriter::new());
        let writer_for_use_case: Arc<dyn IssueLifecycleWriter> = writer.clone();
        let events: Arc<dyn EventPublisher> = Arc::new(RecordingPublisher::default());

        let outcome =
            re_enable_source_issue(&db, &events, &writer_for_use_case, &project, "ghost")
                .await
                .expect("use case Ok for missing local row");

        assert!(
            !outcome.local_cleared,
            "no local row to clear => local_cleared is false"
        );
        assert!(outcome.writeback.is_ok(), "writeback still runs");

        // The Lifecycle writer was called for the missing-row case so
        // the upstream Source Issue is brought back to `Ready`.
        let calls = writer.calls();
        assert!(
            calls
                .iter()
                .any(|(sid, status)| sid == "ghost" && *status == LifecycleStatus::Ready),
            "Lifecycle Ready was written for the Source Issue: {calls:?}"
        );
    }
}
