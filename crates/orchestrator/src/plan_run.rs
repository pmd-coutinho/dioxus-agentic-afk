//! Plan Run phase dependencies (ADR-0034).
//!
//! The Plan Run coordinator depends on two narrow seams so tests can drive
//! the flow without git or Codex: an [`IntegrationBranchRefresher`] that
//! produces a baseline commit for one Plan Run, and a
//! [`PlanningPhaseRunner`] that executes the Planning Phase prompt and
//! returns raw stdout for the Plan Run coordinator to parse.

use std::path::{Path, PathBuf};
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
    #[error("assignment worktree provisioning failed: {0}")]
    WorktreeProvision(String),
    #[error("issue source lifecycle write-back failed: {0}")]
    LifecycleWrite(String),
    #[error("implementation phase execution failed: {0}")]
    Implementation(String),
    #[error("review phase execution failed: {0}")]
    Review(String),
    #[error("merge phase execution failed: {0}")]
    Merge(String),
    #[error("integration branch push failed: {0}")]
    IntegrationPush(String),
    #[error("integration branch push rejected as non-fast-forward: {stderr}")]
    NonFastForward { stderr: String },
    #[error("assignment worktree cleanup failed: {0}")]
    Cleanup(String),
}

/// Per-assignment context passed to Implementation, Review, and Merge
/// phase runners. Production `DockerCodexRunner` reads
/// `assignment.worktree_path` to bind-mount the worktree read-write into
/// the Codex Sandbox, and uses the IDs to populate the
/// `agentic-afk.*` Docker labels.
pub struct AssignmentContext<'a> {
    pub project: &'a agentic_afk_contracts::ProjectResponse,
    pub plan_run: &'a agentic_afk_contracts::PlanRunResponse,
    pub assignment: &'a agentic_afk_contracts::IssueAssignmentResponse,
    pub process_recorder: Option<&'a crate::in_flight_phase_tracker::PhaseProcessRecorder>,
}

