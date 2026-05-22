//! Review Loop: prompt rendering, findings extraction, and the pure
//! `decide_review_loop_step` decision.
//!
//! One Review Loop iteration runs implementation then review against the
//! same Issue Assignment. The Review Phase agent approves or rejects with
//! findings; rejections return the assignment to another implementation
//! pass until either the next review approves or the per-Project Review
//! Retry Limit is reached.

use agentic_afk_contracts::{
    IssueAssignmentResponse, PlanRunResponse, ProjectExecutionConfigResponse, ProjectResponse,
};

use crate::coordinator::CoordinatorError;
use crate::plan_run::{ParsedReviewOutput, RefreshedBaseline};

/// Phase name written to `phase_outputs.phase` for review passes.
pub const PHASE_NAME: &str = "review";

/// What the coordinator should do next after one review pass on an Issue
/// Assignment. Pure: this enum is the entire decision surface of the
/// Review Loop.
#[derive(Clone, Debug)]
pub enum ReviewLoopStep {
    /// The Review Phase approved the assignment. The carried JSON value
    /// is the approving review body, used as the merge prompt's
    /// reviewed-evidence input.
    Approved { review_body: serde_json::Value },
    /// The Review Phase rejected with findings; the assignment returns
    /// for another implementation pass with the rendered findings text.
    Retry { findings: String },
    /// The Review Loop has reached the Project's Review Retry Limit and
    /// the assignment must block.
    Block { reason: String },
}

/// Why one review pass could not produce a `ReviewLoopStep`. These are
/// hard phase failures (runner error, unparseable output) rather than the
/// pure rejection/approval outcomes carried by `ReviewLoopStep`.
#[derive(Clone, Debug)]
pub enum ReviewLoopRejection {
    PhaseFailed(String),
    Unparseable(String),
}

impl From<ReviewLoopRejection> for CoordinatorError {
    fn from(rejection: ReviewLoopRejection) -> Self {
        match rejection {
            ReviewLoopRejection::PhaseFailed(error) => CoordinatorError::new(
                500,
                "urn:agentic-afk:review-phase-failed",
                error,
            ),
            ReviewLoopRejection::Unparseable(error) => CoordinatorError::new(
                500,
                "urn:agentic-afk:review-output-unparseable",
                error,
            ),
        }
    }
}

/// Decide what to do after one parsed Review Phase output. The caller is
/// responsible for incrementing the persisted rejection count *before*
/// calling this so the decision sees the post-increment value; this keeps
/// the fn pure (no DB access) while still letting the Review Retry Limit
/// drive the Block outcome.
pub fn decide_review_loop_step(
    parsed: &ParsedReviewOutput,
    rejection_count: i64,
    review_retry_limit: i64,
) -> ReviewLoopStep {
    if parsed.outcome == "approved" {
        return ReviewLoopStep::Approved {
            review_body: parsed.body.clone(),
        };
    }
    if rejection_count >= review_retry_limit {
        return ReviewLoopStep::Block {
            reason: format!(
                "Review Loop exhausted: {rejection_count} rejection(s) reached the Project Review Retry Limit ({review_retry_limit})."
            ),
        };
    }
    ReviewLoopStep::Retry {
        findings: extract_review_findings(&parsed.body),
    }
}

pub fn extract_review_findings(body: &serde_json::Value) -> String {
    body.get("findings")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(|text| format!("- {text}")))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
pub fn render_review_prompt(
    project_instructions: &str,
    project: &ProjectResponse,
    plan_run: &PlanRunResponse,
    baseline: &RefreshedBaseline,
    config: &ProjectExecutionConfigResponse,
    assignment: &IssueAssignmentResponse,
    source_issue_brief: &str,
    impl_body: &serde_json::Value,
) -> String {
    let template = include_str!("../prompts/plan-run/review.md");
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
        .replace(
            "{{IMPLEMENTATION_PHASE_OUTPUT}}",
            &serde_json::to_string_pretty(impl_body).unwrap_or_default(),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parsed(outcome: &str, body: serde_json::Value) -> ParsedReviewOutput {
        ParsedReviewOutput {
            outcome: outcome.to_string(),
            body,
        }
    }

    #[test]
    fn approved_returns_approved_step_with_body() {
        let body = json!({"outcome": "approved", "summary": "ok"});
        let step = decide_review_loop_step(&parsed("approved", body.clone()), 0, 3);
        match step {
            ReviewLoopStep::Approved { review_body } => assert_eq!(review_body, body),
            other => panic!("expected Approved, got {other:?}"),
        }
    }

    #[test]
    fn rejection_under_limit_retries_with_findings() {
        let body = json!({"outcome": "rejected", "findings": ["fix a", "fix b"]});
        let step = decide_review_loop_step(&parsed("rejected", body), 1, 3);
        match step {
            ReviewLoopStep::Retry { findings } => {
                assert!(findings.contains("fix a"));
                assert!(findings.contains("fix b"));
            }
            other => panic!("expected Retry, got {other:?}"),
        }
    }

    #[test]
    fn rejection_at_limit_blocks() {
        // After increment, rejection_count == review_retry_limit triggers
        // the single canonical Review Retry Limit guard. The earlier
        // defensive iteration guard (loop_iteration > limit + 1) was
        // removed because rejection_count >= limit fully covers the same
        // invariant.
        let body = json!({"outcome": "rejected", "findings": []});
        let step = decide_review_loop_step(&parsed("rejected", body), 3, 3);
        match step {
            ReviewLoopStep::Block { reason } => assert!(reason.contains("3")),
            other => panic!("expected Block, got {other:?}"),
        }
    }
}
