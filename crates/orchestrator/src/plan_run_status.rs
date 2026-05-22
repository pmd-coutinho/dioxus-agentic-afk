//! Typed Assignment Status transitions for the Plan Run coordinator.
//!
//! `AssignmentStatus` mirrors the persisted status values for an
//! `IssueAssignment` row inside a Plan Run. `transition_assignment` is the
//! single helper that persists the new status and publishes the
//! `AssignmentStatusChanged` event, replacing ~10 inline pairings of
//! `persistence::set_assignment_status` followed by
//! `events.assignment_status_changed`.

use std::sync::Arc;

use agentic_afk_contracts::IssueAssignmentResponse;
use agentic_afk_persistence::{self as persistence, Db};

use crate::coordinator::{CoordinatorError, EventPublisher};

/// The fine-grained execution state of one Issue Assignment inside a Plan
/// Run, distinct from the coarse `LifecycleStatus` written back to the
/// Issue Source. See CONTEXT.md → Assignment Status.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssignmentStatus {
    Implementing,
    Implemented,
    Reviewed,
    Merging,
    Merged,
    Blocked { reason: String },
}

impl AssignmentStatus {
    /// Persisted string discriminator used in the `issue_assignments.status`
    /// column. Pairs with `persistence::set_assignment_status` /
    /// `persistence::block_assignment`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Implementing => "implementing",
            Self::Implemented => "implemented",
            Self::Reviewed => "reviewed",
            Self::Merging => "merging",
            Self::Merged => "merged",
            Self::Blocked { .. } => "blocked",
        }
    }
}

/// Persist a new `AssignmentStatus` and publish the resulting
/// `AssignmentStatusChanged` event in one step. Blocked transitions use
/// `persistence::block_assignment` so the `block_reason` column is set
/// alongside the status; every other transition uses
/// `persistence::set_assignment_status`.
pub async fn transition_assignment(
    db: &Db,
    events: &Arc<dyn EventPublisher>,
    project_id: &str,
    assignment_id: &str,
    status: AssignmentStatus,
) -> Result<IssueAssignmentResponse, CoordinatorError> {
    let updated = match &status {
        AssignmentStatus::Blocked { reason } => {
            persistence::block_assignment(db, assignment_id, reason)
                .await
                .map_err(CoordinatorError::from_persistence)?
        }
        other => persistence::set_assignment_status(db, assignment_id, other.as_str(), None)
            .await
            .map_err(CoordinatorError::from_persistence)?,
    };
    events.assignment_status_changed(project_id, updated.clone());
    Ok(updated)
}