/// Plan-Run-scoped context passed to the Planning Phase runner.
/// Production `DockerCodexRunner` uses the IDs to populate the
/// `agentic-afk.*` Docker labels on the Planning Phase Codex Sandbox.
pub struct PlanningContext<'a> {
    pub project: &'a agentic_afk_contracts::ProjectResponse,
    pub plan_run: &'a agentic_afk_contracts::PlanRunResponse,
    pub process_recorder: Option<&'a crate::in_flight_phase_tracker::PhaseProcessRecorder>,
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
    fn run(&self, prompt: &str, context: &PlanningContext<'_>)
    -> Result<String, PlanRunPhaseError>;
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
    fn run(
        &self,
        prompt: &str,
        _context: &PlanningContext<'_>,
    ) -> Result<String, PlanRunPhaseError> {
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

/// Execute an Implementation Phase prompt and return raw agent stdout for
/// the Plan Run coordinator to parse.
pub trait ImplementationPhaseRunner: Send + Sync {
    fn run(
        &self,
        prompt: &str,
        context: &AssignmentContext<'_>,
    ) -> Result<String, PlanRunPhaseError>;
}

/// Execute a Review Phase prompt and return raw agent stdout.
pub trait ReviewPhaseRunner: Send + Sync {
    fn run(
        &self,
        prompt: &str,
        context: &AssignmentContext<'_>,
    ) -> Result<String, PlanRunPhaseError>;
}

/// Test runner that returns canned stdout. A single stdout repeats on each
/// call; pass multiple stdouts via `with_stdouts` to return successive
/// values (the last one repeats after the queue is exhausted), which the
/// Review Loop tests rely on (issue #44).
pub struct FakeImplementationPhaseRunner {
    stdouts: Mutex<Vec<String>>,
    prompts: Mutex<Vec<String>>,
}

impl FakeImplementationPhaseRunner {
    pub fn with_stdout(stdout: impl Into<String>) -> Self {
        Self::with_stdouts(vec![stdout.into()])
    }

    pub fn with_stdouts<S: Into<String>>(stdouts: impl IntoIterator<Item = S>) -> Self {
        let stdouts: Vec<String> = stdouts.into_iter().map(Into::into).collect();
        assert!(
            !stdouts.is_empty(),
            "FakeImplementationPhaseRunner needs at least one stdout"
        );
        Self {
            stdouts: Mutex::new(stdouts),
            prompts: Mutex::new(Vec::new()),
        }
    }

    pub fn last_prompt(&self) -> Option<String> {
        self.prompts.lock().unwrap().last().cloned()
    }

    pub fn prompts(&self) -> Vec<String> {
        self.prompts.lock().unwrap().clone()
    }

    pub fn call_count(&self) -> usize {
        self.prompts.lock().unwrap().len()
    }
}

impl ImplementationPhaseRunner for FakeImplementationPhaseRunner {
    fn run(
        &self,
        prompt: &str,
        _context: &AssignmentContext<'_>,
    ) -> Result<String, PlanRunPhaseError> {
        self.prompts.lock().unwrap().push(prompt.to_string());
        let mut queue = self.stdouts.lock().unwrap();
        let stdout = if queue.len() == 1 {
            queue[0].clone()
        } else {
            queue.remove(0)
        };
        Ok(stdout)
    }
}

pub struct FakeReviewPhaseRunner {
    stdouts: Mutex<Vec<String>>,
    prompts: Mutex<Vec<String>>,
}

impl FakeReviewPhaseRunner {
    pub fn with_stdout(stdout: impl Into<String>) -> Self {
        Self::with_stdouts(vec![stdout.into()])
    }

    pub fn with_stdouts<S: Into<String>>(stdouts: impl IntoIterator<Item = S>) -> Self {
        let stdouts: Vec<String> = stdouts.into_iter().map(Into::into).collect();
        assert!(
            !stdouts.is_empty(),
            "FakeReviewPhaseRunner needs at least one stdout"
        );
        Self {
            stdouts: Mutex::new(stdouts),
            prompts: Mutex::new(Vec::new()),
        }
    }

    pub fn last_prompt(&self) -> Option<String> {
        self.prompts.lock().unwrap().last().cloned()
    }

    pub fn prompts(&self) -> Vec<String> {
        self.prompts.lock().unwrap().clone()
    }

    pub fn call_count(&self) -> usize {
        self.prompts.lock().unwrap().len()
    }
}

impl ReviewPhaseRunner for FakeReviewPhaseRunner {
    fn run(
        &self,
        prompt: &str,
        _context: &AssignmentContext<'_>,
    ) -> Result<String, PlanRunPhaseError> {
        self.prompts.lock().unwrap().push(prompt.to_string());
        let mut queue = self.stdouts.lock().unwrap();
        let stdout = if queue.len() == 1 {
            queue[0].clone()
        } else {
            queue.remove(0)
        };
        Ok(stdout)
    }
}

pub struct UnimplementedImplementationPhaseRunner;

impl ImplementationPhaseRunner for UnimplementedImplementationPhaseRunner {
    fn run(
        &self,
        _prompt: &str,
        _context: &AssignmentContext<'_>,
    ) -> Result<String, PlanRunPhaseError> {
        Err(PlanRunPhaseError::Implementation(
            "implementation runner not wired".to_string(),
        ))
    }
}

pub struct UnimplementedReviewPhaseRunner;

impl ReviewPhaseRunner for UnimplementedReviewPhaseRunner {
    fn run(
        &self,
        _prompt: &str,
        _context: &AssignmentContext<'_>,
    ) -> Result<String, PlanRunPhaseError> {
        Err(PlanRunPhaseError::Review(
            "review runner not wired".to_string(),
        ))
    }
}

/// Parse the JSON body wrapped in `<impl>...</impl>` returned by the
/// Implementation Phase agent.
pub fn parse_implementation_output(stdout: &str) -> Result<ParsedImplementationOutput, String> {
    let body = extract_tagged_json(stdout, "impl")?;
    let outcome = body
        .get("outcome")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "implementation output missing `outcome` string".to_string())?;
    if !matches!(outcome, "ready_for_review" | "blocked" | "failed") {
        return Err(format!(
            "implementation output `outcome` must be one of ready_for_review|blocked|failed, got {outcome}"
        ));
    }
    Ok(ParsedImplementationOutput {
        outcome: outcome.to_string(),
        body,
    })
}

/// Parse the JSON body wrapped in `<review>...</review>` returned by the
/// Review Phase agent.
pub fn parse_review_output(stdout: &str) -> Result<ParsedReviewOutput, String> {
    let body = extract_tagged_json(stdout, "review")?;
    let outcome = body
        .get("outcome")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "review output missing `outcome` string".to_string())?;
    if !matches!(outcome, "approved" | "rejected") {
        return Err(format!(
            "review output `outcome` must be approved|rejected, got {outcome}"
        ));
    }
    Ok(ParsedReviewOutput {
        outcome: outcome.to_string(),
        body,
    })
}

fn extract_tagged_json(stdout: &str, tag: &str) -> Result<serde_json::Value, String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let body = if let Some(end) = stdout.rfind(&close) {
        let start = stdout[..end]
            .rfind(&open)
            .ok_or_else(|| format!("output missing {open} opening tag"))?
            + open.len();
        if end < start {
            return Err(format!("output has malformed {tag} tags"));
        }
        strip_json_code_fence(stdout[start..end].trim())
    } else {
        let start = stdout
            .rfind(&open)
            .ok_or_else(|| format!("output missing {open} opening tag"))?
            + open.len();
        strip_json_code_fence(stdout[start..].trim())
    };
    serde_json::from_str(body).map_err(|error| {
        format!(
            "{tag} output is not valid JSON: {error}; body starts with: {}",
            excerpt_for_error(body)
        )
    })
}

#[cfg(test)]
mod parse_tagged_phase_output_tests {
    use super::*;

