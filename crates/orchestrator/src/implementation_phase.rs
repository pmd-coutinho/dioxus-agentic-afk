//! Implementation Phase: prompt rendering and outcome check.
//!
//! Implementation does not own a Review Loop of its own — it runs as the
//! first half of one [`crate::review_loop`] iteration. This module owns
//! the implementation prompt template and the typed rejection that fires
//! when the agent returns an outcome other than `ready_for_review`.

use agentic_afk_contracts::{
    IssueAssignmentResponse, PlanRunResponse, ProjectExecutionConfigResponse, ProjectResponse,
};

use crate::coordinator::CoordinatorError;
use crate::plan_run::RefreshedBaseline;

/// Why an implementation pass did not enter the Review Phase.
#[derive(Clone, Debug)]
pub enum ImplementationRejection {
    /// `deps.implementation.run` failed (process error, parse error,
    /// etc).
    PhaseFailed(String),
    /// Implementation output did not parse into a known schema.
    Unparseable(String),
    /// The implementation outcome was not `ready_for_review`, so the
    /// Review Phase cannot accept it.
    NotReadyForReview { outcome: String },
}

impl From<ImplementationRejection> for CoordinatorError {
    fn from(rejection: ImplementationRejection) -> Self {
        match rejection {
            ImplementationRejection::PhaseFailed(error) => {
                CoordinatorError::new(500, "urn:agentic-afk:implementation-phase-failed", error)
            }
            ImplementationRejection::Unparseable(error) => CoordinatorError::new(
                500,
                "urn:agentic-afk:implementation-output-unparseable",
                error,
            ),
            ImplementationRejection::NotReadyForReview { outcome } => CoordinatorError::new(
                500,
                "urn:agentic-afk:implementation-not-ready",
                format!("implementation outcome `{outcome}` does not enter Review Phase"),
            ),
        }
    }
}

/// Phase name written to `phase_outputs.phase`.
pub const PHASE_NAME: &str = "implementation";

/// Confirm the parsed implementation outcome enters the Review Phase.
/// Today the only accepted outcome is `ready_for_review`; anything else
/// is treated as a block-the-assignment signal.
pub fn check_implementation_outcome(outcome: &str) -> Result<(), ImplementationRejection> {
    if outcome == "ready_for_review" {
        Ok(())
    } else {
        Err(ImplementationRejection::NotReadyForReview {
            outcome: outcome.to_string(),
        })
    }
}

/// Extract the agent's freeform `block_reason` from a parsed Implementation
/// Phase body, with sensible fallbacks so the typed BlockReason always
/// carries human-readable detail even when the agent omitted the field.
/// Fallback order: `block_reason` → `summary` → synthetic outcome marker.
pub fn extract_implementation_block_reason(
    body: &serde_json::Value,
    outcome: &str,
) -> String {
    if let Some(reason) = body.get("block_reason").and_then(serde_json::Value::as_str) {
        let trimmed = reason.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if let Some(summary) = body.get("summary").and_then(serde_json::Value::as_str) {
        let trimmed = summary.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    format!("implementation outcome `{outcome}` reported without block_reason")
}

#[allow(clippy::too_many_arguments)]
pub fn render_implementation_prompt(
    project_instructions: &str,
    project: &ProjectResponse,
    plan_run: &PlanRunResponse,
    baseline: &RefreshedBaseline,
    config: &ProjectExecutionConfigResponse,
    assignment: &IssueAssignmentResponse,
    source_issue_brief: &str,
    review_findings: &str,
) -> String {
    let template = include_str!("../prompts/plan-run/implement.md");
    template
        .replace("{{PROJECT_INSTRUCTIONS}}", project_instructions)
        .replace("{{PROJECT_NAME}}", &project.path)
        .replace("{{PLAN_RUN_ID}}", &plan_run.id)
        .replace("{{PLAN_RUN_BASELINE}}", &baseline.commit_sha)
        .replace("{{INTEGRATION_BRANCH}}", &config.integration_branch)
        .replace("{{SOURCE_ISSUE_ID}}", &assignment.source_id)
        .replace("{{SOURCE_ISSUE_TITLE}}", &assignment.source_title)
        .replace("{{ISSUE_BRANCH}}", &assignment.branch)
        .replace("{{SOURCE_ISSUE_BRIEF}}", source_issue_brief)
        .replace("{{REVIEW_FINDINGS}}", review_findings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_for_review_passes() {
        assert!(check_implementation_outcome("ready_for_review").is_ok());
    }

    #[test]
    fn blocked_outcome_rejected() {
        let err = check_implementation_outcome("blocked").unwrap_err();
        assert!(matches!(
            err,
            ImplementationRejection::NotReadyForReview { outcome } if outcome == "blocked"
        ));
    }

    #[test]
    fn extract_block_reason_prefers_explicit_field() {
        let body = serde_json::json!({"block_reason": "needs ADR clarification", "summary": "x"});
        assert_eq!(
            extract_implementation_block_reason(&body, "blocked"),
            "needs ADR clarification"
        );
    }

    #[test]
    fn extract_block_reason_falls_back_to_summary() {
        let body = serde_json::json!({"summary": "stopped after migration"});
        assert_eq!(
            extract_implementation_block_reason(&body, "blocked"),
            "stopped after migration"
        );
    }

    #[test]
    fn extract_block_reason_falls_back_to_synthetic_marker() {
        let body = serde_json::json!({});
        assert_eq!(
            extract_implementation_block_reason(&body, "failed"),
            "implementation outcome `failed` reported without block_reason"
        );
    }

    #[test]
    fn extract_block_reason_skips_empty_strings() {
        let body = serde_json::json!({"block_reason": "   ", "summary": "real summary"});
        assert_eq!(
            extract_implementation_block_reason(&body, "blocked"),
            "real summary"
        );
    }
}
