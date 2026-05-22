//! Merge Phase: prompt rendering, parsed-output classification, and the
//! typed per-Issue-Assignment merge outcome.
//!
//! The Merge Phase runs sequentially across reviewed Issue Assignments so
//! the Integration Branch sees one merge attempt at a time. This module
//! owns the merge prompt, the typed outcome the coordinator collects into
//! a Vec for the Plan Run terminal decision, and the classification fn
//! that turns a parsed merge body into that outcome.

use agentic_afk_contracts::{
    IssueAssignmentResponse, PlanRunResponse, ProjectExecutionConfigResponse, ProjectResponse,
};

use crate::coordinator::CoordinatorError;
use crate::plan_run::{ParsedMergeOutput, RefreshedBaseline};

/// Phase name written to `phase_outputs.phase` for merge passes.
pub const PHASE_NAME: &str = "merge";

/// What happened when the Merge Phase attempted to integrate one Issue
/// Assignment. The coordinator collects these across the reviewed
/// successes so `decide_plan_run_terminal` can finalize the Plan Run.
#[derive(Clone, Debug)]
pub enum AssignmentMergeOutcome {
    /// Merge succeeded locally; the Integration Branch push is deferred
    /// until every reviewed peer in the Plan Run has settled.
    Merged,
    /// The reviewed assignment could not be merged (runner failure,
    /// unparseable output, or explicit `outcome: "blocked"`). The
    /// assignment stays outside the Integration Branch push.
    Blocked { reason: String },
    /// The assignment never reached the Review Phase, so the Merge Phase
    /// never ran. Carried through so the terminal decision can distinguish
    /// "reviewed but failed to merge" from "blocked before review."
    NotAttempted,
}

/// Hard failure surface for parsing a merge stdout. Distinct from
/// `AssignmentMergeOutcome::Blocked` — these are coordinator-level errors,
/// not assignment-level outcomes.
#[derive(Clone, Debug)]
pub enum MergeRejection {
    Unparseable(String),
}

impl From<MergeRejection> for CoordinatorError {
    fn from(rejection: MergeRejection) -> Self {
        match rejection {
            MergeRejection::Unparseable(error) => CoordinatorError::new(
                500,
                "urn:agentic-afk:merge-output-unparseable",
                error,
            ),
        }
    }
}

/// Classify a parsed merge body into a per-assignment outcome. The
/// `outcome == "blocked"` path pulls the block reason from the merge
/// body's `block_reason` field, falling back to `summary`, falling back
/// to a fixed message. Pure: no I/O.
pub fn decide_merge_outcome(parsed: &ParsedMergeOutput) -> AssignmentMergeOutcome {
    if parsed.outcome != "blocked" {
        return AssignmentMergeOutcome::Merged;
    }
    let reason = parsed
        .body
        .get("block_reason")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            parsed
                .body
                .get("summary")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| "Merge Phase blocked without an explicit reason".to_string())
        });
    AssignmentMergeOutcome::Blocked { reason }
}

#[allow(clippy::too_many_arguments)]
pub fn render_merge_prompt(
    project_instructions: &str,
    project: &ProjectResponse,
    plan_run: &PlanRunResponse,
    baseline: &RefreshedBaseline,
    config: &ProjectExecutionConfigResponse,
    assignment: &IssueAssignmentResponse,
    review_body: &serde_json::Value,
) -> String {
    let template = include_str!("../prompts/plan-run/merge.md");
    let selection = assignment
        .selection_summary
        .clone()
        .unwrap_or_else(|| "(no selection summary)".to_string());
    template
        .replace("{{PROJECT_INSTRUCTIONS}}", project_instructions)
        .replace("{{PROJECT_NAME}}", &project.path)
        .replace("{{PLAN_RUN_ID}}", &plan_run.id)
        .replace("{{PLAN_RUN_BASELINE}}", &baseline.commit_sha)
        .replace("{{INTEGRATION_BRANCH}}", &config.integration_branch)
        .replace("{{SOURCE_ISSUE_ID}}", &assignment.source_id)
        .replace("{{SOURCE_ISSUE_TITLE}}", &assignment.source_title)
        .replace("{{ISSUE_BRANCH}}", &assignment.branch)
        .replace("{{SELECTION_SUMMARY}}", &selection)
        .replace(
            "{{REVIEW_PHASE_OUTPUT}}",
            &serde_json::to_string_pretty(review_body).unwrap_or_default(),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parsed(outcome: &str, body: serde_json::Value) -> ParsedMergeOutput {
        ParsedMergeOutput {
            outcome: outcome.to_string(),
            body,
        }
    }

    #[test]
    fn merged_outcome_classifies_merged() {
        let p = parsed("merged", json!({"summary": "ok"}));
        assert!(matches!(decide_merge_outcome(&p), AssignmentMergeOutcome::Merged));
    }

    #[test]
    fn blocked_with_explicit_reason_uses_block_reason_field() {
        let p = parsed("blocked", json!({"block_reason": "conflict in foo.rs"}));
        match decide_merge_outcome(&p) {
            AssignmentMergeOutcome::Blocked { reason } => {
                assert_eq!(reason, "conflict in foo.rs");
            }
            other => panic!("expected Blocked, got {other:?}"),
        }
    }

    #[test]
    fn blocked_without_block_reason_falls_back_to_summary() {
        let p = parsed("blocked", json!({"summary": "tests failed"}));
        match decide_merge_outcome(&p) {
            AssignmentMergeOutcome::Blocked { reason } => {
                assert_eq!(reason, "tests failed");
            }
            other => panic!("expected Blocked, got {other:?}"),
        }
    }

    #[test]
    fn blocked_with_neither_field_uses_fallback_reason() {
        let p = parsed("blocked", json!({}));
        match decide_merge_outcome(&p) {
            AssignmentMergeOutcome::Blocked { reason } => {
                assert!(reason.contains("without an explicit reason"));
            }
            other => panic!("expected Blocked, got {other:?}"),
        }
    }
}
