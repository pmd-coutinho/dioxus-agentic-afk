//! Plan Run coordinator (issue #48).
//!
//! Owns the body of the Plan Run flow that previously lived inside the
//! `start_plan_run` HTTP handler in the control-plane server. The handler is
//! now a thin wrapper that validates the request, creates the Plan Run row,
//! delegates orchestration to [`run_plan_run`], and maps the typed result
//! back to an HTTP response.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use agentic_afk_contracts::{
    IssueAssignmentResponse, IssueSource, PhaseOutputResponse, PlanRunResponse,
    ProjectExecutionConfigResponse, ProjectResponse, SourceIssueSnapshot,
};
use agentic_afk_persistence::{self as persistence, Db, PersistenceError};

use crate::plan_run::{
    AssignmentWorktreeCleaner, AssignmentWorktreeProvisioner, FakeAssignmentWorktreeCleaner,
    FakeImplementationPhaseRunner, FakeIntegrationBranchPusher, FakeLifecycleWriter,
    FakeMergePhaseRunner, FakePlanningPhaseRunner, FakeReviewPhaseRunner,
    FakeWorktreeProvisioner, ImplementationPhaseRunner, IntegrationBranchPusher,
    IntegrationBranchRefresher, IssueLifecycleWriter, MergePhaseRunner, PlanningPhaseRunner,
    RefreshedBaseline, ReviewPhaseRunner, StaticIntegrationBranchRefresher,
    UnimplementedAssignmentWorktreeCleaner, UnimplementedImplementationPhaseRunner,
    UnimplementedIntegrationBranchPusher, UnimplementedIntegrationBranchRefresher,
    UnimplementedLifecycleWriter, UnimplementedMergePhaseRunner,
    UnimplementedPlanningPhaseRunner, UnimplementedReviewPhaseRunner,
    UnimplementedWorktreeProvisioner,
};
use crate::production::{
    CodexImplementationPhaseRunner, CodexMergePhaseRunner, CodexPlanningPhaseRunner,
    CodexReviewPhaseRunner, GhLifecycleWriter, GitAssignmentWorktreeCleaner,
    GitIntegrationBranchPusher, GitIntegrationBranchRefresher,
    WorktrunkAssignmentWorktreeProvisioner,
};

/// Plan Run phase dependencies wired into the router. Tests inject fakes;
/// production wires real-git / Codex implementations.
#[derive(Clone)]
pub struct PlanRunDeps {
    pub refresher: Arc<dyn IntegrationBranchRefresher>,
    pub planner: Arc<dyn PlanningPhaseRunner>,
    pub worktree: Arc<dyn AssignmentWorktreeProvisioner>,
    pub lifecycle: Arc<dyn IssueLifecycleWriter>,
    pub implementation: Arc<dyn ImplementationPhaseRunner>,
    pub review: Arc<dyn ReviewPhaseRunner>,
    /// Merge Phase runner. Integrates one reviewed Issue Assignment into the
    /// configured Integration Branch.
    pub merger: Arc<dyn MergePhaseRunner>,
    /// Push the Integration Branch after a verified successful merge. Held
    /// as a separate seam from the merge runner so the push boundary can be
    /// asserted independently in tests.
    pub pusher: Arc<dyn IntegrationBranchPusher>,
    /// Clean up the Assignment Worktree and deterministic branch after a
    /// successful merge so the worktree does not linger past completion.
    pub cleaner: Arc<dyn AssignmentWorktreeCleaner>,
    /// When set, the Plan Run coordinator builds per-Plan-Run production
    /// Codex phase runners (`planner`, `implementation`, `review`, `merger`)
    /// against this binary and the Project path, replacing the placeholder
    /// runners. `None` means tests own the runner wiring directly.
    pub production_codex_binary: Option<PathBuf>,
    /// When set, the Plan Run coordinator builds a per-Plan-Run production
    /// `IssueLifecycleWriter` against this `gh` binary and the Project's
    /// enabled Issue Source. `None` means tests own the lifecycle writer
    /// directly.
    pub production_gh_binary: Option<PathBuf>,
}

impl PlanRunDeps {
    pub fn unimplemented() -> Self {
        Self {
            refresher: Arc::new(UnimplementedIntegrationBranchRefresher),
            planner: Arc::new(UnimplementedPlanningPhaseRunner),
            worktree: Arc::new(UnimplementedWorktreeProvisioner),
            lifecycle: Arc::new(UnimplementedLifecycleWriter),
            implementation: Arc::new(UnimplementedImplementationPhaseRunner),
            review: Arc::new(UnimplementedReviewPhaseRunner),
            merger: Arc::new(UnimplementedMergePhaseRunner),
            pusher: Arc::new(UnimplementedIntegrationBranchPusher),
            cleaner: Arc::new(UnimplementedAssignmentWorktreeCleaner),
            production_codex_binary: None,
            production_gh_binary: None,
        }
    }

    /// Wire the Plan Run seams to real implementations for production. The
    /// Integration Branch refresher / pusher / cleaner and the Worktrunk
    /// provisioner are project-agnostic and constructed eagerly. The four
    /// Codex phase runners and the Issue Source lifecycle writer need
    /// per-Plan-Run project context (the Project path the Codex binary
    /// runs in, the Project's enabled Issue Source kind/locator) so the
    /// coordinator resolves them lazily per Plan Run via
    /// [`resolve_deps_for_project`]. The placeholder runners stored here
    /// only fire if the coordinator ever skipped resolution.
    pub fn production(
        worktrunk_binary_path: PathBuf,
        codex_binary_path: PathBuf,
        gh_binary_path: PathBuf,
    ) -> Self {
        Self {
            refresher: Arc::new(GitIntegrationBranchRefresher),
            planner: Arc::new(UnimplementedPlanningPhaseRunner),
            worktree: Arc::new(WorktrunkAssignmentWorktreeProvisioner::new(
                worktrunk_binary_path,
            )),
            lifecycle: Arc::new(UnimplementedLifecycleWriter),
            implementation: Arc::new(UnimplementedImplementationPhaseRunner),
            review: Arc::new(UnimplementedReviewPhaseRunner),
            merger: Arc::new(UnimplementedMergePhaseRunner),
            pusher: Arc::new(GitIntegrationBranchPusher),
            cleaner: Arc::new(GitAssignmentWorktreeCleaner),
            production_codex_binary: Some(codex_binary_path),
            production_gh_binary: Some(gh_binary_path),
        }
    }

