//! Single public entry point for appending Project Activity (ADR-0032).
//!
//! Every Control Plane Activity write goes through here so the audit log
//! (`project_activity` table) and the live SSE wire format
//! (`event_bus`) stay a single source of truth. Callers must never call
//! `persistence::record_project_activity` directly.

use agentic_afk_contracts::{
    ProjectActivityEntryResponse, ProjectEvent, ProjectId,
};
use agentic_afk_persistence::{self as persistence, Db, PersistenceError, ProjectActivityEntry};

use crate::event_bus::EventBus;

/// Persist one Project Activity entry and publish the matching
/// `ProjectEvent::Activity` delta on the live event bus.
///
/// Returns the persisted entry on success. The activity write is the source
/// of truth; if the DB write fails the bus is not touched.
pub async fn record_project_activity(
    db: &Db,
    bus: &EventBus,
    project_id: &str,
    assignment_id: Option<&str>,
    kind: &str,
    detail: Option<&str>,
) -> Result<ProjectActivityEntry, PersistenceError> {
    let entry = persistence::record_project_activity(
        db,
        project_id,
        assignment_id,
        kind,
        detail,
    )
    .await?;
    let wire = ProjectActivityEntryResponse {
        id: entry.id.clone(),
        project_id: entry.project_id.clone(),
        assignment_id: entry.assignment_id.clone(),
        kind: entry.kind.clone(),
        detail: entry.detail.clone(),
        recorded_at: entry.recorded_at.clone(),
    };
    bus.publish(&ProjectId(project_id.to_string()), ProjectEvent::Activity(wire));
    Ok(entry)
}
