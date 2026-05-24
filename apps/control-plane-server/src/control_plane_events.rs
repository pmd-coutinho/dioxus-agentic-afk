//! Unified seam for Control Plane events to the Dashboard SSE bus.
//!
//! Three concerns:
//! - durable `record_activity` (persist-then-publish per ADR-0032),
//! - paired-lifecycle `during_issue_source_sync` (closure shape prevents
//!   handlers from dropping terminal events),
//! - thin `publish_*` emit fns for individual deltas.
//!
//! Every Control Plane Activity write goes through [`record_activity`] so the
//! audit log (`project_activity` table) and the live SSE wire format
//! (`event_bus`) stay a single source of truth. Callers must never call
//! `persistence::record_project_activity` directly.
//!
//! [`during_issue_source_sync`] exists so any started lifecycle event has a
//! guaranteed terminal counterpart (Completed or Failed) — closing the
//! orphan-delta gap where a Started event could leave the live wire dangling
//! if a later step failed without publishing its own terminal event.
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

use agentic_afk_contracts::{
    AssignmentAttemptResponse, AutoReplanState, IssueAssignmentResponse, IssueSourceCandidate,
    IssueSourceSyncResponse, PauseReason, PhaseOutputResponse, PlanRunResponse,
    PlanningSnapshotResponse, ProjectActivityEntryResponse, ProjectEvent,
    ProjectExecutionConfigResponse, ProjectId, ProjectResponse,
};
use agentic_afk_persistence::{self as persistence, Db, PersistenceError, ProjectActivityEntry};

use crate::event_bus::EventBus;

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

/// Persist one Project Activity entry and publish the matching
/// `ProjectEvent::Activity` delta on the live event bus.
///
/// Returns the persisted entry on success. The activity write is the source
/// of truth; if the DB write fails the bus is not touched.
pub async fn record_activity(
    db: &Db,
    bus: &EventBus,
    project_id: &str,
    assignment_id: Option<&str>,
    kind: &str,
    detail: Option<&str>,
) -> Result<ProjectActivityEntry, PersistenceError> {
    let entry =
        persistence::record_project_activity(db, project_id, assignment_id, kind, detail).await?;
    let wire = ProjectActivityEntryResponse {
        id: entry.id.clone(),
        project_id: entry.project_id.clone(),
        assignment_id: entry.assignment_id.clone(),
        kind: entry.kind.clone(),
        detail: entry.detail.clone(),
        recorded_at: entry.recorded_at.clone(),
    };
    bus.publish(
        &ProjectId(project_id.to_string()),
        ProjectEvent::Activity(wire),
    );
    Ok(entry)
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
    publish_issue_source_sync_started(bus, project_id);

    match f().await {
        Ok(resp) => {
            publish_issue_source_sync_completed(bus, project_id, resp.clone());
            Ok(resp)
        }
        Err(e) => {
            publish_issue_source_sync_failed(bus, project_id, &e.detail());
            Err(e)
        }
    }
}

pub fn publish_project_changed(bus: &EventBus, project_id: &str, project: ProjectResponse) -> u64 {
    bus.publish(
        &ProjectId(project_id.to_string()),
        ProjectEvent::ProjectChanged(project),
    )
}

pub fn publish_auto_replan_state_changed(
    bus: &EventBus,
    project_id: &str,
    state: AutoReplanState,
    reason: Option<PauseReason>,
) -> u64 {
    bus.publish(
        &ProjectId(project_id.to_string()),
        ProjectEvent::AutoReplanStateChanged { state, reason },
    )
}

pub fn publish_assignment_created(
    bus: &EventBus,
    project_id: &str,
    assignment: IssueAssignmentResponse,
) -> u64 {
    bus.publish(
        &ProjectId(project_id.to_string()),
        ProjectEvent::AssignmentCreated(assignment),
    )
}

pub fn publish_assignment_status_changed(
    bus: &EventBus,
    project_id: &str,
    assignment: IssueAssignmentResponse,
) -> u64 {
    bus.publish(
        &ProjectId(project_id.to_string()),
        ProjectEvent::AssignmentStatusChanged(assignment),
    )
}

pub fn publish_assignment_attempt_added(
    bus: &EventBus,
    project_id: &str,
    assignment_id: &str,
    attempt: AssignmentAttemptResponse,
) -> u64 {
    bus.publish(
        &ProjectId(project_id.to_string()),
        ProjectEvent::AssignmentAttemptAdded {
            assignment_id: assignment_id.to_string(),
            attempt,
        },
    )
}

pub fn publish_plan_run_started(
    bus: &EventBus,
    project_id: &str,
    plan_run: PlanRunResponse,
) -> u64 {
    bus.publish(
        &ProjectId(project_id.to_string()),
        ProjectEvent::PlanRunStarted(plan_run),
    )
}

pub fn publish_plan_run_phase_completed(
    bus: &EventBus,
    project_id: &str,
    plan_run_id: &str,
    phase_output: PhaseOutputResponse,
) -> u64 {
    bus.publish(
        &ProjectId(project_id.to_string()),
        ProjectEvent::PlanRunPhaseCompleted {
            plan_run_id: plan_run_id.to_string(),
            phase_output,
        },
    )
}

pub fn publish_plan_run_completed(
    bus: &EventBus,
    project_id: &str,
    plan_run: PlanRunResponse,
) -> u64 {
    bus.publish(
        &ProjectId(project_id.to_string()),
        ProjectEvent::PlanRunCompleted(plan_run),
    )
}

pub fn publish_project_execution_config_changed(
    bus: &EventBus,
    project_id: &str,
    config: ProjectExecutionConfigResponse,
) -> u64 {
    bus.publish(
        &ProjectId(project_id.to_string()),
        ProjectEvent::ProjectExecutionConfigChanged(config),
    )
}

pub fn publish_planning_snapshot_changed(
    bus: &EventBus,
    project_id: &str,
    snapshot: Option<PlanningSnapshotResponse>,
) -> u64 {
    bus.publish(
        &ProjectId(project_id.to_string()),
        ProjectEvent::PlanningSnapshotChanged { snapshot },
    )
}

pub fn publish_issue_source_sync_started(bus: &EventBus, project_id: &str) -> u64 {
    bus.publish(
        &ProjectId(project_id.to_string()),
        ProjectEvent::IssueSourceSyncStarted,
    )
}

pub fn publish_issue_source_sync_completed(
    bus: &EventBus,
    project_id: &str,
    response: IssueSourceSyncResponse,
) -> u64 {
    bus.publish(
        &ProjectId(project_id.to_string()),
        ProjectEvent::IssueSourceSyncCompleted(response),
    )
}

pub fn publish_issue_source_sync_failed(bus: &EventBus, project_id: &str, error: &str) -> u64 {
    bus.publish(
        &ProjectId(project_id.to_string()),
        ProjectEvent::IssueSourceSyncFailed {
            error: error.to_string(),
        },
    )
}

pub fn publish_issue_source_candidates_changed(
    bus: &EventBus,
    project_id: &str,
    candidates: Vec<IssueSourceCandidate>,
) -> u64 {
    bus.publish(
        &ProjectId(project_id.to_string()),
        ProjectEvent::IssueSourceCandidatesChanged { candidates },
    )
}
