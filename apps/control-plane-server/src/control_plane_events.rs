//! Paired-lifecycle seam for Control Plane events.
//!
//! This module exists so any started lifecycle event has a guaranteed
//! terminal counterpart (Completed or Failed) — closing the orphan-delta
//! gap where a Started event could leave the live wire dangling if a
//! later step failed without publishing its own terminal event.
//!
//! Today it covers `IssueSourceSyncStarted`/`Completed`/`Failed` via
//! [`during_issue_source_sync`] and re-exposes Activity recording via
//! [`record_activity`]. C2 will fold the existing 1:1 publisher wrappers
//! through this seam.
//!
//! Persist-then-publish semantics for Activity follow ADR-0032.
//!
//! Event order during sync:
//!   Started -> PlanningSnapshotChanged -> Completed
//! `Completed` is intentionally last so "Completed" means all sync-driven
//! deltas have been emitted.
//!
//! Background: previously the `sync_issue_source` handler published
//! `IssueSourceSyncStarted` first and could return early on a persistence
//! error from `replace_planning_snapshot` without publishing any terminal
//! event — the Dashboard saw Started then nothing (an "orphan delta").
//! The closure shape here makes the terminal publish unavoidable: every
//! `Err` from the inner closure flows through the same `Failed` publish
//! site.

use std::future::Future;

use agentic_afk_contracts::IssueSourceSyncResponse;
use agentic_afk_persistence::{Db, PersistenceError, ProjectActivityEntry};

use crate::activity_publisher;
use crate::event_bus::EventBus;
use crate::project_event_publisher;

/// Sync failure cause. Carries enough detail to render either a
/// `IssueSourceSyncFailed { error }` event payload or an HTTP response
/// (problem+json vs. typed persistence response) at the caller.
#[derive(Debug)]
pub enum SyncErr {
    /// Issue Source could not be fetched (e.g. missing dir, gh auth).
    Source(String),
    /// Snapshot persistence failed after the source fetched cleanly.
    Persistence(PersistenceError),
}

impl SyncErr {
    /// Human-readable detail suitable for `IssueSourceSyncFailed { error }`.
    pub fn detail(&self) -> String {
        match self {
            SyncErr::Source(detail) => detail.clone(),
            SyncErr::Persistence(err) => format!("failed to persist Planning Snapshot: {err}"),
        }
    }
}

/// Persist one Project Activity entry and publish the matching live delta.
///
/// Thin re-export over [`activity_publisher::record_project_activity`] so all
/// new code can target one seam.
pub async fn record_activity(
    db: &Db,
    bus: &EventBus,
    project_id: &str,
    assignment_id: Option<&str>,
    kind: &str,
    detail: Option<&str>,
) -> Result<ProjectActivityEntry, PersistenceError> {
    activity_publisher::record_project_activity(db, bus, project_id, assignment_id, kind, detail)
        .await
}

/// Run `f` inside a paired `IssueSourceSyncStarted` / terminal frame.
///
/// Publishes `IssueSourceSyncStarted` immediately, awaits the inner future,
/// then publishes exactly one of `IssueSourceSyncCompleted(resp)` or
/// `IssueSourceSyncFailed { error }` before returning the inner result.
///
/// The closure shape exists specifically to prevent the orphan-delta bug
/// that lived in the previous open-coded handler: any early return from a
/// step inside `f` produces a `SyncErr`, which the seam funnels through the
/// single `Failed` publish site.
pub async fn during_issue_source_sync<F, Fut>(
    bus: &EventBus,
    project_id: &str,
    f: F,
) -> Result<IssueSourceSyncResponse, SyncErr>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<IssueSourceSyncResponse, SyncErr>>,
{
    project_event_publisher::publish_issue_source_sync_started(bus, project_id);

    match f().await {
        Ok(resp) => {
            project_event_publisher::publish_issue_source_sync_completed(
                bus,
                project_id,
                resp.clone(),
            );
            Ok(resp)
        }
        Err(e) => {
            project_event_publisher::publish_issue_source_sync_failed(bus, project_id, &e.detail());
            Err(e)
        }
    }
}