    /// Build a `PlanRunDeps` with the default fake seams used by tests.
    /// The Planning runner returns an empty plan; the implementation and
    /// review runners return canned stubs. Tests override individual seams
    /// by mutating the struct fields.
    pub fn default_test_deps() -> Self {
        Self {
            refresher: Arc::new(StaticIntegrationBranchRefresher::new(RefreshedBaseline {
                commit_sha: "test-baseline".to_string(),
            })),
            planner: Arc::new(FakePlanningPhaseRunner::with_stdout(
                r#"<plan>{"issues":[],"summary":"none"}</plan>"#,
            )),
            worktree: Arc::new(FakeWorktreeProvisioner::new(
                std::env::temp_dir().join("agentic-afk-test-worktrees"),
            )),
            lifecycle: Arc::new(FakeLifecycleWriter::new()),
            implementation: Arc::new(FakeImplementationPhaseRunner::with_stdout(
                r#"<impl>{"outcome":"ready_for_review","summary":"stub","commits":[],"verification":[],"gaps":[]}</impl>"#,
            )),
            review: Arc::new(FakeReviewPhaseRunner::with_stdout(
                r#"<review>{"outcome":"approved","findings":[],"summary":"stub approved","verification":[],"gaps":[]}</review>"#,
            )),
            merger: Arc::new(FakeMergePhaseRunner::with_stdout(
                r#"<merge>{"outcome":"merged","summary":"stub merged","merged_source_ids":[],"verification":[],"gaps":[]}</merge>"#,
            )),
            pusher: Arc::new(FakeIntegrationBranchPusher::new()),
            cleaner: Arc::new(FakeAssignmentWorktreeCleaner::new()),
            production_codex_binary: None,
            production_gh_binary: None,
        }
    }
}

/// Resolve production phase runners and lifecycle writer using the given
/// Project as per-Plan-Run context. If `deps` carries production binary
/// paths, returns a new `PlanRunDeps` whose Codex runners point at the
/// Project path and whose lifecycle writer is bound to the Project's
/// enabled Issue Source. Otherwise returns a clone of `deps` unchanged so
/// tests keep their injected fakes.
pub fn resolve_deps_for_project(
    deps: &PlanRunDeps,
    project: &agentic_afk_contracts::ProjectResponse,
) -> PlanRunDeps {
    let mut resolved = deps.clone();
    if let Some(codex_binary) = deps.production_codex_binary.clone() {
        let project_path = std::path::PathBuf::from(&project.path);
        resolved.planner = Arc::new(CodexPlanningPhaseRunner::new(
            codex_binary.clone(),
            project_path.clone(),
        ));
        resolved.implementation = Arc::new(CodexImplementationPhaseRunner::new(
            codex_binary.clone(),
            project_path.clone(),
        ));
        resolved.review = Arc::new(CodexReviewPhaseRunner::new(
            codex_binary.clone(),
            project_path.clone(),
        ));
        resolved.merger = Arc::new(CodexMergePhaseRunner::new(codex_binary, project_path));
    }
    if let (Some(gh_binary), Some(source)) = (
        deps.production_gh_binary.clone(),
        project.enabled_issue_source.clone(),
    ) {
        resolved.lifecycle = Arc::new(GhLifecycleWriter::for_project(
            gh_binary,
            project.clone(),
            source,
        ));
    }
    resolved
}

/// Publishes Plan Run lifecycle events to a downstream consumer (the
/// control-plane server's per-Project event bus). Defined as a narrow trait
/// in the orchestrator crate so the coordinator does not depend on the HTTP
/// server's event-bus implementation directly.
pub trait EventPublisher: Send + Sync {
    fn plan_run_started(&self, project_id: &str, plan_run: PlanRunResponse);
    fn plan_run_completed(&self, project_id: &str, plan_run: PlanRunResponse);
    fn plan_run_phase_completed(
        &self,
        project_id: &str,
        plan_run_id: &str,
        phase_output: PhaseOutputResponse,
    );
    fn assignment_created(&self, project_id: &str, assignment: IssueAssignmentResponse);
    fn assignment_status_changed(&self, project_id: &str, assignment: IssueAssignmentResponse);
}

/// Coordinator failure surface. Each variant carries the HTTP status, a
/// stable problem-type URN, and a human-readable detail that the handler
/// turns into an RFC-7807 response body.
#[derive(Clone, Debug)]
pub struct CoordinatorError {
    pub status: u16,
    pub problem_type: String,
    pub detail: String,
}

impl CoordinatorError {
    pub fn new(status: u16, problem_type: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            status,
            problem_type: problem_type.into(),
            detail: detail.into(),
        }
    }

    pub fn from_persistence(error: PersistenceError) -> Self {
        let (status, problem_type) = match &error {
            PersistenceError::NotFound(_) => (404, "urn:agentic-afk:not-found"),
            PersistenceError::Duplicate(_) => (409, "urn:agentic-afk:duplicate"),
            PersistenceError::InvalidPath(_) => (422, "urn:agentic-afk:invalid-path"),
            PersistenceError::InvalidIssueSource(_) => {
                (422, "urn:agentic-afk:invalid-issue-source")
            }
            PersistenceError::SnapshotNotFound(_) => {
                (404, "urn:agentic-afk:planning-snapshot-not-found")
            }
            PersistenceError::ActiveAssignment(_) => (409, "urn:agentic-afk:active-assignment"),
            PersistenceError::AssignmentNotFound(_) => {
                (404, "urn:agentic-afk:assignment-not-found")
            }
            PersistenceError::Database(_) => (500, "urn:agentic-afk:internal-error"),
        };
        Self::new(status, problem_type, error.to_string())
    }
}

