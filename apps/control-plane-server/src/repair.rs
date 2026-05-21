//! HTTP handler for starting a repair Assignment Attempt against a failed
//! GitHub Change Proposal.
//!
//! The handler reuses the existing Issue Assignment and Assignment Worktree
//! rather than creating a fresh assignment, and enforces the bounded
//! Repair Loop (attempt count + elapsed window). When the budget is
//! exhausted the assignment is blocked with source-visible state, leaving
//! recovery and abandonment as the next operator decisions.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use agentic_afk_contracts::{
    AssignmentTerminalOutcome, FailedCheckFact, IssueAssignmentResponse, ProblemDetail,
    RepairAssignmentRequest,
};
use agentic_afk_orchestrator::repair::{RepairPromptFacts, run_repair_codex};
use agentic_afk_persistence::{self as persistence, PersistenceError};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::AppState;

pub(crate) async fn repair_assignment(
    State(state): State<Arc<AppState>>,
    Path((id, assignment_id)): Path<(String, String)>,
    payload: Option<Json<RepairAssignmentRequest>>,
) -> Response {
    let request = payload.map(|Json(body)| body).unwrap_or_default();

    let project = match persistence::get_project(&state.db, &id).await {
        Ok(project) => project,
        Err(error) => return crate::persistence_error_to_response(error),
    };
    let assignment = match persistence::get_assignment(&state.db, &assignment_id).await {
        Ok(assignment) => assignment,
        Err(error) => return crate::persistence_error_to_response(error),
    };
    if assignment.project_id.0 != project.id.0 {
        return crate::persistence_error_to_response(PersistenceError::AssignmentNotFound(
            assignment_id,
        ));
    }
    let Some(proposal) = assignment.change_proposal.clone() else {
        return problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "urn:agentic-afk:no-change-proposal",
            "Issue Assignment has no Change Proposal to repair".to_string(),
        );
    };
    let Some(source) = project.enabled_issue_source.clone() else {
        return problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "urn:agentic-afk:issue-source-not-enabled",
            "Project has no enabled Issue Source".to_string(),
        );
    };

    let now = current_unix_seconds();
    let decision =
        match persistence::repair::evaluate_repair_budget(&state.db, &assignment_id, now).await {
            Ok(decision) => decision,
            Err(error) => return crate::persistence_error_to_response(error),
        };
    if !decision.is_allow() {
        let detail = decision.block_detail().unwrap_or_else(|| {
            "repair budget exhausted; recovery or abandonment required".to_string()
        });
        let blocked = match persistence::set_assignment_status(
            &state.db,
            &assignment_id,
            "blocked",
            Some(&detail),
        )
        .await
        {
            Ok(assignment) => assignment,
            Err(error) => return crate::persistence_error_to_response(error),
        };
        let _ = crate::write_assignment_lifecycle(
            &state.config.gh_binary_path,
            &project,
            &source,
            &assignment.source_id,
            "blocked",
        );
        if source.kind == "github" {
            let _ = crate::comment_github_issue(
                &state.config.gh_binary_path,
                &source.locator,
                &assignment.source_id,
                &format!("Repair budget exhausted: {detail}"),
            );
        }
        return (StatusCode::CONFLICT, Json(blocked)).into_response();
    }

    let source_raw_text =
        match persistence::get_assignment_source_raw_text(&state.db, &assignment_id).await {
            Ok(text) => text,
            Err(error) => return crate::persistence_error_to_response(error),
        };
    let worktree_path = PathBuf::from(&assignment.worktree_path);
    let facts = RepairPromptFacts {
        source_id: &assignment.source_id,
        source_title: &assignment.source_title,
        source_raw_text: &source_raw_text,
        change_proposal_url: &proposal.url,
        branch: &assignment.branch,
        failed_checks: &request.failed_checks,
        verified_worktree_facts: request.verified_worktree_facts.as_deref(),
    };

    let execution = run_repair_codex(&state.config.codex_binary_path, &worktree_path, &facts);
    let assignment = match execution {
        Ok(execution) => {
            let assignment = match persistence::repair::record_repair_attempt(
                &state.db,
                &assignment_id,
                Some(execution.process_id),
                execution.process_identity.as_deref(),
                Some(&execution.terminal_outcome),
                now,
            )
            .await
            {
                Ok(assignment) => assignment,
                Err(error) => return crate::persistence_error_to_response(error),
            };
            apply_repair_outcome(
                state,
                project,
                source,
                assignment,
                proposal.url,
                Some(execution.terminal_outcome),
                request.failed_checks,
            )
            .await
        }
        Err(detail) => {
            let failed = AssignmentTerminalOutcome {
                outcome: "Failed".to_string(),
                summary: detail.clone(),
            };
            let assignment = match persistence::repair::record_repair_attempt(
                &state.db,
                &assignment_id,
                None,
                None,
                Some(&failed),
                now,
            )
            .await
            {
                Ok(assignment) => assignment,
                Err(error) => return crate::persistence_error_to_response(error),
            };
            apply_repair_outcome(
                state,
                project,
                source,
                assignment,
                proposal.url,
                Some(failed),
                request.failed_checks,
            )
            .await
        }
    };

    (StatusCode::OK, Json(assignment)).into_response()
}