    #[test]
    fn implementation_output_parses_without_closing_tag_when_json_is_complete() {
        let parsed =
            parse_implementation_output(r#"<impl>{"outcome":"ready_for_review","summary":"done"}"#)
                .expect("implementation output without closing tag parses");

        assert_eq!(parsed.outcome, "ready_for_review");
        assert_eq!(parsed.body["summary"], "done");
    }

    #[test]
    fn review_output_parses_without_closing_tag_when_json_is_complete() {
        let parsed = parse_review_output(r#"<review>{"outcome":"approved","summary":"ok"}"#)
            .expect("review output without closing tag parses");

        assert_eq!(parsed.outcome, "approved");
        assert_eq!(parsed.body["summary"], "ok");
    }

    #[test]
    fn merge_output_parses_without_closing_tag_when_json_is_complete() {
        let parsed = parse_merge_output(r#"<merge>{"outcome":"merged","summary":"ok"}"#)
            .expect("merge output without closing tag parses");

        assert_eq!(parsed.outcome, "merged");
        assert_eq!(parsed.body["summary"], "ok");
    }

    #[test]
    fn tagged_phase_output_still_rejects_missing_opening_tag() {
        let error = parse_implementation_output(r#"{"outcome":"ready_for_review"}"#).unwrap_err();

        assert!(
            error.contains("output missing <impl> opening tag"),
            "{error}"
        );
    }
}

fn strip_json_code_fence(body: &str) -> &str {
    let after_open = if let Some(after_open) = body.strip_prefix("```json") {
        after_open
    } else if let Some(after_open) = body.strip_prefix("```") {
        after_open
    } else {
        return body;
    };
    let after_open = after_open.trim_start();
    let Some(before_close) = after_open.strip_suffix("```") else {
        return body;
    };
    before_close.trim()
}

fn excerpt_for_error(body: &str) -> String {
    const LIMIT: usize = 240;
    let normalized = body
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>();
    let mut chars = normalized.chars();
    let excerpt = chars.by_ref().take(LIMIT).collect::<String>();
    if chars.next().is_none() {
        normalized
    } else {
        format!("{excerpt}...")
    }
}

#[derive(Clone, Debug)]
pub struct ParsedImplementationOutput {
    pub outcome: String,
    pub body: serde_json::Value,
}

#[derive(Clone, Debug)]
pub struct ParsedReviewOutput {
    pub outcome: String,
    pub body: serde_json::Value,
}

/// Production placeholder that errors until the real Codex planning path is
/// implemented (later slices of ADR-0034).
pub struct UnimplementedPlanningPhaseRunner;

impl PlanningPhaseRunner for UnimplementedPlanningPhaseRunner {
    fn run(
        &self,
        _prompt: &str,
        _context: &PlanningContext<'_>,
    ) -> Result<String, PlanRunPhaseError> {
        Err(PlanRunPhaseError::Planning(
            "real Codex planning runner not implemented yet".to_string(),
        ))
    }
}

/// Extract the JSON body delimited by `<plan>...</plan>` from raw planner
/// stdout and parse the issue selection.
pub fn parse_planning_output(stdout: &str) -> Result<ParsedPlanningOutput, String> {
    let body = extract_planning_body(stdout)?;
    let value: serde_json::Value = serde_json::from_str(body).map_err(|error| {
        format!(
            "planning output is not valid JSON: {error}; body starts with: {}",
            excerpt_for_error(body)
        )
    })?;
    let issues = value
        .get("issues")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "planning output is missing `issues` array".to_string())?;
    Ok(ParsedPlanningOutput {
        is_empty: issues.is_empty(),
        body: value,
    })
}

fn extract_planning_body(stdout: &str) -> Result<&str, String> {
    if let Some(end) = stdout.rfind("</plan>") {
        let start = stdout[..end].rfind("<plan>").ok_or_else(|| {
            format!(
                "planning output missing <plan> opening tag; output ends with: {}",
                excerpt_for_error(stdout)
            )
        })? + "<plan>".len();
        if end < start {
            return Err("planning output has malformed <plan> tags".to_string());
        }
        return Ok(strip_json_code_fence(stdout[start..end].trim()));
    }

    let start = stdout.rfind("<plan>").ok_or_else(|| {
        format!(
            "planning output missing <plan> opening tag; output ends with: {}",
            excerpt_for_error(stdout)
        )
    })? + "<plan>".len();
    Ok(strip_json_code_fence(stdout[start..].trim()))
}

#[derive(Clone, Debug)]
pub struct ParsedPlanningOutput {
    pub is_empty: bool,
    pub body: serde_json::Value,
}

/// One issue picked by the Planning Phase, parsed out of the planner output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannerSelection {
    pub source_issue_id: String,
    pub title: String,
    pub branch: String,
    pub selection_summary: String,
}

/// Extract the selected issues from a parsed planning output. Returns an
/// empty vector for the empty-selection success path.
pub fn extract_planner_selections(
    parsed: &ParsedPlanningOutput,
) -> Result<Vec<PlannerSelection>, String> {
    let issues = parsed
        .body
        .get("issues")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "planning output is missing `issues` array".to_string())?;
    issues
        .iter()
        .map(|issue| {
            let source_issue_id = string_field(issue, "source_issue_id")?;
            let title = string_field(issue, "title")?;
            let branch = string_field(issue, "branch")?;
            let selection_summary = string_field(issue, "selection_summary")?;
            Ok(PlannerSelection {
                source_issue_id,
                title,
                branch,
                selection_summary,
            })
        })
        .collect()
}

fn string_field(value: &serde_json::Value, key: &str) -> Result<String, String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("planning output issue is missing `{key}` string"))
}

/// Create an Assignment Worktree from a Plan Run baseline commit. Production
/// drives Worktrunk; tests inject a fake.
pub trait AssignmentWorktreeProvisioner: Send + Sync {
    fn provision(
        &self,
        project_path: &Path,
        baseline_commit: &str,
        branch: &str,
    ) -> Result<PathBuf, PlanRunPhaseError>;
}

/// The five canonical Lifecycle Status values written back to the
/// **Issue Source** for one **Source Issue**. Mirrors CONTEXT.md →
/// Lifecycle Status. The `as_str` discriminator is what production
/// adapters (label suffixes, markdown line text) emit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleStatus {
    Ready,
    Claimed,
    Running,
    Blocked,
    Completed,
}

