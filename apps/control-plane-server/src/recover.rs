//! POST /api/projects/{id}/assignments/{assignment_id}/recover handler.
//!
//! Recovery continues a blocked Issue Assignment in its existing Assignment Worktree
//! under one replacement Codex process. The handler:
//!
//! 1. Confirms the assignment exists and belongs to the Project.
//! 2. Refuses unless the current status is `blocked` (recovery is not a generic
//!    re-run; it is specifically for blocked work). This keeps "abandon" and
//!    "recover" semantically distinct.
//! 3. Verifies and stops the prior Codex process when its identity can be matched,
//!    so two owned agents never share one Assignment Worktree.
//! 4. Builds a Codex prompt from durable facts only — the Source Issue text, branch,
//!    worktree path, prior process identity (if any), prior block reason.
//! 5. Runs Codex against the existing Assignment Worktree, persists a `recovery`
//!    Assignment Attempt, and updates assignment status from the structured terminal
//!    outcome. Recovery never charges the CI repair budget — it lives in a separate
//!    attempt kind and a separate code path from #23's repair flow.

use std::path::Path;
use std::sync::Arc;

use agentic_afk_contracts::AssignmentTerminalOutcome;
use agentic_afk_orchestrator::{
    RecoveryPromptFacts, build_recovery_prompt, codex_process_identity, run_recovery_codex,
    stop_prior_codex_if_owned,
};
use agentic_afk_persistence::{
    self as persistence, PersistenceError, get_issue_assignment_public,
    list_assignment_attempts, record_recovery_attempt,
};
use axum::Json;
use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::Response;

use crate::{AppState, assignment_problem, persistence_error_to_response, sync_problem_response};

