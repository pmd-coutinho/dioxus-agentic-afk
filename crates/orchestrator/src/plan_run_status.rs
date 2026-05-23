//! Typed Assignment Status transitions for the Plan Run coordinator.
//!
//! `AssignmentStatus` mirrors the persisted status values for an
//! `IssueAssignment` row inside a Plan Run. `transition_assignment` is the
//! single helper that persists the new status and publishes the
//! `AssignmentStatusChanged` event, replacing ~10 inline pairings of
//! `persistence::set_assignment_status` followed by
//! `events.assignment_status_changed`.

use std::sync::Arc;

use agentic_afk_contracts::{BlockReason, IssueAssignmentResponse};
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
    /// The Merge Phase has integrated locally and verified, but the
    /// Integration Branch push has not yet succeeded. Dormant for Max
    /// Parallel Tasks. See ADR-0037 and CONTEXT.md → Assignment Status.
    MergeStaged,
    Merged,
    /// Blocked with a typed **Block Reason** (ADR-0038). `kind` drives
    /// Dashboard affordances; `detail` is the optional freeform text
    /// surfaced as the status detail.
    Blocked {
        kind: BlockReason,
        detail: String,
    },
}

impl AssignmentStatus {
    /// Persisted string discriminator used in the `issue_assignments.status`
    /// column. Pairs with `persistence::set_assignment_status` /
    /// `persistence::record_blocked_with_kind`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Implementing => "implementing",
            Self::Implemented => "implemented",
            Self::Reviewed => "reviewed",
            Self::Merging => "merging",
            Self::MergeStaged => "merge_staged",
            Self::Merged => "merged",
            Self::Blocked { .. } => "blocked",
        }
    }
}

/// Persist a new `AssignmentStatus` and publish the resulting
/// `AssignmentStatusChanged` event in one step. Blocked transitions use
/// `persistence::record_blocked_with_kind` so the typed `block_reason_kind`
/// and freeform `block_reason` detail (ADR-0038) are set alongside the
/// status; every other transition uses `persistence::set_assignment_status`.
pub async fn transition_assignment(
    db: &Db,
    events: &Arc<dyn EventPublisher>,
    project_id: &str,
    assignment_id: &str,
    status: AssignmentStatus,
) -> Result<IssueAssignmentResponse, CoordinatorError> {
    let updated = match &status {
        AssignmentStatus::Blocked { kind, detail } => {
            // An empty `detail` is persisted as `NULL` so the round-tripped
            // `BlockReasonResponse.detail` is `None` rather than
            // `Some("")`. Callers that supply meaningful text always reach
            // this branch with a non-empty string (push stderr, conflict
            // summary, operator note).
            let persisted_detail = if detail.is_empty() {
                None
            } else {
                Some(detail.as_str())
            };
            persistence::record_blocked_with_kind(db, assignment_id, *kind, persisted_detail)
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