/// Run the Plan Run coordinator for an already-created Plan Run row.
///
/// The HTTP handler is responsible for validating the request (project
/// exists, trusted, no active Plan Run, etc.), refreshing the Integration
/// Branch baseline, creating the row, and publishing the
/// `PlanRunStarted` event. This entry point owns everything after that:
/// planning, parallel implementation+review, merge, push, cleanup, and the
/// terminal Plan Run state.
///
/// Returns the finished `PlanRunResponse` for the HTTP layer to serialize
/// as the 201 body, or a [`CoordinatorError`] for the handler to convert
/// into an RFC-7807 problem response.
#[allow(clippy::too_many_arguments)]
pub async fn run_plan_run(
    deps: &PlanRunDeps,
    db: &Db,
    events: &Arc<dyn EventPublisher>,
    gh_binary_path: &Path,
    project: &ProjectResponse,
    project_id: &str,
    plan_run: &PlanRunResponse,
    baseline: &RefreshedBaseline,
    execution_config: &ProjectExecutionConfigResponse,
) -> Result<PlanRunResponse, CoordinatorError> {
    // Resolve production runners against the per-Plan-Run Project context
    // (codex binary cwd + Issue Source kind/locator) when the caller wired
    // production binaries. Tests with injected fakes flow through
    // unchanged because `production_codex_binary` / `production_gh_binary`
    // remain `None`.
    let resolved = resolve_deps_for_project(deps, project);
    let deps = &resolved;
    let eligible = persistence::get_planning_snapshot(db, project_id)
        .await
        .map(|snapshot| snapshot.eligible)
        .unwrap_or_default();
    let project_instructions = load_project_instructions(&project.path);
    let prompt = render_planning_prompt(
        &project_instructions,
        project,
        execution_config,
        baseline,
        &eligible,
    );
    let planner_stdout = match deps.planner.run(&prompt) {
        Ok(stdout) => stdout,
        Err(error) => {
            let _ = persistence::record_plan_run_phase_output(
                db,
                &plan_run.id,
                "planning",
                "failed",
                &serde_json::json!({ "error": error.to_string() }),
            )
            .await;
            if let Ok(run) = persistence::finish_plan_run(db, &plan_run.id, "failed").await {
                events.plan_run_completed(project_id, run);
            }
            return Err(CoordinatorError::new(
                500,
                "urn:agentic-afk:planning-phase-failed",
                error.to_string(),
            ));
        }
    };

    let parsed = match crate::parse_planning_output(&planner_stdout) {
        Ok(parsed) => parsed,
        Err(error) => {
            let _ = persistence::record_plan_run_phase_output(
                db,
                &plan_run.id,
                "planning",
                "failed",
                &serde_json::json!({ "error": error }),
            )
            .await;
            if let Ok(run) = persistence::finish_plan_run(db, &plan_run.id, "failed").await {
                events.plan_run_completed(project_id, run);
            }
            return Err(CoordinatorError::new(
                500,
                "urn:agentic-afk:planning-output-unparseable",
                error,
            ));
        }
    };

    if parsed.is_empty {
        return finalize_empty_planning(db, events, project_id, &plan_run.id, &parsed.body).await;
    }

    finalize_selection_planning(
        deps,
        db,
        events,
        gh_binary_path,
        project,
        project_id,
        plan_run,
        baseline,
        &parsed.body,
    )
    .await
}

async fn finalize_empty_planning(
    db: &Db,
    events: &Arc<dyn EventPublisher>,
    project_id: &str,
    plan_run_id: &str,
    body: &serde_json::Value,
) -> Result<PlanRunResponse, CoordinatorError> {
    let phase_output = persistence::record_plan_run_phase_output(
        db,
        plan_run_id,
        "planning",
        "succeeded_empty",
        body,
    )
    .await
    .map_err(CoordinatorError::from_persistence)?;
    events.plan_run_phase_completed(project_id, plan_run_id, phase_output);
    let finished = persistence::finish_plan_run(db, plan_run_id, "succeeded_empty")
        .await
        .map_err(CoordinatorError::from_persistence)?;
    events.plan_run_completed(project_id, finished.clone());
    Ok(finished)
}