impl LifecycleStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Claimed => "claimed",
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::Completed => "completed",
        }
    }
}

/// Write a Lifecycle Status for one Source Issue back to the upstream
/// Issue Source. Production drives `gh` or a local markdown file; tests
/// inject a fake. The fatality of a failure depends on the calling site:
/// the Claimed transition is a correctness invariant per ADR-0035, and
/// every later transition is best-effort and recorded as Activity.
pub trait IssueLifecycleWriter: Send + Sync {
    fn write(&self, source_id: &str, status: LifecycleStatus) -> Result<(), PlanRunPhaseError>;
}

/// Test provisioner that records its calls and returns a fixed worktree
/// path per call.
pub struct FakeWorktreeProvisioner {
    base: PathBuf,
    calls: Mutex<Vec<(PathBuf, String, String)>>,
}

impl FakeWorktreeProvisioner {
    pub fn new(base: impl Into<PathBuf>) -> Self {
        Self {
            base: base.into(),
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn calls(&self) -> Vec<(PathBuf, String, String)> {
        self.calls.lock().unwrap().clone()
    }
}

impl AssignmentWorktreeProvisioner for FakeWorktreeProvisioner {
    fn provision(
        &self,
        project_path: &Path,
        baseline_commit: &str,
        branch: &str,
    ) -> Result<PathBuf, PlanRunPhaseError> {
        self.calls.lock().unwrap().push((
            project_path.to_path_buf(),
            baseline_commit.to_string(),
            branch.to_string(),
        ));
        Ok(self.base.join(branch))
    }
}

/// Test lifecycle writer that records each write and optionally errors
/// every call.
pub struct FakeLifecycleWriter {
    calls: Mutex<Vec<(String, LifecycleStatus)>>,
    error: Option<String>,
}

impl FakeLifecycleWriter {
    pub fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            error: None,
        }
    }

    pub fn failing(error: impl Into<String>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            error: Some(error.into()),
        }
    }

    /// Every recorded write, including the lifecycle status. Tests that
    /// only care about Source Issue identity can map over `.0`.
    pub fn calls(&self) -> Vec<(String, LifecycleStatus)> {
        self.calls.lock().unwrap().clone()
    }
}

impl Default for FakeLifecycleWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl IssueLifecycleWriter for FakeLifecycleWriter {
    fn write(&self, source_id: &str, status: LifecycleStatus) -> Result<(), PlanRunPhaseError> {
        self.calls
            .lock()
            .unwrap()
            .push((source_id.to_string(), status));
        if let Some(error) = &self.error {
            Err(PlanRunPhaseError::LifecycleWrite(error.clone()))
        } else {
            Ok(())
        }
    }
}

/// Production placeholder used until a real worktrunk + project context is
/// wired in.
pub struct UnimplementedWorktreeProvisioner;

impl AssignmentWorktreeProvisioner for UnimplementedWorktreeProvisioner {
    fn provision(
        &self,
        _project_path: &Path,
        _baseline_commit: &str,
        _branch: &str,
    ) -> Result<PathBuf, PlanRunPhaseError> {
        Err(PlanRunPhaseError::WorktreeProvision(
            "real Assignment Worktree provisioner not implemented yet".to_string(),
        ))
    }
}

/// Production placeholder used until the real `gh` lifecycle writer is
/// wired in.
pub struct UnimplementedLifecycleWriter;

impl IssueLifecycleWriter for UnimplementedLifecycleWriter {
    fn write(&self, _source_id: &str, _status: LifecycleStatus) -> Result<(), PlanRunPhaseError> {
        Err(PlanRunPhaseError::LifecycleWrite(
            "real Issue Source lifecycle writer not implemented yet".to_string(),
        ))
    }
}

// --- Merge Phase (issue #45) ---

/// Execute a Merge Phase prompt and return raw agent stdout for the Plan
/// Run coordinator to parse. The merger integrates one reviewed Issue
/// Assignment's branch into the configured Integration Branch.
pub trait MergePhaseRunner: Send + Sync {
    fn run(
        &self,
        prompt: &str,
        context: &AssignmentContext<'_>,
    ) -> Result<String, PlanRunPhaseError>;
}

/// Push the verified Integration Branch upstream. Production drives
/// `git push`; tests inject a fake so the Integration Branch push
/// boundary can be asserted without touching real git remotes.
pub trait IntegrationBranchPusher: Send + Sync {
    fn push(&self, project_path: &Path, integration_branch: &str) -> Result<(), PlanRunPhaseError>;
}

/// Clean up the Assignment Worktree and deterministic branch for one
/// merged Issue Assignment. Best-effort; failures are surfaced to the
/// caller for activity logging but do not roll back the merge.
pub trait AssignmentWorktreeCleaner: Send + Sync {
    fn cleanup(
        &self,
        project_path: &Path,
        worktree_path: &Path,
        branch: &str,
    ) -> Result<(), PlanRunPhaseError>;
}

/// Test merger that returns canned stdout. Like the implementation/review
/// fakes, a queue is supported for tests that drive multiple successive
/// merge attempts.
pub struct FakeMergePhaseRunner {
    stdouts: Mutex<Vec<String>>,
    prompts: Mutex<Vec<String>>,
}

impl FakeMergePhaseRunner {
    pub fn with_stdout(stdout: impl Into<String>) -> Self {
        Self::with_stdouts(vec![stdout.into()])
    }