pub async fn recover_assignment(
    State(state): State<Arc<AppState>>,
    AxumPath((project_id, assignment_id)): AxumPath<(String, String)>,
) -> Response {
    let assignment = match get_issue_assignment_public(&state.db, &assignment_id).await {
        Ok(assignment) if assignment.project_id.0 == project_id => assignment,
        Ok(_) => {
            return sync_problem_response(
                StatusCode::NOT_FOUND,
                "urn:agentic-afk:assignment-not-found",
                "Not Found",
                format!("Issue Assignment not found: {assignment_id}"),
            );
        }
        Err(PersistenceError::AssignmentNotFound(_)) => {
            return sync_problem_response(
                StatusCode::NOT_FOUND,
                "urn:agentic-afk:assignment-not-found",
                "Not Found",
                format!("Issue Assignment not found: {assignment_id}"),
            );
        }
        Err(error) => return persistence_error_to_response(error),
    };

    if assignment.status != "blocked" {
        return sync_problem_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "urn:agentic-afk:assignment-not-recoverable",
            "Unprocessable Entity",
            format!(
                "Issue Assignment is not in a recoverable state: {} (only `blocked` can be recovered)",
                assignment.status
            ),
        );
    }

    if assignment.worktree_path.is_empty() {
        return sync_problem_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "urn:agentic-afk:assignment-not-recoverable",
            "Unprocessable Entity",
            "Blocked Issue Assignment has no Assignment Worktree to recover into"
                .to_string(),
        );
    }

    // Pull all attempts so we can build the recovery prompt from durable facts only.
    let attempts = match list_assignment_attempts(&state.db, &assignment.id).await {
        Ok(attempts) => attempts,
        Err(error) => return persistence_error_to_response(error),
    };
    let prior_attempt = attempts.last();
    let prior_pid = prior_attempt.and_then(|attempt| attempt.process_id);
    let prior_identity = prior_attempt
        .and_then(|attempt| attempt.process_identity.clone());
    let prior_block_reason = prior_attempt
        .and_then(|attempt| attempt.terminal_outcome.as_ref())
        .map(|outcome| outcome.summary.clone())
        .or_else(|| assignment.status_detail.clone());

    // Verify-and-stop any still-owned prior Codex process before spawning the
    // replacement. We never permit two owned agents on one Assignment Worktree.
    let _ = stop_prior_codex_if_owned(
        prior_pid,
        prior_identity.as_deref(),
        codex_process_identity,
    );

    // Re-look up the worktree path on disk (recovery never overwrites it; the
    // Assignment Worktree row is the single source of truth).
    let worktree_path = std::path::PathBuf::from(&assignment.worktree_path);

    // Resolve the original source raw text from the persisted Issue Assignment row.
    let source_raw_text =
        match persistence::get_assignment_source_raw_text(&state.db, &assignment.id).await {
            Ok(raw) => raw,
            Err(error) => return persistence_error_to_response(error),
        };

    let prompt = build_recovery_prompt(RecoveryPromptFacts {
        source_issue_raw_text: &source_raw_text,
        source_id: &assignment.source_id,
        assignment_branch: &assignment.branch,
        assignment_worktree_path: &assignment.worktree_path,
        prior_process_identity: prior_identity.as_deref(),
        prior_block_reason: prior_block_reason.as_deref(),
    });

    let execution = run_recovery_codex(
        &state.config.codex_binary_path,
        Path::new(&assignment.worktree_path),
        &prompt,
    );

    let final_assignment = match execution {
        Ok(execution) => {
            if let Err(error) = record_recovery_attempt(
                &state.db,
                &assignment.id,
                Some(execution.process_id),
                execution.process_identity.as_deref(),
                Some(&execution.terminal_outcome),
            )
            .await
            {
                return persistence_error_to_response(error);
            }
            let (status, detail) = recovery_status_from_outcome(&execution.terminal_outcome);
            match persistence::set_assignment_status(
                &state.db,
                &assignment.id,
                status,
                detail.as_deref(),
            )
            .await
            {
                Ok(updated) => updated,
                Err(error) => return persistence_error_to_response(error),
            }
        }
        Err(detail) => {
            let failed = AssignmentTerminalOutcome {
                outcome: "Failed".to_string(),
                summary: detail.clone(),
            };
            if let Err(error) =
                record_recovery_attempt(&state.db, &assignment.id, None, None, Some(&failed))
                    .await
            {
                return persistence_error_to_response(error);
            }
            match persistence::set_assignment_status(
                &state.db,
                &assignment.id,
                "failed",
                Some(&detail),
            )
            .await
            {
                Ok(updated) => updated,
                Err(error) => return persistence_error_to_response(error),
            }
        }
    };

    let _ = worktree_path; // kept above for clarity that recovery reuses the same path.
    let _ = assignment_problem; // suppress unused-import warning when reusing helpers later.

    let _ = crate::activity_publisher::record_project_activity(
        &state.db,
        &state.event_bus,
        &project_id,
        Some(&final_assignment.id),
        "assignment_recovered",
        final_assignment.status_detail.as_deref().or(Some(&final_assignment.status)),
    )
    .await;

    (StatusCode::CREATED, Json(final_assignment)).into_response()
}

fn recovery_status_from_outcome(outcome: &AssignmentTerminalOutcome) -> (&'static str, Option<String>) {
    match outcome.outcome.as_str() {
        // Recovery never auto-creates a Change Proposal here; local markdown has no
        // proposal target and GitHub proposal creation belongs to the verified
        // initial/repair path. Recovery hands a successful pass back as `blocked`
        // with an explanatory detail until the developer re-runs the standard
        // proposal flow. This avoids invented prior-agent reasoning and keeps the
        // recovery surface narrow.
        "ReadyForProposal" => (
            "blocked",
            Some(
                "Recovery reached ReadyForProposal; review the worktree and re-run the proposal flow."
                    .to_string(),
            ),
        ),
        "Failed" => ("failed", Some(outcome.summary.clone())),
        _ => ("blocked", Some(outcome.summary.clone())),
    }
}

use axum::response::IntoResponse;