#[allow(clippy::too_many_arguments)]
async fn finalize_selection_planning(
    deps: &PlanRunDeps,
    db: &Db,
    events: &Arc<dyn EventPublisher>,
    gh_binary_path: &Path,
    project: &ProjectResponse,
    project_id: &str,
    plan_run: &PlanRunResponse,
    baseline: &RefreshedBaseline,
    body: &serde_json::Value,
) -> Result<PlanRunResponse, CoordinatorError> {
    let parsed = crate::ParsedPlanningOutput {
        is_empty: false,
        body: body.clone(),
    };
    let selections = match crate::extract_planner_selections(&parsed) {
        Ok(selections) => selections,
        Err(error) => {
            return Err(fail_planning_phase(
                db,
                events,
                project_id,
                &plan_run.id,
                &error,
                "urn:agentic-afk:planning-output-unparseable",
            )
            .await);
        }
    };

    let snapshot = match persistence::get_planning_snapshot(db, project_id).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return Err(fail_planning_phase(
                db,
                events,
                project_id,
                &plan_run.id,
                &error.to_string(),
                "urn:agentic-afk:planning-snapshot-missing",
            )
            .await);
        }
    };

    let eligible_by_id: std::collections::HashMap<&str, &SourceIssueSnapshot> = snapshot
        .eligible
        .iter()
        .map(|issue| (issue.source_id.as_str(), issue))
        .collect();

    let exec_config_lookup =
        match persistence::get_project_execution_config(db, project_id).await {
            Ok(Some(config)) => config,
            Ok(None) => {
                return Err(fail_planning_phase(
                    db,
                    events,
                    project_id,
                    &plan_run.id,
                    "Project Execution Config disappeared during Planning Phase",
                    "urn:agentic-afk:execution-config-missing",
                )
                .await);
            }
            Err(error) => return Err(CoordinatorError::from_persistence(error)),
        };

    // The Planning Phase may return up to Max Parallel Tasks selections for
    // one Plan Run. Selections beyond the configured cap force the planner
    // to converge for now rather than implicitly truncating the batch.
    let max_parallel = exec_config_lookup.max_parallel_tasks.max(1) as usize;
    if selections.len() > max_parallel {
        return Err(fail_planning_phase(
            db,
            events,
            project_id,
            &plan_run.id,
            &format!(
                "Planning Phase returned {} issues but Project Max Parallel Tasks is {}",
                selections.len(),
                max_parallel
            ),
            "urn:agentic-afk:planning-exceeds-max-parallel",
        )
        .await);
    }

    let Some(source) = project.enabled_issue_source.clone() else {
        return Err(fail_planning_phase(
            db,
            events,
            project_id,
            &plan_run.id,
            "Project has no enabled Issue Source for claim write-back",
            "urn:agentic-afk:issue-source-missing",
        )
        .await);
    };

    // Provision all assignments sequentially to preserve deterministic
    // ordering in Plan Run snapshots. Each row is created, worktree
    // provisioned, lifecycle written back, and an `AssignmentCreated`
    // event published before any implementation pass starts. This keeps
    // the parallel tranche observable from the Dashboard immediately.
    let mut claimed: Vec<IssueAssignmentResponse> = Vec::with_capacity(selections.len());
    let project_path = std::path::Path::new(&project.path);
    for selection in &selections {
        let Some(issue) = eligible_by_id.get(selection.source_issue_id.as_str()).copied() else {
            return Err(fail_planning_phase(
                db,
                events,
                project_id,
                &plan_run.id,
                &format!(
                    "Planning Phase selected Source Issue {} which is not in the eligible set",
                    selection.source_issue_id
                ),
                "urn:agentic-afk:planning-selection-ineligible",
            )
            .await);
        };
        let assignment = persistence::create_plan_run_assignment(
            db,
            &plan_run.id,
            project_id,
            &source,
            issue,
            &selection.branch,
            &selection.selection_summary,
        )
        .await
        .map_err(CoordinatorError::from_persistence)?;

        let worktree_path = match deps.worktree.provision(
            project_path,
            &baseline.commit_sha,
            &selection.branch,
        ) {
            Ok(path) => path,
            Err(error) => {
                let _ = persistence::release_issue_assignment(db, &assignment.id).await;
                return Err(fail_planning_phase(
                    db,
                    events,
                    project_id,
                    &plan_run.id,
                    &error.to_string(),
                    "urn:agentic-afk:assignment-worktree-failed",
                )
                .await);
            }
        };
        let worktree_path_str = worktree_path.to_string_lossy().into_owned();
        let assignment = persistence::set_assignment_worktree(db, &assignment.id, &worktree_path_str)
            .await
            .map_err(CoordinatorError::from_persistence)?;

        if let Err(error) = deps.lifecycle.write_claimed(&issue.source_id) {
            let _ = persistence::release_issue_assignment(db, &assignment.id).await;
            return Err(fail_planning_phase(
                db,
                events,
                project_id,
                &plan_run.id,
                &error.to_string(),
                "urn:agentic-afk:issue-source-lifecycle-failed",
            )
            .await);
        }

        events.assignment_created(project_id, assignment.clone());
        claimed.push(assignment);
    }

    let phase_output = persistence::record_plan_run_phase_output(
        db,
        &plan_run.id,
        "planning",
        "succeeded",
        body,
    )
    .await
    .map_err(CoordinatorError::from_persistence)?;
    events.plan_run_phase_completed(project_id, &plan_run.id, phase_output);

    // Drive implementation+review for every claimed assignment concurrently
    // (bounded by Max Parallel Tasks since claim already capped). Each task
    // owns its own Review Loop and finishes with a per-assignment
    // `AssignmentBatchOutcome`. The merge phase runs sequentially across
    // reviewed successful assignments afterward so the Integration Branch
    // sees one merge at a time.
    let outcomes = match run_parallel_implement_review(
        deps,
        db,
        events,
        gh_binary_path,
        project,
        project_id,
        plan_run,
        baseline,
        &exec_config_lookup,
        &claimed,
    )
    .await
    {
        Ok(outcomes) => outcomes,
        Err(error) => {
            // A hard phase failure short-circuits the parallel tranche.
            // Finish the Plan Run as `failed` so the Dashboard records
            // the terminal state alongside the per-assignment phase
            // failure already persisted by `fail_assignment_phase`.
            if let Ok(run) = persistence::finish_plan_run(db, &plan_run.id, "failed").await {
                events.plan_run_completed(project_id, run);
            }
            return Err(error);
        }
    };

    finalize_parallel_plan_run(
        deps,
        db,
        events,
        gh_binary_path,
        project,
        project_id,
        plan_run,
        baseline,
        &exec_config_lookup,
        outcomes,
    )
    .await
}

/// Per-assignment outcome of the parallel implementation+review tranche.
/// The merge phase then consumes the reviewed successes one at a time so
/// the Integration Branch sees a single merge attempt per reviewed
/// assignment.
#[derive(Clone, Debug)]
struct AssignmentBatchOutcome {
    assignment: IssueAssignmentResponse,
    /// Phase Output body recorded for the approving Review Phase, used as
    /// the merge prompt's reviewed evidence. `None` when the assignment
    /// blocked before reaching `reviewed`.
    review_body: Option<serde_json::Value>,
}

#[allow(clippy::too_many_arguments)]
async fn run_parallel_implement_review(
    deps: &PlanRunDeps,
    db: &Db,
    events: &Arc<dyn EventPublisher>,
    gh_binary_path: &Path,
    project: &ProjectResponse,
    project_id: &str,
    plan_run: &PlanRunResponse,
    baseline: &RefreshedBaseline,
    exec_config: &ProjectExecutionConfigResponse,
    claimed: &[IssueAssignmentResponse],
) -> Result<Vec<AssignmentBatchOutcome>, CoordinatorError> {
    use tokio::task::JoinSet;

    let mut join_set: JoinSet<(
        IssueAssignmentResponse,
        Result<Option<serde_json::Value>, CoordinatorError>,
    )> = JoinSet::new();
    for assignment in claimed {
        let deps = deps.clone();
        let db = db.clone();
        let events = events.clone();
        let gh_binary_path = gh_binary_path.to_path_buf();
        let project = project.clone();
        let project_id = project_id.to_string();
        let plan_run = plan_run.clone();
        let baseline = baseline.clone();
        let exec_config = exec_config.clone();
        let assignment = assignment.clone();
        join_set.spawn(async move {
            let outcome = run_assignment_implement_review(
                &deps,
                &db,
                &events,
                &gh_binary_path,
                &project,
                &project_id,
                &plan_run,
                &baseline,
                &exec_config,
                &assignment,
            )
            .await;
            (assignment, outcome)
        });
    }

    // Collect into a (source_id -> outcome) map so we can preserve the
    // deterministic claim order on the way out. `JoinSet` reports
    // completion in the order tasks finish, which is non-deterministic.
    let mut by_source: std::collections::HashMap<String, AssignmentBatchOutcome> =
        std::collections::HashMap::with_capacity(claimed.len());
    while let Some(joined) = join_set.join_next().await {
        let (assignment, outcome) = match joined {
            Ok(value) => value,
            Err(error) => {
                return Err(CoordinatorError::new(
                    500,
                    "urn:agentic-afk:assignment-task-panic",
                    format!("assignment task panicked: {error}"),
                ));
            }
        };
        let review_body = outcome?;
        // Re-read the assignment so the latest status (reviewed / blocked)
        // is captured even if the orchestrator updated it after the task
        // returned.
        let refreshed = persistence::get_assignment(db, &assignment.id)
            .await
            .map_err(CoordinatorError::from_persistence)?;
        by_source.insert(
            refreshed.source_id.clone(),
            AssignmentBatchOutcome {
                assignment: refreshed,
                review_body,
            },
        );
    }

    let mut outcomes = Vec::with_capacity(claimed.len());
    for assignment in claimed {
        if let Some(outcome) = by_source.remove(&assignment.source_id) {
            outcomes.push(outcome);
        }
    }
    Ok(outcomes)
}

