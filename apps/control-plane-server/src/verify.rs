//! Route handler for refreshing Change Proposal state.
//!
//! Inspects required GitHub checks, advances the assignment through
//! `proposal_pending` -> `proposal_verified` -> `completed` lifecycle states,
//! writes Completed lifecycle back to the Source Issue on Human Merge, and
//! cleans up the accepted Assignment Worktree and deterministic branch.

use crate::{AppState, persistence_error_to_response, sync_problem_response};
use agentic_afk_orchestrator::verify::{
    CheckState, cleanup_assignment_worktree, inspect_required_checks, is_pull_request_merged,
    parse_pull_request_number,
};
use agentic_afk_persistence::{self as persistence, verify as persistence_verify};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use std::sync::Arc;

pub(crate) async fn refresh_proposal_state(
    State(state): State<Arc<AppState>>,
    Path((project_id, assignment_id)): Path<(String, String)>,
) -> Response {
    let assignment = match persistence_verify::get_assignment_by_id(&state.db, &assignment_id).await
    {
        Ok(assignment) => assignment,
        Err(error) => return persistence_error_to_response(error),
    };
    if assignment.project_id.0 != project_id {
        return sync_problem_response(
            StatusCode::NOT_FOUND,
            "urn:agentic-afk:assignment-not-found",
            "Not Found",
            format!(
                "Issue Assignment {assignment_id} does not belong to Project {project_id}"
            ),
        );
    }
    // Already completed: idempotent return.
    if assignment.status == "completed" {
        return Json(assignment).into_response();
    }

    let Some(proposal) = assignment.change_proposal.clone() else {
        return sync_problem_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "urn:agentic-afk:assignment-has-no-change-proposal",
            "Unprocessable Entity",
            "Issue Assignment has no Change Proposal to verify".to_string(),
        );
    };

    let project = match persistence::get_project(&state.db, &project_id).await {
        Ok(project) => project,
        Err(error) => return persistence_error_to_response(error),
    };
    let Some(source) = project.enabled_issue_source.clone() else {
        return sync_problem_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "urn:agentic-afk:issue-source-not-enabled",
            "Unprocessable Entity",
            "Project has no enabled Issue Source".to_string(),
        );
    };
    if source.kind != "github" {
        return sync_problem_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "urn:agentic-afk:proposal-host-not-supported",
            "Unprocessable Entity",
            "Change Proposal verification is only supported for GitHub Issue Sources".to_string(),
        );
    }

    let Some(pull_request_number) = parse_pull_request_number(&proposal.url) else {
        return sync_problem_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "urn:agentic-afk:invalid-change-proposal-url",
            "Unprocessable Entity",
            format!("Change Proposal URL is not a recognized pull request: {}", proposal.url),
        );
    };

    // First check whether the pull request has been Human Merged. Merge wins
    // over check state, since GitHub closes pending checks on merge.
    let merged = match is_pull_request_merged(
        &state.config.gh_binary_path,
        &source.locator,
        pull_request_number,
    ) {
        Ok(merged) => merged,
        Err(detail) => {
            return sync_problem_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "urn:agentic-afk:proposal-state-refresh-failed",
                "Unprocessable Entity",
                detail,
            );
        }
    };
    if merged {
        // Write Completed lifecycle back to the Source Issue.
        let _ = crate::write_github_lifecycle_pub(
            &state.config.gh_binary_path,
            &source.locator,
            &assignment.source_id,
            "completed",
        );
        // Best-effort cleanup of the Assignment Worktree and branch.
        let _ = cleanup_assignment_worktree(
            &state.config.worktrunk_binary_path,
            std::path::Path::new(&project.path),
            &assignment.branch,
        );
        let updated = match persistence_verify::mark_assignment_completed(
            &state.db,
            &assignment.id,
        )
        .await
        {
            Ok(assignment) => assignment,
            Err(error) => return persistence_error_to_response(error),
        };
        crate::project_event_publisher::publish_assignment_status_changed(
            &state.event_bus,
            &project_id,
            updated.clone(),
        );
        let _ = crate::activity_publisher::record_project_activity(
            &state.db,
            &state.event_bus,
            &project_id,
            Some(&updated.id),
            "assignment_completed",
            updated
                .change_proposal
                .as_ref()
                .map(|proposal| proposal.url.as_str()),
        )
        .await;
        let _ = crate::activity_publisher::record_project_activity(
            &state.db,
            &state.event_bus,
            &project_id,
            Some(&updated.id),
            "assignment_cleanup",
            Some(&updated.branch),
        )
        .await;
        return Json(updated).into_response();
    }

    // Not merged. Inspect required checks to decide pending / verified / failing.
    let check_state = match inspect_required_checks(
        &state.config.gh_binary_path,
        &source.locator,
        pull_request_number,
    ) {
        Ok(state) => state,
        Err(detail) => {
            return sync_problem_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "urn:agentic-afk:proposal-state-refresh-failed",
                "Unprocessable Entity",
                detail,
            );
        }
    };

    let updated = match check_state {
        CheckState::Pending => {
            // No transition. Publish a refreshed Change Proposal delta so
            // the live Dashboard can re-render with the latest known
            // proposal state even though persistence did not change.
            if let Some(proposal) = assignment.change_proposal.clone() {
                crate::project_event_publisher::publish_change_proposal_refreshed(
                    &state.event_bus,
                    &project_id,
                    &assignment.id,
                    proposal,
                );
            }
            assignment
        }
        CheckState::Passing => {
            let verified =
                match persistence_verify::mark_proposal_verified(&state.db, &assignment.id).await {
                    Ok(assignment) => assignment,
                    Err(error) => return persistence_error_to_response(error),
                };
            if let Some(proposal) = verified.change_proposal.clone() {
                crate::project_event_publisher::publish_change_proposal_verified(
                    &state.event_bus,
                    &project_id,
                    &verified.id,
                    proposal,
                );
            }
            crate::project_event_publisher::publish_assignment_status_changed(
                &state.event_bus,
                &project_id,
                verified.clone(),
            );
            let _ = crate::activity_publisher::record_project_activity(
                &state.db,
                &state.event_bus,
                &project_id,
                Some(&verified.id),
                "change_proposal_verified",
                verified
                    .change_proposal
                    .as_ref()
                    .map(|proposal| proposal.url.as_str()),
            )
            .await;
            verified
        }
        CheckState::Failing(detail) => {
            // Mirror lifecycle to the Source Issue so source-visible state
            // matches the blocked assignment.
            let _ = crate::write_github_lifecycle_pub(
                &state.config.gh_binary_path,
                &source.locator,
                &assignment.source_id,
                "blocked",
            );
            let failing = match persistence_verify::mark_proposal_failing(
                &state.db,
                &assignment.id,
                &detail,
            )
            .await
            {
                Ok(assignment) => assignment,
                Err(error) => return persistence_error_to_response(error),
            };
            crate::project_event_publisher::publish_assignment_status_changed(
                &state.event_bus,
                &project_id,
                failing.clone(),
            );
            let _ = crate::activity_publisher::record_project_activity(
                &state.db,
                &state.event_bus,
                &project_id,
                Some(&failing.id),
                "change_proposal_failing",
                Some(&detail),
            )
            .await;
            failing
        }
    };

    Json(updated).into_response()
}

use axum::response::IntoResponse;