    pub fn with_stdouts<S: Into<String>>(stdouts: impl IntoIterator<Item = S>) -> Self {
        let stdouts: Vec<String> = stdouts.into_iter().map(Into::into).collect();
        assert!(
            !stdouts.is_empty(),
            "FakeMergePhaseRunner needs at least one stdout"
        );
        Self {
            stdouts: Mutex::new(stdouts),
            prompts: Mutex::new(Vec::new()),
        }
    }

    pub fn last_prompt(&self) -> Option<String> {
        self.prompts.lock().unwrap().last().cloned()
    }

    pub fn prompts(&self) -> Vec<String> {
        self.prompts.lock().unwrap().clone()
    }

    pub fn call_count(&self) -> usize {
        self.prompts.lock().unwrap().len()
    }
}

impl MergePhaseRunner for FakeMergePhaseRunner {
    fn run(
        &self,
        prompt: &str,
        _context: &AssignmentContext<'_>,
    ) -> Result<String, PlanRunPhaseError> {
        self.prompts.lock().unwrap().push(prompt.to_string());
        let mut queue = self.stdouts.lock().unwrap();
        let stdout = if queue.len() == 1 {
            queue[0].clone()
        } else {
            queue.remove(0)
        };
        Ok(stdout)
    }
}

pub struct UnimplementedMergePhaseRunner;

impl MergePhaseRunner for UnimplementedMergePhaseRunner {
    fn run(
        &self,
        _prompt: &str,
        _context: &AssignmentContext<'_>,
    ) -> Result<String, PlanRunPhaseError> {
        Err(PlanRunPhaseError::Merge(
            "merge runner not wired".to_string(),
        ))
    }
}

/// Scripted outcomes consumed by [`FakeIntegrationBranchPusher`] in FIFO
/// order, one per `push` call. Mirrors the `PushOutcome` taxonomy from
/// [`crate::push_attempt`] but is its own type because the fake harness
/// needs `Transient` (mapping to [`PlanRunPhaseError::IntegrationPush`])
/// separately from any classifier semantics: the fake is the boundary
/// that *produces* errors, not one that classifies them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FakePushOutcome {
    /// Push completed successfully.
    Success,
    /// Push failed with a transient/unclassified error: maps to
    /// [`PlanRunPhaseError::IntegrationPush`] so the orchestrator's
    /// classifier sees a generic-error payload.
    Transient { stderr: String },
    /// Push was rejected by the remote because the Integration Branch
    /// has diverged: maps to [`PlanRunPhaseError::NonFastForward`].
    NonFastForward { stderr: String },
}

/// Test pusher that records each push call. Used to assert that the
/// Integration Branch is only pushed after a verified merge outcome.
///
/// Two construction modes:
/// - Single-mode ([`Self::new`] / [`Self::failing`]): repeats the same
///   outcome for every call. Kept for unmigrated tests.
/// - Scriptable ([`Self::with_outcomes`]): consumes a `Vec<FakePushOutcome>`
///   FIFO, one entry per push. Over-consuming panics so tests catch
///   accidental extra pushes.
pub struct FakeIntegrationBranchPusher {
    calls: Mutex<Vec<(PathBuf, String)>>,
    error: Option<String>,
    outcomes: Option<Mutex<Vec<FakePushOutcome>>>,
}

impl FakeIntegrationBranchPusher {
    pub fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            error: None,
            outcomes: None,
        }
    }

    pub fn failing(error: impl Into<String>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            error: Some(error.into()),
            outcomes: None,
        }
    }

    /// Scripted constructor: outcomes are consumed FIFO, one per `push`
    /// call. Pushing more times than the script has entries panics with a
    /// clear message so tests catch unexpected extra pushes immediately.
    pub fn with_outcomes(outcomes: Vec<FakePushOutcome>) -> Self {
        assert!(
            !outcomes.is_empty(),
            "FakeIntegrationBranchPusher::with_outcomes needs at least one outcome"
        );
        Self {
            calls: Mutex::new(Vec::new()),
            error: None,
            outcomes: Some(Mutex::new(outcomes)),
        }
    }

    pub fn calls(&self) -> Vec<(PathBuf, String)> {
        self.calls.lock().unwrap().clone()
    }

    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

impl Default for FakeIntegrationBranchPusher {
    fn default() -> Self {
        Self::new()
    }
}

impl IntegrationBranchPusher for FakeIntegrationBranchPusher {
    fn push(&self, project_path: &Path, integration_branch: &str) -> Result<(), PlanRunPhaseError> {
        self.calls
            .lock()
            .unwrap()
            .push((project_path.to_path_buf(), integration_branch.to_string()));
        if let Some(outcomes) = &self.outcomes {
            let mut queue = outcomes.lock().unwrap();
            assert!(
                !queue.is_empty(),
                "FakeIntegrationBranchPusher: push called {} time(s) but script is exhausted",
                self.calls.lock().unwrap().len()
            );
            let next = queue.remove(0);
            return match next {
                FakePushOutcome::Success => Ok(()),
                FakePushOutcome::Transient { stderr } => {
                    Err(PlanRunPhaseError::IntegrationPush(stderr))
                }
                FakePushOutcome::NonFastForward { stderr } => {
                    Err(PlanRunPhaseError::NonFastForward { stderr })
                }
            };
        }
        if let Some(error) = &self.error {
            Err(PlanRunPhaseError::IntegrationPush(error.clone()))
        } else {
            Ok(())
        }
    }
}