/// Drive the implementation + review Review Loop for a single Issue
/// Assignment. Returns the approving review body for the Merge Phase, or
/// `None` if the assignment blocked before reaching `reviewed`.
#[allow(clippy::too_many_arguments)]
async fn run_assignment_implement_review(
    deps: &PlanRunDeps,
    db: &Db,
    events: &Arc<dyn EventPublisher>,
    gh_binary_path: &Path,
    project: &ProjectResponse,
    project_id: &str,
    plan_run: &PlanRunResponse,
    baseline: &RefreshedBaseline,
    exec_config: &ProjectExecutionConfigResponse,
    assignment: &IssueAssignmentResponse,
) -> Result<Option<serde_json::Value>, CoordinatorError> {
    let project_instructions = load_project_instructions(&project.path);
    let raw_text = persistence::get_assignment_source_raw_text(db, &assignment.id)
        .await
        .map_err(CoordinatorError::from_persistence)?;

    let mut review_findings = String::new();
    let mut loop_iteration: i64 = 0;
    loop {
        loop_iteration += 1;

        let assignment_state =
            persistence::set_assignment_status(db, &assignment.id, "implementing", None)
                .await
                .map_err(CoordinatorError::from_persistence)?;
        events.assignment_status_changed(project_id, assignment_state);

        let impl_prompt = render_implementation_prompt(
            &project_instructions,
            project,
            plan_run,
            baseline,
            exec_config,
            assignment,
            &raw_text,
            &review_findings,
        );
        let impl_stdout = match deps.implementation.run(&impl_prompt) {
            Ok(stdout) => stdout,
            Err(error) => {
                return Err(fail_assignment_phase(
                    db,
                    events,
                    project_id,
                    assignment,
                    "implementation",
                    &error.to_string(),
                    "urn:agentic-afk:implementation-phase-failed",
                )
                .await);
            }
        };
        let impl_parsed = match crate::parse_implementation_output(&impl_stdout) {
            Ok(parsed) => parsed,
            Err(error) => {
                return Err(fail_assignment_phase(
                    db,
                    events,
                    project_id,
                    assignment,
                    "implementation",
                    &error,
                    "urn:agentic-afk:implementation-output-unparseable",
                )
                .await);
            }
        };
        if impl_parsed.outcome != "ready_for_review" {
            return Err(fail_assignment_phase(
                db,
                events,
                project_id,
                assignment,
                "implementation",
                &format!(
                    "implementation outcome `{}` does not enter Review Phase",
                    impl_parsed.outcome
                ),
                "urn:agentic-afk:implementation-not-ready",
            )
            .await);
        }
        let impl_output = persistence::record_assignment_phase_output(
            db,
            &plan_run.id,
            &assignment.id,
            "implementation",
            &impl_parsed.outcome,
            &impl_parsed.body,
        )
        .await
        .map_err(CoordinatorError::from_persistence)?;
        events.plan_run_phase_completed(project_id, &plan_run.id, impl_output);

        let assignment_state =
            persistence::set_assignment_status(db, &assignment.id, "implemented", None)
                .await
                .map_err(CoordinatorError::from_persistence)?;
        events.assignment_status_changed(project_id, assignment_state);

        let review_prompt = render_review_prompt(
            &project_instructions,
            project,
            plan_run,
            baseline,
            exec_config,
            assignment,
            &raw_text,
            &impl_parsed.body,
        );
        let review_stdout = match deps.review.run(&review_prompt) {
            Ok(stdout) => stdout,
            Err(error) => {
                return Err(fail_assignment_phase(
                    db,
                    events,
                    project_id,
                    assignment,
                    "review",
                    &error.to_string(),
                    "urn:agentic-afk:review-phase-failed",
                )
                .await);
            }
        };
        let review_parsed = match crate::parse_review_output(&review_stdout) {
            Ok(parsed) => parsed,
            Err(error) => {
                return Err(fail_assignment_phase(
                    db,
                    events,
                    project_id,
                    assignment,
                    "review",
                    &error,
                    "urn:agentic-afk:review-output-unparseable",
                )
                .await);
            }
        };
        let review_output = persistence::record_assignment_phase_output(
            db,
            &plan_run.id,
            &assignment.id,
            "review",
            &review_parsed.outcome,
            &review_parsed.body,
        )
        .await
        .map_err(CoordinatorError::from_persistence)?;
        events.plan_run_phase_completed(project_id, &plan_run.id, review_output);

        if review_parsed.outcome == "approved" {
            let reviewed =
                persistence::set_assignment_status(db, &assignment.id, "reviewed", None)
                    .await
                    .map_err(CoordinatorError::from_persistence)?;
            events.assignment_status_changed(project_id, reviewed);
            return Ok(Some(review_parsed.body));
        }

        let rejection_count = persistence::increment_review_rejection(db, &assignment.id)
            .await
            .map_err(CoordinatorError::from_persistence)?;
        review_findings = extract_review_findings(&review_parsed.body);

        if rejection_count >= exec_config.review_retry_limit {
            let reason = format!(
                "Review Loop exhausted: {rejection_count} rejection(s) reached the Project Review Retry Limit ({}).",
                exec_config.review_retry_limit
            );
            block_assignment_for_loop(
                db,
                events,
                gh_binary_path,
                project,
                project_id,
                assignment,
                &reason,
            )
            .await?;
            return Ok(None);
        }
        if loop_iteration > exec_config.review_retry_limit + 1 {
            let reason =
                format!("Review Loop ran {loop_iteration} iterations without converging; blocking.");
            block_assignment_for_loop(
                db,
                events,
                gh_binary_path,
                project,
                project_id,
                assignment,
                &reason,
            )
            .await?;
            return Ok(None);
        }
    }
}

