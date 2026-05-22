//! Planning Phase: prompt rendering, output parsing, and the pure
//! `validate_planner_selection` decision.
//!
//! The Plan Run coordinator imports this module to render the planner
//! prompt against the cached Project instructions and the eligible Source
//! Issue snapshot, parse the planner stdout, and validate the planner's
//! choices against the eligible set, Max Parallel Tasks, and the Project's
//! enabled Issue Source. Validation returns `Vec<PlannedClaim>` ready for
//! worktree provisioning, or a typed `PlanningRejection` the coordinator
//! maps to an RFC-7807 problem response.

use agentic_afk_contracts::{
    ProjectExecutionConfigResponse, ProjectResponse, SourceIssueSnapshot,
};

use crate::coordinator::CoordinatorError;
use crate::plan_run::{
    ParsedPlanningOutput, PlannerSelection, RefreshedBaseline, extract_planner_selections,
};

/// One validated planner choice, paired with the eligible Source Issue it
/// targets. A `PlannedClaim` is ready for Assignment Worktree provisioning
/// and Issue Assignment creation. Ineligible or capacity-exceeding planner
/// output never becomes a `PlannedClaim`.
///
/// See CONTEXT.md → Planned Claim.
#[derive(Clone, Debug)]
pub struct PlannedClaim {
    pub selection: PlannerSelection,
    pub eligible_issue: SourceIssueSnapshot,
}

/// Why the Planning Phase's selection was rejected. Mapped to an
/// RFC-7807 problem-type URN at the HTTP boundary.
#[derive(Clone, Debug)]
pub enum PlanningRejection {
    /// `extract_planner_selections` could not turn the parsed planning
    /// body into a list of selections (missing fields, wrong types, etc).
    Unparseable(String),
    /// The planner returned more selections than the Project's Max
    /// Parallel Tasks permits. Forces the planner to converge rather than
    /// implicitly truncating.
    ExceedsMaxParallel { got: usize, cap: usize },
    /// A planner selection referenced a `source_issue_id` that is not in
    /// the eligible snapshot.
    IneligibleSelection { source_id: String },
    /// The Project has no enabled Issue Source. Without an Issue Source
    /// the Control Plane cannot write the Claimed lifecycle back, so the
    /// Plan Run cannot proceed past Planning.
    MissingIssueSource,
}

impl From<PlanningRejection> for CoordinatorError {
    fn from(rejection: PlanningRejection) -> Self {
        match rejection {
            PlanningRejection::Unparseable(error) => CoordinatorError::new(
                500,
                "urn:agentic-afk:planning-output-unparseable",
                error,
            ),
            PlanningRejection::ExceedsMaxParallel { got, cap } => CoordinatorError::new(
                500,
                "urn:agentic-afk:planning-exceeds-max-parallel",
                format!(
                    "Planning Phase returned {got} issues but Project Max Parallel Tasks is {cap}"
                ),
            ),
            PlanningRejection::IneligibleSelection { source_id } => CoordinatorError::new(
                500,
                "urn:agentic-afk:planning-selection-ineligible",
                format!(
                    "Planning Phase selected Source Issue {source_id} which is not in the eligible set"
                ),
            ),
            PlanningRejection::MissingIssueSource => CoordinatorError::new(
                500,
                "urn:agentic-afk:issue-source-missing",
                "Project has no enabled Issue Source for claim write-back",
            ),
        }
    }
}

/// Pair planner selections with their eligible Source Issue snapshots,
/// enforcing Max Parallel Tasks and the Project's enabled Issue Source
/// invariant. Pure: no I/O, no async.
pub fn validate_planner_selection(
    parsed: &ParsedPlanningOutput,
    eligible: &[SourceIssueSnapshot],
    max_parallel_tasks: i64,
    has_enabled_issue_source: bool,
) -> Result<Vec<PlannedClaim>, PlanningRejection> {
    let selections = extract_planner_selections(parsed).map_err(PlanningRejection::Unparseable)?;

    let cap = max_parallel_tasks.max(1) as usize;
    if selections.len() > cap {
        return Err(PlanningRejection::ExceedsMaxParallel {
            got: selections.len(),
            cap,
        });
    }

    if !has_enabled_issue_source {
        return Err(PlanningRejection::MissingIssueSource);
    }

    let by_id: std::collections::HashMap<&str, &SourceIssueSnapshot> = eligible
        .iter()
        .map(|issue| (issue.source_id.as_str(), issue))
        .collect();

    selections
        .into_iter()
        .map(|selection| {
            let eligible_issue = by_id
                .get(selection.source_issue_id.as_str())
                .copied()
                .cloned()
                .ok_or_else(|| PlanningRejection::IneligibleSelection {
                    source_id: selection.source_issue_id.clone(),
                })?;
            Ok(PlannedClaim {
                selection,
                eligible_issue,
            })
        })
        .collect()
}