pub struct UnimplementedIntegrationBranchPusher;

impl IntegrationBranchPusher for UnimplementedIntegrationBranchPusher {
    fn push(
        &self,
        _project_path: &Path,
        _integration_branch: &str,
    ) -> Result<(), PlanRunPhaseError> {
        Err(PlanRunPhaseError::IntegrationPush(
            "real Integration Branch pusher not implemented yet".to_string(),
        ))
    }
}

/// Test cleaner that records cleanup calls and can be set to fail.
pub struct FakeAssignmentWorktreeCleaner {
    calls: Mutex<Vec<(PathBuf, PathBuf, String)>>,
    error: Option<String>,
}

impl FakeAssignmentWorktreeCleaner {
    pub fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            error: None,
        }
    }

    pub fn failing(error: impl Into<String>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            error: Some(error.into()),
        }
    }

    pub fn calls(&self) -> Vec<(PathBuf, PathBuf, String)> {
        self.calls.lock().unwrap().clone()
    }

    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

impl Default for FakeAssignmentWorktreeCleaner {
    fn default() -> Self {
        Self::new()
    }
}

impl AssignmentWorktreeCleaner for FakeAssignmentWorktreeCleaner {
    fn cleanup(
        &self,
        project_path: &Path,
        worktree_path: &Path,
        branch: &str,
    ) -> Result<(), PlanRunPhaseError> {
        self.calls.lock().unwrap().push((
            project_path.to_path_buf(),
            worktree_path.to_path_buf(),
            branch.to_string(),
        ));
        if let Some(error) = &self.error {
            Err(PlanRunPhaseError::Cleanup(error.clone()))
        } else {
            Ok(())
        }
    }
}

pub struct UnimplementedAssignmentWorktreeCleaner;

impl AssignmentWorktreeCleaner for UnimplementedAssignmentWorktreeCleaner {
    fn cleanup(
        &self,
        _project_path: &Path,
        _worktree_path: &Path,
        _branch: &str,
    ) -> Result<(), PlanRunPhaseError> {
        Err(PlanRunPhaseError::Cleanup(
            "real Assignment Worktree cleaner not implemented yet".to_string(),
        ))
    }
}

/// Parse the JSON body wrapped in `<merge>...</merge>` returned by the
/// Merge Phase agent. The merger reports either `merged` (integration
/// succeeded and the Integration Branch may be pushed) or `blocked`
/// (integration could not finish safely in this attempt).
pub fn parse_merge_output(stdout: &str) -> Result<ParsedMergeOutput, String> {
    let body = extract_tagged_json(stdout, "merge")?;
    let outcome = body
        .get("outcome")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "merge output missing `outcome` string".to_string())?;
    if !matches!(outcome, "merged" | "blocked") {
        return Err(format!(
            "merge output `outcome` must be merged|blocked, got {outcome}"
        ));
    }
    Ok(ParsedMergeOutput {
        outcome: outcome.to_string(),
        body,
    })
}

#[derive(Clone, Debug)]
pub struct ParsedMergeOutput {
    pub outcome: String,
    pub body: serde_json::Value,
}

// --- Per-Source-Issue fake runners (issue #46) ---
//
// Parallel Plan Runs interleave implementation / review / merge calls across
// multiple Issue Assignments. The queue-based fakes above are sufficient for
// single-assignment tests, but parallel tests need to return distinct
// stdouts per Source Issue without depending on call ordering. These
// matchers inspect the prompt for the `Source Issue: <id>` marker the prompt
// templates already render and pick the matching stdout deterministically.

fn source_id_from_prompt(prompt: &str) -> Option<String> {
    for line in prompt.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Source Issue:") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

fn pick_stdout_for_source<'a>(
    map: &'a std::collections::HashMap<String, Vec<String>>,
    fallback: Option<&'a str>,
    prompt: &str,
    counters: &Mutex<std::collections::HashMap<String, usize>>,
) -> Result<String, PlanRunPhaseError> {
    let source_id = source_id_from_prompt(prompt).unwrap_or_default();
    if let Some(queue) = map.get(&source_id) {
        if queue.is_empty() {
            return Err(PlanRunPhaseError::Planning(format!(
                "no fake stdout configured for source_id `{source_id}`"
            )));
        }
        let mut counters = counters.lock().unwrap();
        let idx = counters.entry(source_id.clone()).or_insert(0);
        let stdout = if *idx >= queue.len() {
            queue.last().cloned().unwrap()
        } else {
            let value = queue[*idx].clone();
            *idx += 1;
            value
        };
        return Ok(stdout);
    }
    if let Some(stdout) = fallback {
        return Ok(stdout.to_string());
    }
    Err(PlanRunPhaseError::Planning(format!(
        "no fake stdout configured for source_id `{source_id}`"
    )))
}

/// Implementation runner that picks stdout by `Source Issue:` line found in
/// the rendered prompt. Used by parallel Plan Run tests (issue #46).
pub struct PerSourceImplementationPhaseRunner {
    map: std::collections::HashMap<String, Vec<String>>,
    fallback: Option<String>,
    counters: Mutex<std::collections::HashMap<String, usize>>,
    prompts: Mutex<Vec<String>>,
}

impl PerSourceImplementationPhaseRunner {
    pub fn new() -> Self {
        Self {
            map: std::collections::HashMap::new(),
            fallback: None,
            counters: Mutex::new(std::collections::HashMap::new()),
            prompts: Mutex::new(Vec::new()),
        }
    }