/// Block an assignment that exhausted its Review Loop without finishing
/// the surrounding Plan Run. Used by the parallel tranche so blocked
/// assignments stay outside the Merge Phase while reviewed peers continue.
async fn block_assignment_for_loop(
    db: &Db,
    events: &Arc<dyn EventPublisher>,
    gh_binary_path: &Path,
    project: &ProjectResponse,
    project_id: &str,
    assignment: &IssueAssignmentResponse,
    reason: &str,
) -> Result<(), CoordinatorError> {
    let blocked = persistence::block_assignment(db, &assignment.id, reason)
        .await
        .map_err(CoordinatorError::from_persistence)?;
    events.assignment_status_changed(project_id, blocked);
    if let Some(source) = project.enabled_issue_source.as_ref() {
        if let Err(error) = write_assignment_lifecycle(
            gh_binary_path,
            project,
            source,
            &assignment.source_id,
            "blocked",
        ) {
            eprintln!(
                "warning: failed to write blocked Lifecycle Status back to Issue Source: {error}"
            );
        }
    }
    Ok(())
}

/// Record a failure Phase Output for an assignment, move it to `blocked`,
/// and return a `CoordinatorError` so the caller can short-circuit. Unlike
/// `fail_assignment` this does NOT finish the surrounding Plan Run — the
/// parallel orchestrator finishes the Plan Run once all peers have
/// completed.
async fn fail_assignment_phase(
    db: &Db,
    events: &Arc<dyn EventPublisher>,
    project_id: &str,
    assignment: &IssueAssignmentResponse,
    phase: &str,
    error: &str,
    problem_type: &str,
) -> CoordinatorError {
    let _ = persistence::record_assignment_phase_output(
        db,
        assignment.plan_run_id.as_deref().unwrap_or_default(),
        &assignment.id,
        phase,
        "failed",
        &serde_json::json!({ "error": error }),
    )
    .await;
    if let Ok(updated) =
        persistence::set_assignment_status(db, &assignment.id, "blocked", Some(error)).await
    {
        events.assignment_status_changed(project_id, updated);
    }
    CoordinatorError::new(500, problem_type, error)
}

/// Finish a Plan Run after the parallel implementation + review tranche
/// finishes. Merges reviewed assignments one at a time, cleans both merged
/// and blocked worktrees, and writes the appropriate terminal Plan Run
/// state. Mixed outcomes finish as `succeeded` since reviewed work merged;
/// only all-blocked Plan Runs finish as `failed`.
#[allow(clippy::too_many_arguments)]
async fn finalize_parallel_plan_run(
    deps: &PlanRunDeps,
    db: &Db,
    events: &Arc<dyn EventPublisher>,
    gh_binary_path: &Path,
    project: &ProjectResponse,
    project_id: &str,
    plan_run: &PlanRunResponse,
    baseline: &RefreshedBaseline,
    exec_config: &ProjectExecutionConfigResponse,
    outcomes: Vec<AssignmentBatchOutcome>,
) -> Result<PlanRunResponse, CoordinatorError> {
    let project_instructions = load_project_instructions(&project.path);
    let project_path = std::path::Path::new(&project.path);

    let mut merged_count = 0usize;
    let mut reviewed_count = 0usize;
    let mut blocked_assignments: Vec<IssueAssignmentResponse> = Vec::new();
    let mut merged_assignments: Vec<IssueAssignmentResponse> = Vec::new();

    for outcome in &outcomes {
        match outcome.review_body.as_ref() {
            Some(review_body) => {
                reviewed_count += 1;
                let merge_assignment = outcome.assignment.clone();
                let merging =
                    persistence::set_assignment_status(db, &merge_assignment.id, "merging", None)
                        .await
                        .map_err(CoordinatorError::from_persistence)?;
                events.assignment_status_changed(project_id, merging);

                let prompt = render_merge_prompt(
                    &project_instructions,
                    project,
                    plan_run,
                    baseline,
                    exec_config,
                    &merge_assignment,
                    review_body,
                );
                let merge_stdout = match deps.merger.run(&prompt) {
                    Ok(stdout) => stdout,
                    Err(error) => {
                        let _ = persistence::record_assignment_phase_output(
                            db,
                            &plan_run.id,
                            &merge_assignment.id,
                            "merge",
                            "failed",
                            &serde_json::json!({ "error": error.to_string() }),
                        )
                        .await;
                        let blocked = persistence::block_assignment(
                            db,
                            &merge_assignment.id,
                            &error.to_string(),
                        )
                        .await
                        .map_err(CoordinatorError::from_persistence)?;
                        events.assignment_status_changed(project_id, blocked.clone());
                        blocked_assignments.push(blocked);
                        continue;
                    }
                };
                let merge_parsed = match crate::parse_merge_output(&merge_stdout) {
                    Ok(parsed) => parsed,
                    Err(error) => {
                        let _ = persistence::record_assignment_phase_output(
                            db,
                            &plan_run.id,
                            &merge_assignment.id,
                            "merge",
                            "failed",
                            &serde_json::json!({ "error": error }),
                        )
                        .await;
                        let blocked =
                            persistence::block_assignment(db, &merge_assignment.id, &error)
                                .await
                                .map_err(CoordinatorError::from_persistence)?;
                        events.assignment_status_changed(project_id, blocked.clone());
                        blocked_assignments.push(blocked);
                        continue;
                    }
                };
                let merge_output = persistence::record_assignment_phase_output(
                    db,
                    &plan_run.id,
                    &merge_assignment.id,
                    "merge",
                    &merge_parsed.outcome,
                    &merge_parsed.body,
                )
                .await
                .map_err(CoordinatorError::from_persistence)?;
                events.plan_run_phase_completed(project_id, &plan_run.id, merge_output);

                if merge_parsed.outcome == "blocked" {
                    let reason = merge_parsed
                        .body
                        .get("block_reason")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| {
                            merge_parsed
                                .body
                                .get("summary")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string)
                                .unwrap_or_else(|| {
                                    "Merge Phase blocked without an explicit reason".to_string()
                                })
                        });
                    let blocked =
                        persistence::block_assignment(db, &merge_assignment.id, &reason)
                            .await
                            .map_err(CoordinatorError::from_persistence)?;
                    events.assignment_status_changed(project_id, blocked.clone());
                    if let Some(source) = project.enabled_issue_source.as_ref() {
                        if let Err(error) = write_assignment_lifecycle(
                            gh_binary_path,
                            project,
                            source,
                            &merge_assignment.source_id,
                            "blocked",
                        ) {
                            eprintln!(
                                "warning: failed to write blocked Lifecycle Status back to Issue Source during Merge Phase: {error}"
                            );
                        }
                    }
                    blocked_assignments.push(blocked);
                    continue;
                }

                // Merge succeeded: transition the assignment to `merged`
                // and complete the Source Issue. The Integration Branch
                // push happens once at the end so the upstream sees a
                // single push for the whole merged set.
                let merged =
                    persistence::set_assignment_status(db, &merge_assignment.id, "merged", None)
                        .await
                        .map_err(CoordinatorError::from_persistence)?;
                events.assignment_status_changed(project_id, merged.clone());
                // Source Issue completion only after the verified Integration
                // Branch push. The completed lifecycle write-back is deferred
                // until after `pusher.push` succeeds below.
                merged_count += 1;
                merged_assignments.push(merged);
            }
            None => {
                blocked_assignments.push(outcome.assignment.clone());
            }
        }
    }

    // Push the Integration Branch once if at least one merge succeeded.
    // The push is part of the canonical merge boundary: blocked merges
    // never push, and the push happens after every merge attempt has
    // settled so the upstream only sees the final integrated tree.
    if merged_count > 0 {
        if let Err(error) = deps
            .pusher
            .push(project_path, &exec_config.integration_branch)
        {
            // Push failure: surface the error but keep the merged
            // assignments as `merged` (the integration already happened
            // locally). The Plan Run finishes as `failed` so the
            // developer notices.
            for assignment in &merged_assignments {
                let _ = persistence::record_assignment_phase_output(
                    db,
                    &plan_run.id,
                    &assignment.id,
                    "merge",
                    "failed",
                    &serde_json::json!({ "error": error.to_string() }),
                )
                .await;
            }
            if let Ok(run) = persistence::finish_plan_run(db, &plan_run.id, "failed").await {
                events.plan_run_completed(project_id, run);
            }
            return Err(CoordinatorError::new(
                500,
                "urn:agentic-afk:integration-branch-push-failed",
                error.to_string(),
            ));
        }

        // Push succeeded: now complete the Source Issues. Completion only
        // after the verified Integration Branch push so upstream lifecycle
        // state never claims work the developer did not actually receive.
        if let Some(source) = project.enabled_issue_source.as_ref() {
            for assignment in &merged_assignments {
                if let Err(error) = write_assignment_lifecycle(
                    gh_binary_path,
                    project,
                    source,
                    &assignment.source_id,
                    "completed",
                ) {
                    eprintln!(
                        "warning: failed to write completed Lifecycle Status back to Issue Source after push: {error}"
                    );
                }
            }
        }
    }

    // Cleanup: merged assignments AND blocked assignments both get their
    // worktrees + deterministic branches cleaned at Plan Run finish, so
    // dormant blocked work does not consume Max Parallel Tasks via stale
    // worktrees. Phase Outputs are already durable on the Plan Run row,
    // so cleanup is safe.
    let mut cleanup_targets: Vec<IssueAssignmentResponse> = Vec::new();
    cleanup_targets.extend(merged_assignments.iter().cloned());
    cleanup_targets.extend(blocked_assignments.iter().cloned());
    for assignment in &cleanup_targets {
        if assignment.worktree_path.is_empty() {
            continue;
        }
        let worktree_path = std::path::Path::new(&assignment.worktree_path);
        if let Err(error) =
            deps.cleaner
                .cleanup(project_path, worktree_path, &assignment.branch)
        {
            eprintln!(
                "warning: failed to clean up Assignment Worktree for {} after Plan Run finish: {error}",
                assignment.source_id
            );
        }
    }

    // Plan Run terminal state: any merged → succeeded (partial-success
    // path). All-blocked / nothing reviewed → failed. Empty selections
    // never reach this function (the empty-planning path returns earlier).
    let terminal_state = if merged_count > 0 {
        "succeeded"
    } else if reviewed_count == 0 && outcomes.iter().all(|o| o.review_body.is_none()) {
        "failed"
    } else {
        "failed"
    };

    let finished = persistence::finish_plan_run(db, &plan_run.id, terminal_state)
        .await
        .map_err(CoordinatorError::from_persistence)?;
    events.plan_run_completed(project_id, finished);

    let refreshed = persistence::get_plan_run(db, &plan_run.id)
        .await
        .map_err(CoordinatorError::from_persistence)?;
    Ok(refreshed)
}

