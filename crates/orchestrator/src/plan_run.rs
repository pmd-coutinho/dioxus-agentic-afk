//! Plan Run phase dependencies (ADR-0034).
//!
//! The Plan Run coordinator depends on two narrow seams so tests can drive
//! the flow without git or Codex: an [`IntegrationBranchRefresher`] that
//! produces a baseline commit for one Plan Run, and a
//! [`PlanningPhaseRunner`] that executes the Planning Phase prompt and
//! returns raw stdout for the Plan Run coordinator to parse.

use std::path::Path;
use std::sync::Mutex;

/// One refreshed Integration Branch baseline shared by planning and any
/// selected Issue Assignments for one Plan Run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefreshedBaseline {
    pub commit_sha: String,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PlanRunPhaseError {
    #[error("integration branch refresh failed: {0}")]
    Refresh(String),
    #[error("planning phase execution failed: {0}")]
    Planning(String),
}

/// Refresh the configured Integration Branch and report the baseline commit.
pub trait IntegrationBranchRefresher: Send + Sync {
    fn refresh(
        &self,
        project_path: &Path,
        integration_branch: &str,
    ) -> Result<RefreshedBaseline, PlanRunPhaseError>;
}

/// Execute the Planning Phase prompt and return the raw agent stdout for
/// the Plan Run coordinator to parse.
pub trait PlanningPhaseRunner: Send + Sync {
    fn run(&self, prompt: &str) -> Result<String, PlanRunPhaseError>;
}

/// Test refresher that returns a fixed baseline on every call.
pub struct StaticIntegrationBranchRefresher {
    baseline: RefreshedBaseline,
}

impl StaticIntegrationBranchRefresher {
    pub fn new(baseline: RefreshedBaseline) -> Self {
        Self { baseline }
    }
}

impl IntegrationBranchRefresher for StaticIntegrationBranchRefresher {
    fn refresh(
        &self,
        _project_path: &Path,
        _integration_branch: &str,
    ) -> Result<RefreshedBaseline, PlanRunPhaseError> {
        Ok(self.baseline.clone())
    }
}

/// Test planner that returns canned stdout and remembers the last prompt it
/// was given so tests can assert on rendering.
pub struct FakePlanningPhaseRunner {
    stdout: String,
    last_prompt: Mutex<Option<String>>,
}

impl FakePlanningPhaseRunner {
    pub fn with_stdout(stdout: impl Into<String>) -> Self {
        Self {
            stdout: stdout.into(),
            last_prompt: Mutex::new(None),
        }
    }

    pub fn last_prompt(&self) -> Option<String> {
        self.last_prompt.lock().unwrap().clone()
    }
}

impl PlanningPhaseRunner for FakePlanningPhaseRunner {
    fn run(&self, prompt: &str) -> Result<String, PlanRunPhaseError> {
        *self.last_prompt.lock().unwrap() = Some(prompt.to_string());
        Ok(self.stdout.clone())
    }
}

/// Production placeholder that errors until the real git refresh path is
/// implemented (later slices of ADR-0034).
pub struct UnimplementedIntegrationBranchRefresher;

impl IntegrationBranchRefresher for UnimplementedIntegrationBranchRefresher {
    fn refresh(
        &self,
        _project_path: &Path,
        _integration_branch: &str,
    ) -> Result<RefreshedBaseline, PlanRunPhaseError> {
        Err(PlanRunPhaseError::Refresh(
            "real Integration Branch refresh not implemented yet".to_string(),
        ))
    }
}

/// Production placeholder that errors until the real Codex planning path is
/// implemented (later slices of ADR-0034).
pub struct UnimplementedPlanningPhaseRunner;

impl PlanningPhaseRunner for UnimplementedPlanningPhaseRunner {
    fn run(&self, _prompt: &str) -> Result<String, PlanRunPhaseError> {
        Err(PlanRunPhaseError::Planning(
            "real Codex planning runner not implemented yet".to_string(),
        ))
    }
}

/// Extract the JSON body delimited by `<plan>...</plan>` from raw planner
/// stdout and parse the issue selection.
pub fn parse_planning_output(stdout: &str) -> Result<ParsedPlanningOutput, String> {
    let start = stdout
        .find("<plan>")
        .ok_or_else(|| "planning output missing <plan> opening tag".to_string())?
        + "<plan>".len();
    let end = stdout
        .find("</plan>")
        .ok_or_else(|| "planning output missing </plan> closing tag".to_string())?;
    if end < start {
        return Err("planning output has malformed <plan> tags".to_string());
    }
    let body = stdout[start..end].trim();
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| format!("planning output is not valid JSON: {error}"))?;
    let issues = value
        .get("issues")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "planning output is missing `issues` array".to_string())?;
    Ok(ParsedPlanningOutput {
        is_empty: issues.is_empty(),
        body: value,
    })
}

#[derive(Clone, Debug)]
pub struct ParsedPlanningOutput {
    pub is_empty: bool,
    pub body: serde_json::Value,
}
