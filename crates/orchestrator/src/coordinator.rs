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

use crate::implementation_phase::{check_implementation_outcome, render_implementation_prompt};
use crate::merge_phase::{
    AssignmentMergeOutcome, MergeRejection, decide_merge_outcome, render_merge_prompt,
};
use crate::plan_run_finalize::{PlanRunFinalize, decide_plan_run_terminal};
use crate::plan_run_status::{AssignmentStatus, transition_assignment};
use crate::planning_phase::{PlannedClaim, render_planning_prompt, validate_planner_selection};
use crate::review_loop::{ReviewLoopStep, decide_review_loop_step, render_review_prompt};
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
    /// Record a Project Activity entry as a best-effort write. Used by the
    /// Plan Run coordinator to surface post-claim **Lifecycle Status**
    /// write-back failures (per ADR-0035) so they appear in the Dashboard
    /// Activity feed instead of disappearing to stderr.
    fn record_activity(
        &self,
        project_id: &str,
        assignment_id: Option<&str>,
        kind: &str,
        detail: Option<&str>,
    );
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

/// Immutable identity and inputs of one Plan Run. Loaded once at the start
/// of `run_plan_run` and threaded through phase fns. Fns that take only
/// `&PlanRunInputs` cannot perform I/O — that property lets pure decision
/// fns prove their purity at the type level.
#[derive(Clone)]
pub struct PlanRunInputs {
    pub project: ProjectResponse,
    pub plan_run: PlanRunResponse,
    pub baseline: RefreshedBaseline,
    pub exec_config: ProjectExecutionConfigResponse,
    /// Cached contents of the Project's instructions file (AGENTS.md /
    /// CLAUDE.md / PROJECT.md). Loaded once so every phase prompt uses the
    /// same text without re-reading the file mid-run.
    pub project_instructions: String,
}

impl PlanRunInputs {
    pub fn new(
        project: ProjectResponse,
        plan_run: PlanRunResponse,
        baseline: RefreshedBaseline,
        exec_config: ProjectExecutionConfigResponse,
    ) -> Self {
        let project_instructions = load_project_instructions(&project.path);
        Self {
            project,
            plan_run,
            baseline,
            exec_config,
            project_instructions,
        }
    }

    /// Stable Project identifier used as the `project_id` argument
    /// everywhere persistence and event publishing reach for it.
    pub fn project_id(&self) -> &str {
        self.project.id.0.as_str()
    }
}