async fn fail_planning_phase(
    db: &Db,
    events: &Arc<dyn EventPublisher>,
    project_id: &str,
    plan_run_id: &str,
    error: &str,
    problem_type: &str,
) -> CoordinatorError {
    let _ = persistence::record_plan_run_phase_output(
        db,
        plan_run_id,
        "planning",
        "failed",
        &serde_json::json!({ "error": error }),
    )
    .await;
    if let Ok(run) = persistence::finish_plan_run(db, plan_run_id, "failed").await {
        events.plan_run_completed(project_id, run);
    }
    CoordinatorError::new(500, problem_type, error)
}

// --- Prompt rendering ---------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn render_merge_prompt(
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

fn extract_review_findings(body: &serde_json::Value) -> String {
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

pub(crate) fn load_project_instructions(project_path: &str) -> String {
    for candidate in ["AGENTS.md", "CLAUDE.md", "PROJECT.md"] {
        if let Ok(text) =
            std::fs::read_to_string(std::path::Path::new(project_path).join(candidate))
        {
            return text;
        }
    }
    String::new()
}

#[allow(clippy::too_many_arguments)]
fn render_implementation_prompt(
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

#[allow(clippy::too_many_arguments)]
fn render_review_prompt(
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

// --- Issue Source lifecycle write-back ---------------------------------

/// Write a Lifecycle Status back to the upstream Issue Source for one
/// assignment. Supports `local_markdown` (file write) and `github`
/// (`gh issue edit` labels). Errors are surfaced as `String` so the caller
/// can log them without coupling to a specific error type; the coordinator
/// treats lifecycle write-back as best-effort and only logs warnings.
pub fn write_assignment_lifecycle(
    gh_binary_path: &Path,
    project: &ProjectResponse,
    source: &IssueSource,
    source_id: &str,
    lifecycle_status: &str,
) -> Result<(), String> {
    match source.kind.as_str() {
        "local_markdown" => {
            write_local_markdown_lifecycle(project, source, source_id, lifecycle_status).map(|_| ())
        }
        "github" => {
            write_github_lifecycle(gh_binary_path, &source.locator, source_id, lifecycle_status)
        }
        _ => Err(format!(
            "Lifecycle write-back is not supported for {} Issue Sources",
            source.kind
        )),
    }
}

fn write_local_markdown_lifecycle(
    project: &ProjectResponse,
    source: &IssueSource,
    source_id: &str,
    lifecycle_status: &str,
) -> Result<SourceIssueSnapshot, String> {
    let project_root = std::fs::canonicalize(&project.path)
        .map_err(|error| format!("failed to resolve Project path: {error}"))?;
    let source_dir = std::fs::canonicalize(project_root.join(&source.locator))
        .map_err(|error| format!("failed to read local markdown Issue Source: {error}"))?;
    if !source_dir.starts_with(&project_root) {
        return Err("local markdown Issue Source must be inside the Project path".to_string());
    }
    let issue_path = source_dir.join(format!("{source_id}.md"));
    let raw_text = std::fs::read_to_string(&issue_path)
        .map_err(|_| format!("Source Issue not found: {source_id}"))?;
    let updated_text = update_markdown_lifecycle_status(raw_text, lifecycle_status);
    std::fs::write(&issue_path, updated_text)
        .map_err(|error| format!("failed to write Source Issue file: {error}"))?;
    let updated_raw = std::fs::read_to_string(&issue_path)
        .map_err(|error| format!("failed to read updated Source Issue file: {error}"))?;
    Ok(parse_local_markdown_issue_minimal(
        source_id.to_string(),
        updated_raw,
    ))
}

fn write_github_lifecycle(
    gh_binary_path: &Path,
    locator: &str,
    source_id: &str,
    lifecycle_status: &str,
) -> Result<(), String> {
    let lifecycle_label = format!("agentic-afk:{lifecycle_status}");
    let output = Command::new(gh_binary_path)
        .args([
            "issue",
            "edit",
            source_id,
            "--repo",
            locator,
            "--remove-label",
            "agentic-afk:claimed",
            "--remove-label",
            "agentic-afk:running",
            "--remove-label",
            "agentic-afk:blocked",
            "--remove-label",
            "agentic-afk:completed",
            "--add-label",
            &lifecycle_label,
        ])
        .output()
        .map_err(|error| format!("failed to run GitHub lifecycle write-back: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "failed to write GitHub Issue lifecycle: {}",
            command_output(&output)
        ))
    }
}

fn command_output(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        format!("gh exited with status {}", output.status)
    } else {
        stdout
    }
}

/// Rewrite `Lifecycle Status:` in a Source Issue markdown body, inserting
/// it after the title line when no existing line is present.
pub fn update_markdown_lifecycle_status(raw_text: String, lifecycle_status: &str) -> String {
    let mut found = false;
    let mut lines: Vec<String> = raw_text
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.to_ascii_lowercase().starts_with("lifecycle status") && trimmed.contains(':')
            {
                found = true;
                let leading_ws = &line[..line.len() - line.trim_start().len()];
                format!("{}Lifecycle Status: {}", leading_ws, lifecycle_status)
            } else {
                line.to_string()
            }
        })
        .collect();

    if !found {
        // Insert after the first heading, or at the top if there is no heading.
        let insert_idx = lines
            .iter()
            .position(|line| line.trim_start().starts_with("# "))
            .map(|idx| {
                // Find the next blank line after the heading, or the end of the heading line.
                let after_heading = &lines[idx + 1..];
                after_heading
                    .iter()
                    .position(|l| l.trim().is_empty())
                    .map(|blank| idx + 1 + blank)
                    .unwrap_or(idx + 1)
            })
            .unwrap_or(0);
        let leading_ws = lines.get(insert_idx).map(|l| {
            let ws = &l[..l.len() - l.trim_start().len()];
            if ws.is_empty() {
                "\n".to_string()
            } else {
                ws.to_string()
            }
        });
        let new_line = format!("Lifecycle Status: {}", lifecycle_status);
        if let Some(ws) = leading_ws {
            lines.insert(insert_idx, new_line);
            lines.insert(insert_idx + 1, ws);
        } else {
            lines.push(new_line);
        }
    }

    lines.join("\n")
}

