//! HTTP handler for abandoning a blocked Issue Assignment.

use std::sync::Arc;

use agentic_afk_contracts::{IssueAssignmentResponse, ProblemDetail};
use agentic_afk_orchestrator::remove_assignment_worktree;
use agentic_afk_persistence::{self as persistence, PersistenceError};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::AppState;

pub(crate) async fn abandon_assignment(
    State(state): State<Arc<AppState>>,
    Path((project_id, assignment_id)): Path<(String, String)>,
) -> Response {
    let project = match persistence::get_project(&state.db, &project_id).await {
        Ok(project) => project,
        Err(error) => return crate::persistence_error_to_response(error),
    };
    let assignment =
        match persistence::get_project_assignment(&state.db, &project_id, &assignment_id).await {
            Ok(assignment) => assignment,
            Err(error) => return crate::persistence_error_to_response(error),
        };
    if assignment.status != "blocked" {
        return problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "urn:agentic-afk:assignment-not-abandonable",
            format!(
                "Issue Assignment must be blocked to be abandoned, was {}",
                assignment.status
            ),
        );
    }

    if let Err(detail) = remove_assignment_worktree(
        &state.config.worktrunk_binary_path,
        std::path::Path::new(&project.path),
        &assignment.branch,
    ) {
        let _ = crate::activity_publisher::record_project_activity(
            &state.db,
            &state.event_bus,
            &project_id,
            Some(&assignment_id),
            "assignment_abandon_failed",
            Some(&detail),
        )
        .await;
        return problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "urn:agentic-afk:assignment-cleanup-failed",
            detail,
        );
    }

    let abandoned = match persistence::abandon_blocked_assignment(&state.db, &assignment_id).await {
        Ok(abandoned) => abandoned,
        Err(PersistenceError::AssignmentNotAbandonable(status)) => {
            return problem(
                StatusCode::UNPROCESSABLE_ENTITY,
                "urn:agentic-afk:assignment-not-abandonable",
                format!("Issue Assignment must be blocked to be abandoned, was {status}"),
            );
        }
        Err(error) => return crate::persistence_error_to_response(error),
    };

    if let Some(source) = project.enabled_issue_source.clone() {
        let _ = crate::write_assignment_lifecycle_for_abandon(
            &state.config.gh_binary_path,
            &project,
            &source,
            &assignment.source_id,
        );
        if source.kind == "local_markdown" {
            let _ = crate::refresh_local_markdown_after_change(&state.db, &project, &source).await;
        }
    }

    let _ = crate::activity_publisher::record_project_activity(
        &state.db,
        &state.event_bus,
        &project_id,
        Some(&assignment_id),
        "assignment_abandoned",
        Some(&abandoned.branch),
    )
    .await;

    (StatusCode::OK, Json::<IssueAssignmentResponse>(abandoned)).into_response()
}

fn problem(status: StatusCode, problem_type: &str, detail: String) -> Response {
    (
        status,
        [("content-type", "application/problem+json")],
        Json(ProblemDetail {
            problem_type: problem_type.to_string(),
            title: status.canonical_reason().unwrap_or("Error").to_string(),
            status: status.as_u16(),
            detail,
        }),
    )
        .into_response()
}