    pub fn with_source(mut self, source_id: impl Into<String>, stdout: impl Into<String>) -> Self {
        self.map
            .entry(source_id.into())
            .or_default()
            .push(stdout.into());
        self
    }

    pub fn with_source_stdouts<S: Into<String>>(
        mut self,
        source_id: impl Into<String>,
        stdouts: impl IntoIterator<Item = S>,
    ) -> Self {
        let entry = self.map.entry(source_id.into()).or_default();
        for stdout in stdouts {
            entry.push(stdout.into());
        }
        self
    }

    pub fn with_fallback(mut self, stdout: impl Into<String>) -> Self {
        self.fallback = Some(stdout.into());
        self
    }

    pub fn prompts(&self) -> Vec<String> {
        self.prompts.lock().unwrap().clone()
    }

    pub fn call_count(&self) -> usize {
        self.prompts.lock().unwrap().len()
    }

    pub fn last_prompt(&self) -> Option<String> {
        self.prompts.lock().unwrap().last().cloned()
    }
}

impl Default for PerSourceImplementationPhaseRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl ImplementationPhaseRunner for PerSourceImplementationPhaseRunner {
    fn run(
        &self,
        prompt: &str,
        _context: &AssignmentContext<'_>,
    ) -> Result<String, PlanRunPhaseError> {
        self.prompts.lock().unwrap().push(prompt.to_string());
        pick_stdout_for_source(&self.map, self.fallback.as_deref(), prompt, &self.counters)
    }
}

/// Review runner that picks stdout by `Source Issue:` line in the prompt.
pub struct PerSourceReviewPhaseRunner {
    map: std::collections::HashMap<String, Vec<String>>,
    fallback: Option<String>,
    counters: Mutex<std::collections::HashMap<String, usize>>,
    prompts: Mutex<Vec<String>>,
}

impl PerSourceReviewPhaseRunner {
    pub fn new() -> Self {
        Self {
            map: std::collections::HashMap::new(),
            fallback: None,
            counters: Mutex::new(std::collections::HashMap::new()),
            prompts: Mutex::new(Vec::new()),
        }
    }

    pub fn with_source(mut self, source_id: impl Into<String>, stdout: impl Into<String>) -> Self {
        self.map
            .entry(source_id.into())
            .or_default()
            .push(stdout.into());
        self
    }

    pub fn with_source_stdouts<S: Into<String>>(
        mut self,
        source_id: impl Into<String>,
        stdouts: impl IntoIterator<Item = S>,
    ) -> Self {
        let entry = self.map.entry(source_id.into()).or_default();
        for stdout in stdouts {
            entry.push(stdout.into());
        }
        self
    }

    pub fn with_fallback(mut self, stdout: impl Into<String>) -> Self {
        self.fallback = Some(stdout.into());
        self
    }

    pub fn prompts(&self) -> Vec<String> {
        self.prompts.lock().unwrap().clone()
    }

    pub fn call_count(&self) -> usize {
        self.prompts.lock().unwrap().len()
    }

    pub fn last_prompt(&self) -> Option<String> {
        self.prompts.lock().unwrap().last().cloned()
    }
}

impl Default for PerSourceReviewPhaseRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl ReviewPhaseRunner for PerSourceReviewPhaseRunner {
    fn run(
        &self,
        prompt: &str,
        _context: &AssignmentContext<'_>,
    ) -> Result<String, PlanRunPhaseError> {
        self.prompts.lock().unwrap().push(prompt.to_string());
        pick_stdout_for_source(&self.map, self.fallback.as_deref(), prompt, &self.counters)
    }
}

/// Merge runner that picks stdout by `Source Issue:` line in the prompt.
pub struct PerSourceMergePhaseRunner {
    map: std::collections::HashMap<String, Vec<String>>,
    fallback: Option<String>,
    counters: Mutex<std::collections::HashMap<String, usize>>,
    prompts: Mutex<Vec<String>>,
}

impl PerSourceMergePhaseRunner {
    pub fn new() -> Self {
        Self {
            map: std::collections::HashMap::new(),
            fallback: None,
            counters: Mutex::new(std::collections::HashMap::new()),
            prompts: Mutex::new(Vec::new()),
        }
    }

    pub fn with_source(mut self, source_id: impl Into<String>, stdout: impl Into<String>) -> Self {
        self.map
            .entry(source_id.into())
            .or_default()
            .push(stdout.into());
        self
    }

    pub fn with_fallback(mut self, stdout: impl Into<String>) -> Self {
        self.fallback = Some(stdout.into());
        self
    }

    pub fn prompts(&self) -> Vec<String> {
        self.prompts.lock().unwrap().clone()
    }

    pub fn call_count(&self) -> usize {
        self.prompts.lock().unwrap().len()
    }

    pub fn last_prompt(&self) -> Option<String> {
        self.prompts.lock().unwrap().last().cloned()
    }
}

impl Default for PerSourceMergePhaseRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl MergePhaseRunner for PerSourceMergePhaseRunner {
    fn run(
        &self,
        prompt: &str,
        _context: &AssignmentContext<'_>,
    ) -> Result<String, PlanRunPhaseError> {
        self.prompts.lock().unwrap().push(prompt.to_string());
        pick_stdout_for_source(&self.map, self.fallback.as_deref(), prompt, &self.counters)
    }
}

#[cfg(test)]
mod parse_planning_output_tests {
    use super::*;

