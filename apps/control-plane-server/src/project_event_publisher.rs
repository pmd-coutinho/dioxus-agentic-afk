//! Single funnel for non-Activity `ProjectEvent` publishes (issue #32).
//!
//! Activity entries flow through [`crate::activity_publisher`] because they
//! also append to the audit log. Other lifecycle events (Plan Run, Assignment,
//! Planning, Issue Source) are pure live-only deltas — they have no
//! corresponding audit-log row beyond the Activity entry that may already
//! describe them. Routing them through this module keeps the audit trail of
//! "where Project events get published" in one place rather than scattering
//! `event_bus.publish` calls across handlers.

use agentic_afk_contracts::{
    AssignmentAttemptResponse, IssueAssignmentResponse, IssueSourceCandidate,
    IssueSourceSyncResponse, PhaseOutputResponse, PlanRunResponse, PlanningSnapshotResponse,
    ProjectEvent, ProjectExecutionConfigResponse, ProjectId, ProjectResponse,
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

pub fn publish_plan_run_started(bus: &EventBus, project_id: &str, plan_run: PlanRunResponse) -> u64 {
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

pub fn publish_plan_run_completed(bus: &EventBus, project_id: &str, plan_run: PlanRunResponse) -> u64 {
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