pub fn render_planning_prompt(
    project_instructions: &str,
    project: &ProjectResponse,
    config: &ProjectExecutionConfigResponse,
    baseline: &RefreshedBaseline,
    eligible: &[SourceIssueSnapshot],
) -> String {
    let template = include_str!("../prompts/plan-run/plan.md");
    template
        .replace("{{PROJECT_INSTRUCTIONS}}", project_instructions)
        .replace("{{PROJECT_NAME}}", &project.path)
        .replace("{{INTEGRATION_BRANCH}}", &config.integration_branch)
        .replace("{{PLAN_RUN_BASELINE}}", &baseline.commit_sha)
        .replace(
            "{{MAX_PARALLEL_TASKS}}",
            &config.max_parallel_tasks.to_string(),
        )
        .replace(
            "{{ELIGIBLE_SOURCE_ISSUES}}",
            &render_eligible_source_issues(eligible),
        )
}

fn render_eligible_source_issues(eligible: &[SourceIssueSnapshot]) -> String {
    if eligible.is_empty() {
        return "(no eligible Source Issues)".to_string();
    }
    eligible
        .iter()
        .map(|issue| {
            format!(
                "- source_id: {}\n  title: {}\n  raw:\n{}",
                issue.source_id,
                issue.title,
                indent_lines(&issue.raw_text, 4)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn indent_lines(text: &str, spaces: usize) -> String {
    let pad = " ".repeat(spaces);
    text.lines()
        .map(|line| format!("{pad}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan_run::parse_planning_output;

    fn snapshot(source_id: &str) -> SourceIssueSnapshot {
        SourceIssueSnapshot {
            source_id: source_id.to_string(),
            title: format!("title-{source_id}"),
            readiness: "ready".to_string(),
            lifecycle_status: "ready".to_string(),
            parent_issue: None,
            issue_dependencies: Vec::new(),
            source_order: 0,
            raw_text: String::new(),
        }
    }

    fn parsed_with(selections_json: &str) -> ParsedPlanningOutput {
        let stdout = format!("<plan>{{\"issues\":{selections_json}}}</plan>");
        parse_planning_output(&stdout).expect("test planning stdout parses")
    }

    fn selection_object(source_id: &str) -> String {
        format!(
            r#"{{"source_issue_id":"{source_id}","title":"t","branch":"b/{source_id}","selection_summary":"s"}}"#
        )
    }

    #[test]
    fn pairs_selections_with_eligible_snapshots() {
        let parsed = parsed_with(&format!("[{}]", selection_object("42")));
        let eligible = vec![snapshot("42"), snapshot("99")];
        let claims = validate_planner_selection(&parsed, &eligible, 2, true)
            .expect("validation succeeds");
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].selection.source_issue_id, "42");
        assert_eq!(claims[0].eligible_issue.source_id, "42");
    }

    #[test]
    fn rejects_ineligible_selection() {
        let parsed = parsed_with(&format!("[{}]", selection_object("missing")));
        let eligible = vec![snapshot("42")];
        let result = validate_planner_selection(&parsed, &eligible, 2, true);
        assert!(matches!(
            result,
            Err(PlanningRejection::IneligibleSelection { source_id }) if source_id == "missing"
        ));
    }

    #[test]
    fn rejects_when_planner_exceeds_max_parallel() {
        let parsed = parsed_with(&format!(
            "[{}, {}, {}]",
            selection_object("a"),
            selection_object("b"),
            selection_object("c"),
        ));
        let eligible = vec![snapshot("a"), snapshot("b"), snapshot("c")];
        let result = validate_planner_selection(&parsed, &eligible, 2, true);
        assert!(matches!(
            result,
            Err(PlanningRejection::ExceedsMaxParallel { got: 3, cap: 2 })
        ));
    }

    #[test]
    fn rejects_when_no_enabled_issue_source() {
        let parsed = parsed_with(&format!("[{}]", selection_object("42")));
        let eligible = vec![snapshot("42")];
        let result = validate_planner_selection(&parsed, &eligible, 2, false);
        assert!(matches!(result, Err(PlanningRejection::MissingIssueSource)));
    }
}