    #[test]
    fn parses_json_inside_plan_tags() {
        let parsed = parse_planning_output(r#"<plan>{"issues":[],"summary":"none"}</plan>"#)
            .expect("planning output parses");

        assert!(parsed.is_empty);
        assert_eq!(parsed.body["summary"], "none");
    }

    #[test]
    fn tolerates_json_code_fence_inside_plan_tags() {
        let parsed = parse_planning_output(
            r#"<plan>
```json
{"issues":[],"summary":"none"}
```
</plan>"#,
        )
        .expect("fenced planning output parses");

        assert!(parsed.is_empty);
        assert_eq!(parsed.body["summary"], "none");
    }

    #[test]
    fn tolerates_generic_code_fence_inside_plan_tags() {
        let parsed = parse_planning_output(
            r#"<plan>
```
{"issues":[],"summary":"none"}
```
</plan>"#,
        )
        .expect("fenced planning output parses");

        assert!(parsed.is_empty);
        assert_eq!(parsed.body["summary"], "none");
    }

    #[test]
    fn parses_last_plan_block_when_prompt_text_is_echoed() {
        let stdout = r#"Output one JSON object wrapped in `<plan>` tags:
<plan>
{
  "issues": [
    {
      "source_issue_id": "{{SOURCE_ISSUE_ID}}",
      "title": "{{SOURCE_ISSUE_TITLE}}",
      "branch": "{{ISSUE_BRANCH}}",
      "selection_summary": "{{WHY_THIS_ISSUE_CAN_START_NOW}}"
    }
  ],
  "summary": "{{PLAN_SUMMARY}}"
}
</plan>

<plan>{"issues":[],"summary":"real answer"}</plan>"#;

        let parsed = parse_planning_output(stdout).expect("last planning block parses");

        assert!(parsed.is_empty);
        assert_eq!(parsed.body["summary"], "real answer");
    }

    #[test]
    fn parses_complete_json_after_plan_opening_without_closing_tag() {
        let parsed = parse_planning_output(r#"<plan>{"issues":[],"summary":"no close"}"#)
            .expect("planning output without closing tag parses when JSON is complete");

        assert!(parsed.is_empty);
        assert_eq!(parsed.body["summary"], "no close");
    }

    #[test]
    fn invalid_json_error_includes_short_body_excerpt() {
        let error = parse_planning_output("<plan>not json</plan>").unwrap_err();

        assert!(error.contains("body starts with: not json"), "{error}");
    }

    #[test]
    fn missing_plan_tag_error_includes_output_excerpt() {
        let error = parse_planning_output("plain text").unwrap_err();

        assert!(error.contains("output ends with: plain text"), "{error}");
    }
}

#[cfg(test)]
mod fake_pusher_tests {
    //! Unit tests for the scriptable `FakeIntegrationBranchPusher`
    //! (issue #63). The fake is the test seam that lets push-failure
    //! scenarios be expressed as data; FIFO consumption and panic-on-
    //! overflow are the contract the orchestrator tests rely on.
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn scripted_pusher_consumes_outcomes_fifo() {
        let pusher = FakeIntegrationBranchPusher::with_outcomes(vec![
            FakePushOutcome::Transient {
                stderr: "ssh: connection reset by peer".into(),
            },
            FakePushOutcome::NonFastForward {
                stderr: "! [rejected] main -> main (non-fast-forward)".into(),
            },
            FakePushOutcome::Success,
        ]);
        let path = PathBuf::from("/tmp/proj");

        match pusher.push(&path, "main") {
            Err(PlanRunPhaseError::IntegrationPush(detail)) => {
                assert!(detail.contains("connection reset"));
            }
            other => panic!("expected Transient -> IntegrationPush, got {other:?}"),
        }
        match pusher.push(&path, "main") {
            Err(PlanRunPhaseError::NonFastForward { stderr }) => {
                assert!(stderr.contains("non-fast-forward"));
            }
            other => panic!("expected NonFastForward, got {other:?}"),
        }
        assert!(pusher.push(&path, "main").is_ok());
        assert_eq!(pusher.call_count(), 3);
    }

    #[test]
    #[should_panic(expected = "script is exhausted")]
    fn scripted_pusher_panics_when_over_called() {
        let pusher = FakeIntegrationBranchPusher::with_outcomes(vec![FakePushOutcome::Success]);
        let path = PathBuf::from("/tmp/proj");
        let _ = pusher.push(&path, "main");
        let _ = pusher.push(&path, "main");
    }

    #[test]
    fn single_mode_constructors_remain_available() {
        // Unmigrated tests continue to use `new()` and `failing(...)`
        // unchanged.
        let path = PathBuf::from("/tmp/proj");

        let ok_pusher = FakeIntegrationBranchPusher::new();
        assert!(ok_pusher.push(&path, "main").is_ok());
        assert!(ok_pusher.push(&path, "main").is_ok());

        let fail_pusher = FakeIntegrationBranchPusher::failing("boom");
        match fail_pusher.push(&path, "main") {
            Err(PlanRunPhaseError::IntegrationPush(d)) => assert_eq!(d, "boom"),
            other => panic!("expected IntegrationPush, got {other:?}"),
        }
        match fail_pusher.push(&path, "main") {
            Err(PlanRunPhaseError::IntegrationPush(d)) => assert_eq!(d, "boom"),
            other => panic!("expected IntegrationPush again, got {other:?}"),
        }
    }
}