/// Mutable collaborators (database, event bus, phase-runner adapters,
/// external binary paths) used while running a Plan Run. Grouping these
/// behind one struct keeps `run_plan_run` callsites readable and lets
/// helpers like `transition_assignment` take a single effects reference
/// instead of three.
#[derive(Clone)]
pub struct PlanRunEffects {
    pub db: Db,
    pub events: Arc<dyn EventPublisher>,
    pub deps: PlanRunDeps,
    pub gh_binary_path: PathBuf,
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
pub async fn run_plan_run(
    inputs: &PlanRunInputs,
    effects: &PlanRunEffects,
) -> Result<PlanRunResponse, CoordinatorError> {
    // Resolve production runners against the per-Plan-Run Project context
    // (codex binary cwd + Issue Source kind/locator) when the caller wired
    // production binaries. Tests with injected fakes flow through
    // unchanged because `production_codex_binary` / `production_gh_binary`
    // remain `None`.
    let resolved_deps = resolve_deps_for_project(&effects.deps, &inputs.project);
    let effects = &PlanRunEffects {
        deps: resolved_deps,
        ..effects.clone()
    };
    let deps = &effects.deps;
    let db = &effects.db;
    let events = &effects.events;
    let project = &inputs.project;
    let project_id = inputs.project_id();
    let plan_run = &inputs.plan_run;
    let baseline = &inputs.baseline;
    let execution_config = &inputs.exec_config;
    let eligible = persistence::get_planning_snapshot(db, project_id)
        .await
        .map(|raw| agentic_afk_planning_snapshot::normalize(raw).eligible)
        .unwrap_or_default();
    let prompt = render_planning_prompt(
        &inputs.project_instructions,
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
        project,
        project_id,
        plan_run,
        baseline,
        &parsed,
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
    project: &ProjectResponse,
    project_id: &str,
    plan_run: &PlanRunResponse,
    baseline: &RefreshedBaseline,
    parsed: &crate::ParsedPlanningOutput,
) -> Result<PlanRunResponse, CoordinatorError> {
    let snapshot = match persistence::get_planning_snapshot(db, project_id).await {
        Ok(raw) => agentic_afk_planning_snapshot::normalize(raw),
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

    // Apply the pure `validate_planner_selection` decision. Typed
    // rejections (Unparseable / ExceedsMaxParallel / IneligibleSelection /
    // MissingIssueSource) carry their RFC-7807 problem-type URN via
    // `From<PlanningRejection> for CoordinatorError`, so this single call
    // replaces four inline guard branches the coordinator used to spell
    // out by hand.
    let claims = match validate_planner_selection(
        parsed,
        &snapshot.eligible,
        exec_config_lookup.max_parallel_tasks,
        project.enabled_issue_source.is_some(),
    ) {
        Ok(claims) => claims,
        Err(rejection) => {
            let err = CoordinatorError::from(rejection);
            return Err(fail_planning_phase(
                db,
                events,
                project_id,
                &plan_run.id,
                &err.detail,
                &err.problem_type,
            )
            .await);
        }
    };

    // SAFETY: validate_planner_selection rejected with MissingIssueSource
    // above if this was None. Cloning the source once for the loop is
    // cheaper than threading it through the rejection arm.
    let source = project
        .enabled_issue_source
        .clone()
        .expect("validate_planner_selection ensured an enabled Issue Source");

    // Provision all assignments sequentially to preserve deterministic
    // ordering in Plan Run snapshots. Each row is created, worktree
    // provisioned, lifecycle written back, and an `AssignmentCreated`
    // event published before any implementation pass starts. This keeps
    // the parallel tranche observable from the Dashboard immediately.
    let mut claimed: Vec<IssueAssignmentResponse> = Vec::with_capacity(claims.len());
    let project_path = std::path::Path::new(&project.path);
    for claim in &claims {
        let PlannedClaim {
            selection,
            eligible_issue,
        } = claim;
        let assignment = persistence::create_plan_run_assignment(
            db,
            &plan_run.id,
            project_id,
            &source,
            eligible_issue,
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

        if let Err(error) = deps
            .lifecycle
            .write(&eligible_issue.source_id, crate::LifecycleStatus::Claimed)
        {
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
        &parsed.body,
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
    loop {
        transition_assignment(
            db,
            events,
            project_id,
            &assignment.id,
            AssignmentStatus::Implementing,
        )
        .await?;

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
        if let Err(rejection) = check_implementation_outcome(&impl_parsed.outcome) {
            let err = CoordinatorError::from(rejection);
            return Err(fail_assignment_phase(
                db,
                events,
                project_id,
                assignment,
                "implementation",
                &err.detail,
                &err.problem_type,
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

        transition_assignment(
            db,
            events,
            project_id,
            &assignment.id,
            AssignmentStatus::Implemented,
        )
        .await?;

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

        // Increment the persisted rejection counter *before* asking
        // `decide_review_loop_step` what to do so the limit guard fires
        // deterministically. Approved outcomes skip the increment.
        let rejection_count = if review_parsed.outcome == "approved" {
            0
        } else {
            persistence::increment_review_rejection(db, &assignment.id)
                .await
                .map_err(CoordinatorError::from_persistence)?
        };
        match decide_review_loop_step(
            &review_parsed,
            rejection_count,
            exec_config.review_retry_limit,
        ) {
            ReviewLoopStep::Approved { review_body } => {
                transition_assignment(
                    db,
                    events,
                    project_id,
                    &assignment.id,
                    AssignmentStatus::Reviewed,
                )
                .await?;
                return Ok(Some(review_body));
            }
            ReviewLoopStep::Retry { findings } => {
                review_findings = findings;
            }
            ReviewLoopStep::Block { reason } => {
                block_assignment_for_loop(
                    db,
                    events,
                    deps,
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
}

/// Block an assignment that exhausted its Review Loop without finishing
/// the surrounding Plan Run. Used by the parallel tranche so blocked
/// assignments stay outside the Merge Phase while reviewed peers continue.
async fn block_assignment_for_loop(
    db: &Db,
    events: &Arc<dyn EventPublisher>,
    deps: &PlanRunDeps,
    project: &ProjectResponse,
    project_id: &str,
    assignment: &IssueAssignmentResponse,
    reason: &str,
) -> Result<(), CoordinatorError> {
    transition_assignment(
        db,
        events,
        project_id,
        &assignment.id,
        AssignmentStatus::Blocked {
            kind: agentic_afk_contracts::BlockReason::ReviewRetryLimitExhausted,
            detail: reason.to_string(),
        },
    )
    .await?;
    if project.enabled_issue_source.is_some() {
        if let Err(error) = deps
            .lifecycle
            .write(&assignment.source_id, crate::LifecycleStatus::Blocked)
        {
            // Per ADR-0035, post-claim lifecycle write-back is
            // best-effort. Surface the failure as Project Activity so the
            // developer notices through the Dashboard rather than stderr.
            events.record_activity(
                project_id,
                Some(&assignment.id),
                "lifecycle_writeback_failed",
                Some(&format!(
                    "blocked Lifecycle Status write-back failed: {error}"
                )),
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
    project: &ProjectResponse,
    project_id: &str,
    plan_run: &PlanRunResponse,
    baseline: &RefreshedBaseline,
    exec_config: &ProjectExecutionConfigResponse,
    outcomes: Vec<AssignmentBatchOutcome>,
) -> Result<PlanRunResponse, CoordinatorError> {
    let project_instructions = load_project_instructions(&project.path);
    let project_path = std::path::Path::new(&project.path);

    let mut merge_outcomes: Vec<AssignmentMergeOutcome> = Vec::with_capacity(outcomes.len());
    let mut blocked_assignments: Vec<IssueAssignmentResponse> = Vec::new();
    let mut merged_assignments: Vec<IssueAssignmentResponse> = Vec::new();

    for outcome in &outcomes {
        let Some(review_body) = outcome.review_body.as_ref() else {
            blocked_assignments.push(outcome.assignment.clone());
            merge_outcomes.push(AssignmentMergeOutcome::NotAttempted);
            continue;
        };

        let merge_assignment = outcome.assignment.clone();
        transition_assignment(
            db,
            events,
            project_id,
            &merge_assignment.id,
            AssignmentStatus::Merging,
        )
        .await?;

        let prompt = render_merge_prompt(
            &project_instructions,
            project,
            plan_run,
            baseline,
            exec_config,
            &merge_assignment,
            review_body,
        );

        // Classify the merge attempt as one of Merged / Blocked. Runner
        // failures and unparseable outputs collapse to Blocked with the
        // surfaced error; a parsed merge body flows through the pure
        // `decide_merge_outcome` for the Merged-vs-Blocked discriminator.
        let merge_outcome: AssignmentMergeOutcome = match deps.merger.run(&prompt) {
            Err(error) => {
                let reason = error.to_string();
                let _ = persistence::record_assignment_phase_output(
                    db,
                    &plan_run.id,
                    &merge_assignment.id,
                    "merge",
                    "failed",
                    &serde_json::json!({ "error": &reason }),
                )
                .await;
                AssignmentMergeOutcome::Blocked { reason }
            }
            Ok(merge_stdout) => match crate::parse_merge_output(&merge_stdout) {
                Err(error) => {
                    let _ = persistence::record_assignment_phase_output(
                        db,
                        &plan_run.id,
                        &merge_assignment.id,
                        "merge",
                        "failed",
                        &serde_json::json!({ "error": &error }),
                    )
                    .await;
                    let err = CoordinatorError::from(MergeRejection::Unparseable(error));
                    AssignmentMergeOutcome::Blocked { reason: err.detail }
                }
                Ok(parsed) => {
                    let merge_output = persistence::record_assignment_phase_output(
                        db,
                        &plan_run.id,
                        &merge_assignment.id,
                        "merge",
                        &parsed.outcome,
                        &parsed.body,
                    )
                    .await
                    .map_err(CoordinatorError::from_persistence)?;
                    events.plan_run_phase_completed(project_id, &plan_run.id, merge_output);
                    decide_merge_outcome(&parsed)
                }
            },
        };

        match &merge_outcome {
            AssignmentMergeOutcome::Merged => {
                // ADR-0037: after local integration + verification the
                // assignment transitions to `merge_staged` and stays there
                // until the Integration Branch push succeeds. Only a
                // successful push advances `merge_staged` → `merged`; a
                // push failure leaves the assignment dormant at
                // `merge_staged` for operator recovery.
                let staged = transition_assignment(
                    db,
                    events,
                    project_id,
                    &merge_assignment.id,
                    AssignmentStatus::MergeStaged,
                )
                .await?;
                merged_assignments.push(staged);
            }
            AssignmentMergeOutcome::Blocked { reason } => {
                let blocked = transition_assignment(
                    db,
                    events,
                    project_id,
                    &merge_assignment.id,
                    AssignmentStatus::Blocked {
                        kind: agentic_afk_contracts::BlockReason::MergePhaseFailed,
                        detail: reason.clone(),
                    },
                )
                .await?;
                if project.enabled_issue_source.is_some() {
                    if let Err(error) = deps
                        .lifecycle
                        .write(&merge_assignment.source_id, crate::LifecycleStatus::Blocked)
                    {
                        events.record_activity(
                            project_id,
                            Some(&merge_assignment.id),
                            "lifecycle_writeback_failed",
                            Some(&format!(
                                "blocked Lifecycle Status write-back during Merge Phase failed: {error}"
                            )),
                        );
                    }
                }
                blocked_assignments.push(blocked);
            }
            AssignmentMergeOutcome::NotAttempted => unreachable!(
                "NotAttempted is only produced by the no-review-body branch above"
            ),
        }

        merge_outcomes.push(merge_outcome);
    }

    // Push the Integration Branch once if at least one merge succeeded.
    // The push is part of the canonical merge boundary: blocked merges
    // never push, and the push happens after every merge attempt has
    // settled so the upstream only sees the final integrated tree.
    //
    // ADR-0037: each successfully-integrated assignment is currently at
    // `merge_staged`. Push success advances `merge_staged` → `merged` and
    // proceeds with the Lifecycle `Completed` write-back. Push failure
    // leaves the assignment at `merge_staged` (dormant, awaiting operator
    // Retry Push / Abandon Staged) and finishes the Plan Run as `failed`.
    let any_merged = merge_outcomes
        .iter()
        .any(|o| matches!(o, AssignmentMergeOutcome::Merged));
    // Assignments that successfully advanced to `merged` after a verified
    // push. Distinct from `merged_assignments` (currently `merge_staged`)
    // so cleanup can gate on terminal status per ADR-0037.
    let mut pushed_assignments: Vec<IssueAssignmentResponse> = Vec::new();
    // Assignments that remained at `merge_staged` because the push failed.
    // Worktree and issue-branch cleanup is deferred until they reach a
    // terminal status (`merged` or `blocked`).
    let mut staged_assignments: Vec<IssueAssignmentResponse> = Vec::new();
    let mut push_failed = false;
    if any_merged {
        if let Err(error) = deps
            .pusher
            .push(project_path, &exec_config.integration_branch)
        {
            // Push failure (ADR-0037): record the push failure as a
            // failed merge Phase Output for each staged assignment, leave
            // each assignment at `merge_staged` (no transition), defer
            // worktree cleanup, and finish the Plan Run as `failed`.
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
            staged_assignments.extend(merged_assignments.iter().cloned());
            push_failed = true;
        } else {
            // Push succeeded: advance every staged assignment to `merged`
            // and write the Source Issue Lifecycle `Completed` back. The
            // lifecycle write-back happens only after the verified push so
            // upstream state never claims work the developer did not
            // actually receive.
            for staged in &merged_assignments {
                let merged = transition_assignment(
                    db,
                    events,
                    project_id,
                    &staged.id,
                    AssignmentStatus::Merged,
                )
                .await?;
                pushed_assignments.push(merged);
            }
            if project.enabled_issue_source.is_some() {
                for assignment in &pushed_assignments {
                    if let Err(error) = deps
                        .lifecycle
                        .write(&assignment.source_id, crate::LifecycleStatus::Completed)
                    {
                        events.record_activity(
                            project_id,
                            Some(&assignment.id),
                            "lifecycle_writeback_failed",
                            Some(&format!(
                                "completed Lifecycle Status write-back after push failed: {error}"
                            )),
                        );
                    }
                }
            }
        }
    }

    // Cleanup gating (ADR-0037): only assignments in a terminal Assignment
    // Status (`merged` or `blocked`) have their worktrees and issue
    // branches cleaned at Plan Run finish. Dormant `merge_staged`
    // assignments retain their worktree and issue branch so operator
    // recovery (Retry Push / Abandon Staged) can act on them.
    let mut cleanup_targets: Vec<IssueAssignmentResponse> = Vec::new();
    cleanup_targets.extend(pushed_assignments.iter().cloned());
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

    // Plan Run terminal state via the pure `decide_plan_run_terminal`
    // decision. Empty-planning selections never reach this function (the
    // empty-planning path returns earlier in `finalize_empty_planning`),
    // so `planning_was_empty` is always `false` here.
    //
    // ADR-0037: a push failure leaves staged assignments at `merge_staged`
    // and the Plan Run finishes `failed` regardless of how many
    // assignments locally integrated. The pure decision treats any
    // `Merged` outcome as `Succeeded`, so we override on push failure.
    let terminal = if push_failed {
        crate::plan_run_finalize::PlanRunTerminal::Failed
    } else {
        decide_plan_run_terminal(&PlanRunFinalize {
            planning_was_empty: false,
            outcomes: merge_outcomes,
        })
    };
    // Suppress unused warning when push_failed shortcut is taken.
    let _ = &staged_assignments;
    let finished = persistence::finish_plan_run(db, &plan_run.id, terminal.as_str())
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

// --- Issue Source lifecycle write-back ---------------------------------

/// Write a Lifecycle Status back to the upstream Issue Source for one
/// assignment. Supports `local_markdown` (file write) and `github`
/// (`gh issue edit` labels). Errors are surfaced as `String` so the caller
/// can log them without coupling to a specific error type; the coordinator
/// treats lifecycle write-back as best-effort and only logs warnings.
pub(crate) fn write_assignment_lifecycle(
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
            .write("42", crate::LifecycleStatus::Claimed)
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
        let result = resolved
            .lifecycle
            .write("42", crate::LifecycleStatus::Claimed);
        assert!(
            result.is_err(),
            "production lifecycle writer should surface gh failure, got {result:?}",
        );
    }
}
