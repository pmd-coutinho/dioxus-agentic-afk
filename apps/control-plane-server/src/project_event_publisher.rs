//! Single funnel for non-Activity `ProjectEvent` publishes (issue #32).
//!
//! Activity entries flow through [`crate::activity_publisher`] because they
//! also append to the audit log. Other lifecycle events (Assignment, Change
//! Proposal, Planning, Issue Source) are pure live-only deltas — they have no
//! corresponding audit-log row beyond the Activity entry that may already
//! describe them. Routing them through this module keeps the audit trail of
//! "where Project events get published" in one place rather than scattering
//! `event_bus.publish` calls across handlers.

use agentic_afk_contracts::{
    AssignmentAttemptResponse, ChangeProposalResponse, IssueAssignmentResponse,
    IssueSourceCandidate, IssueSourceSyncResponse, PlanningSnapshotResponse, ProjectEvent,
    ProjectId, ProjectResponse,
};

pub fn publish_project_changed(bus: &EventBus, project_id: &str, project: ProjectResponse) -> u64 {
    bus.publish(
        &ProjectId(project_id.to_string()),
        ProjectEvent::ProjectChanged(project),
    )
}

use crate::event_bus::EventBus;

pub fn publish_assignment_created(bus: &EventBus, project_id: &str, assignment: IssueAssignmentResponse) -> u64 {
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

pub fn publish_change_proposal_refreshed(
    bus: &EventBus,
    project_id: &str,
    assignment_id: &str,
    change_proposal: ChangeProposalResponse,
) -> u64 {
    bus.publish(
        &ProjectId(project_id.to_string()),
        ProjectEvent::ChangeProposalRefreshed {
            assignment_id: assignment_id.to_string(),
            change_proposal,
        },
    )
}

pub fn publish_change_proposal_verified(
    bus: &EventBus,
    project_id: &str,
    assignment_id: &str,
    change_proposal: ChangeProposalResponse,
) -> u64 {
    bus.publish(
        &ProjectId(project_id.to_string()),
        ProjectEvent::ChangeProposalVerified {
            assignment_id: assignment_id.to_string(),
            change_proposal,
        },
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