/// Minimal `SourceIssueSnapshot` constructor used after a lifecycle write
/// completes. Coordinator callers only need the basic identity for the
/// optional return value; full parsing remains in the server.
fn parse_local_markdown_issue_minimal(
    source_id: String,
    raw_text: String,
) -> SourceIssueSnapshot {
    SourceIssueSnapshot {
        source_id: source_id.clone(),
        title: source_id,
        readiness: "ready".to_string(),
        lifecycle_status: "ready".to_string(),
        parent_issue: None,
        issue_dependencies: Vec::new(),
        source_order: 0,
        raw_text,
    }
}

/// Reusable absolute path for the gh CLI. Avoids unused-import warnings
/// at top-level when callers do not want PathBuf.
pub type GhBinaryPath = PathBuf;

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_afk_contracts::ProjectId;

    fn test_project() -> agentic_afk_contracts::ProjectResponse {
        agentic_afk_contracts::ProjectResponse {
            id: ProjectId("p".to_string()),
            path: "/tmp/p".to_string(),
            trusted: true,
            git_summary: None,
            enabled_issue_source: Some(IssueSource {
                kind: "github".to_string(),
                locator: "owner/repo".to_string(),
            }),
        }
    }

    #[test]
    fn resolve_deps_passes_through_when_production_binaries_unset() {
        let deps = PlanRunDeps::default_test_deps();
        let resolved = resolve_deps_for_project(&deps, &test_project());
        // Production binaries None: planner runner should still be the
        // FakePlanningPhaseRunner default test deps installs.
        let stdout = resolved
            .planner
            .run("ignored")
            .expect("fake planner returns stdout");
        assert!(stdout.contains("<plan>"));
        // Lifecycle writer should still be the FakeLifecycleWriter, which
        // accepts writes without error.
        resolved
            .lifecycle
            .write_claimed("42")
            .expect("fake lifecycle writer accepts writes");
    }

    #[test]
    fn resolve_deps_swaps_in_production_codex_and_lifecycle_when_binaries_set() {
        let mut deps = PlanRunDeps::default_test_deps();
        deps.production_codex_binary = Some(PathBuf::from("/bin/true"));
        deps.production_gh_binary = Some(PathBuf::from("gh"));
        let resolved = resolve_deps_for_project(&deps, &test_project());
        // Production binaries set: lifecycle writer should now be the
        // GhLifecycleWriter, which dispatches to the canonical
        // `write_assignment_lifecycle` helper. Calling it for a
        // non-existent gh binary surfaces a LifecycleWrite error rather
        // than the FakeLifecycleWriter's silent Ok.
        let result = resolved.lifecycle.write_claimed("42");
        assert!(
            result.is_err(),
            "production lifecycle writer should surface gh failure, got {result:?}",
        );
    }
}