async fn apply_repair_outcome(
    state: Arc<AppState>,
    project: agentic_afk_contracts::ProjectResponse,
    source: agentic_afk_contracts::IssueSource,
    assignment: IssueAssignmentResponse,
    proposal_url: String,
    terminal_outcome: Option<AssignmentTerminalOutcome>,
    failed_checks: Vec<FailedCheckFact>,
) -> IssueAssignmentResponse {
    let _ = failed_checks; // currently recorded only in the attempt prompt
    let outcome_kind = terminal_outcome
        .as_ref()
        .map(|outcome| outcome.outcome.as_str())
        .unwrap_or("Failed");
    let summary = terminal_outcome
        .as_ref()
        .map(|outcome| outcome.summary.clone())
        .unwrap_or_else(|| "repair attempt produced no terminal outcome".to_string());
    let (status, detail, proposal_status) = match outcome_kind {
        "ReadyForProposal" => ("proposal_pending", None, Some("pending")),
        "Failed" => ("failed", Some(summary), None),
        _ => ("blocked", Some(summary), None),
    };
    let assignment_id = assignment.id.clone();
    let _ = crate::write_assignment_lifecycle(
        &state.config.gh_binary_path,
        &project,
        &source,
        &assignment.source_id,
        if status == "proposal_pending" {
            "running"
        } else {
            "blocked"
        },
    );
    if source.kind == "github" && status != "proposal_pending"
        && let Some(detail) = detail.as_deref()
    {
        let _ = crate::comment_github_issue(
            &state.config.gh_binary_path,
            &source.locator,
            &assignment.source_id,
            &format!("Repair Assignment Attempt {status}: {detail}"),
        );
    }
    let assignment = persistence::set_assignment_status(
        &state.db,
        &assignment_id,
        status,
        detail.as_deref(),
    )
    .await
    .unwrap_or(assignment);
    let kind = match status {
        "proposal_pending" => "change_proposal_repaired",
        "failed" => "assignment_failed",
        _ => "assignment_blocked",
    };
    let _ = persistence::record_project_activity(
        &state.db,
        &assignment.project_id.0,
        Some(&assignment_id),
        kind,
        Some(proposal_url.as_str()),
    )
    .await;
    if let Some(proposal_status) = proposal_status {
        if let Ok(updated) = persistence::set_assignment_change_proposal(
            &state.db,
            &assignment_id,
            proposal_status,
            &proposal_url,
        )
        .await
        {
            return updated;
        }
    }
    assignment
}

fn current_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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
