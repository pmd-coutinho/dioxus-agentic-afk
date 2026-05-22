use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use agentic_afk_contracts::{
    AppInfoResponse, AssignmentAttemptResponse, AssignmentTerminalOutcome, CreateProjectRequest,
    EffectiveConfig, EnableIssueSourceRequest, HealthResponse, IssueAssignmentResponse,
    IssueSource, IssueSourceCandidate, IssueSourceSyncResponse, IssueSourceSyncStatusResponse,
    PlanRunResponse, PlanningSnapshotResponse, ProblemDetail, ProjectActivityEntryResponse,
    ProjectEvent, ProjectExecutionConfigResponse, ProjectId, ProjectResponse, ProjectSnapshot,
    ProjectSnapshotResponse, SetProjectExecutionConfigRequest, SourceIssueSnapshot,
};
use agentic_afk_git_summary::summarize_project_path;
pub use agentic_afk_orchestrator::{
    AssignmentWorktreeCleaner, AssignmentWorktreeProvisioner, FakeAssignmentWorktreeCleaner,
    FakeImplementationPhaseRunner, FakeIntegrationBranchPusher, FakeLifecycleWriter,
    FakeMergePhaseRunner, FakePlanningPhaseRunner, FakeReviewPhaseRunner,
    FakeWorktreeProvisioner, GitAssignmentWorktreeCleaner, GitIntegrationBranchPusher,
    GitIntegrationBranchRefresher, ImplementationPhaseRunner, IntegrationBranchPusher,
    IntegrationBranchRefresher, IssueLifecycleWriter, MergePhaseRunner,
    PerSourceImplementationPhaseRunner, PerSourceMergePhaseRunner, PerSourceReviewPhaseRunner,
    PlanRunPhaseError, PlannerSelection, PlanningPhaseRunner, RefreshedBaseline,
    ReviewPhaseRunner, StaticIntegrationBranchRefresher, UnimplementedAssignmentWorktreeCleaner,
    UnimplementedImplementationPhaseRunner, UnimplementedIntegrationBranchPusher,
    UnimplementedIntegrationBranchRefresher, UnimplementedLifecycleWriter,
    UnimplementedMergePhaseRunner, UnimplementedPlanningPhaseRunner,
    UnimplementedReviewPhaseRunner, UnimplementedWorktreeProvisioner,
    WorktrunkAssignmentWorktreeProvisioner,
};
use agentic_afk_persistence::{self as persistence, Db, PersistenceError};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::process::Command;
use tower_http::services::{ServeDir, ServeFile};
use utoipa::OpenApi;
use utoipa::ToSchema;

pub mod activity_publisher;
pub mod event_bus;
pub mod project_event_publisher;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControlPlaneConfig {
    pub bind_address: SocketAddr,
    pub dashboard_asset_dir: PathBuf,
    pub database_url: String,
    pub gh_binary_path: PathBuf,
    pub worktrunk_binary_path: PathBuf,
    pub codex_binary_path: PathBuf,
}

impl ControlPlaneConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let bind_address = std::env::var("AGENTIC_AFK_BIND_ADDRESS")
            .unwrap_or_else(|_| "127.0.0.1:3637".to_string())
            .parse()?;
        let dashboard_asset_dir = std::env::var("AGENTIC_AFK_DASHBOARD_ASSET_DIR")
            .unwrap_or_else(|_| {
                "target/dx/agentic-afk-dashboard/release/web/public".to_string()
            })
            .into();
        let database_url = std::env::var("AGENTIC_AFK_DATABASE_URL")
            .unwrap_or_else(|_| "sqlite://agentic-afk.db".to_string());
        let gh_binary_path = std::env::var("AGENTIC_AFK_GH_BIN")
            .unwrap_or_else(|_| "gh".to_string())
            .into();
        let worktrunk_binary_path = std::env::var("AGENTIC_AFK_WORKTRUNK_BIN")
            .unwrap_or_else(|_| "wt".to_string())
            .into();
        let codex_binary_path = std::env::var("AGENTIC_AFK_CODEX_BIN")
            .unwrap_or_else(|_| "codex".to_string())
            .into();

        Ok(Self {
            bind_address,
            dashboard_asset_dir,
            database_url,
            gh_binary_path,
            worktrunk_binary_path,
            codex_binary_path,
        })
    }

    fn effective_config(&self) -> EffectiveConfig {
        EffectiveConfig {
            bind_address: self.bind_address.to_string(),
            dashboard_asset_dir: self.dashboard_asset_dir.display().to_string(),
            database_url: self.database_url.clone(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) config: ControlPlaneConfig,
    pub(crate) db: Db,
    pub(crate) event_bus: event_bus::EventBus,
    pub(crate) plan_run_deps: PlanRunDeps,
}

/// Plan Run phase dependencies wired into the router. Tests inject fakes;
/// production wires real-git / Codex implementations (placeholder until
/// later ADR-0034 slices land).
#[derive(Clone)]
pub struct PlanRunDeps {
    pub refresher: Arc<dyn IntegrationBranchRefresher>,
    pub planner: Arc<dyn PlanningPhaseRunner>,
    pub worktree: Arc<dyn AssignmentWorktreeProvisioner>,
    pub lifecycle: Arc<dyn IssueLifecycleWriter>,
    pub implementation: Arc<dyn ImplementationPhaseRunner>,
    pub review: Arc<dyn ReviewPhaseRunner>,
    /// Merge Phase runner (issue #45). Integrates one reviewed Issue
    /// Assignment into the configured Integration Branch.
    pub merger: Arc<dyn MergePhaseRunner>,
    /// Push the Integration Branch after a verified successful merge.
    /// Held as a separate seam from the merge runner so the push
    /// boundary can be asserted independently in tests.
    pub pusher: Arc<dyn IntegrationBranchPusher>,
    /// Clean up the Assignment Worktree and deterministic branch after a
    /// successful merge so the worktree does not linger past completion.
    pub cleaner: Arc<dyn AssignmentWorktreeCleaner>,
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
        }
    }

    /// Wire the project-agnostic Plan Run seams to real implementations
    /// for production. The four phase runners (planning, implementation,
    /// review, merge) and the Issue Source lifecycle writer remain
    /// `Unimplemented` here because their construction requires per-project
    /// context (codex binary path bound to the Project's path, Issue
    /// Source kind/locator) that the current `PlanRunDeps` shape does not
    /// carry. Closing that last seam needs the coordinator-extraction
    /// refactor (Gap 5) so the coordinator can build the runners per Plan
    /// Run; until then this wiring delivers real Integration Branch
    /// refresh, worktree provisioning, push, and cleanup against the
    /// developer's working tree.
    pub fn production(worktrunk_binary_path: PathBuf) -> Self {
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
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct SetLifecycleStatusRequest {
    pub lifecycle_status: String,
}

#[derive(OpenApi)]
#[openapi(
    paths(
        health,
        app_info,
        create_project,
        list_projects,
        get_project,
        trust_project,
        list_issue_source_candidates,
        enable_issue_source,
        get_issue_source_sync_status,
        sync_issue_source,
        get_planning_snapshot,
        update_lifecycle_status,
        get_project_activity,
        get_project_snapshot
    ),
    components(schemas(
        HealthResponse,
        AppInfoResponse,
        EffectiveConfig,
        CreateProjectRequest,
        EnableIssueSourceRequest,
        SetLifecycleStatusRequest,
        ProjectResponse,
        IssueSource,
        IssueSourceCandidate,
        IssueSourceSyncResponse,
        IssueSourceSyncStatusResponse,
        PlanningSnapshotResponse,
        IssueAssignmentResponse,
        AssignmentAttemptResponse,
        AssignmentTerminalOutcome,
        SourceIssueSnapshot,
        ProjectActivityEntryResponse,
        ProjectSnapshot,
        ProjectSnapshotResponse,
        ProjectEvent,
        ProblemDetail
    )),
    tags((name = "Local Control Plane", description = "Local Control Plane API"))
)]
struct ApiDoc;

pub fn router(config: ControlPlaneConfig, db: Db) -> Router {
    router_with_bus(config, db, event_bus::EventBus::new())
}

fn plan_run_deps_from_env() -> PlanRunDeps {
    let mode = std::env::var("AGENTIC_AFK_TEST_PLAN_RUN_STUBS").ok();
    let Some(mode) = mode else {
        // Production wiring: real git refresher / pusher / cleaner and
        // Worktrunk-backed provisioner. The four Codex phase runners and
        // the Issue Source lifecycle writer remain Unimplemented placeholders
        // pending the coordinator-extraction refactor described in PRD #40
        // validation gap 5 (Plan Run coordinator lives in HTTP handler).
        let worktrunk = std::env::var("AGENTIC_AFK_WORKTRUNK_BIN")
            .unwrap_or_else(|_| "wt".to_string());
        return PlanRunDeps::production(worktrunk.into());
    };
    let stdout = match mode.as_str() {
        "1" => r#"<plan>{"issues":[],"summary":"test stub: no eligible work"}</plan>"#.to_string(),
        // `select=<source_id>` mode: planner selects the given Source Issue
        // with a derived branch name and selection summary. The driving
        // test must seed the planning snapshot so the selection is eligible.
        other if other.starts_with("select=") => {
            let source_id = &other["select=".len()..];
            format!(
                r#"<plan>{{"issues":[{{"source_issue_id":"{sid}","title":"stub {sid}","branch":"agent/issue-{sid}","selection_summary":"stub selection"}}],"summary":"test stub: select one"}}</plan>"#,
                sid = source_id,
            )
        }
        _ => r#"<plan>{"issues":[],"summary":"test stub: no eligible work"}</plan>"#.to_string(),
    };
    let refresher = Arc::new(StaticIntegrationBranchRefresher::new(RefreshedBaseline {
        commit_sha: "test-baseline".to_string(),
    }));
    let planner = Arc::new(FakePlanningPhaseRunner::with_stdout(stdout));
    PlanRunDeps {
        refresher,
        planner,
        ..default_test_deps()
    }
}

/// Default fakes for the worktree/lifecycle/implementation/review seams,
/// used by the `*_with_plan_run_deps*` builders so existing callers don't
/// have to wire the full graph.
fn default_test_deps() -> PlanRunDeps {
    PlanRunDeps {
        refresher: Arc::new(StaticIntegrationBranchRefresher::new(RefreshedBaseline {
            commit_sha: "test-baseline".to_string(),
        })),
        planner: Arc::new(FakePlanningPhaseRunner::with_stdout(
            r#"<plan>{"issues":[],"summary":"none"}</plan>"#,
        )),
        worktree: Arc::new(FakeWorktreeProvisioner::new(std::env::temp_dir().join(
            "agentic-afk-test-worktrees",
        ))),
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
    }
}

/// Build a router that shares a caller-provided `EventBus`. Useful for
/// tests that need to publish activity through the same bus the SSE
/// endpoint subscribes from.
pub fn router_with_bus(config: ControlPlaneConfig, db: Db, event_bus: event_bus::EventBus) -> Router {
    router_with_full_deps(config, db, event_bus, plan_run_deps_from_env())
}

/// Build a router with caller-provided Plan Run phase dependencies. Tests
/// pass fakes; production wires real-git / Codex impls (later slices).
pub fn router_with_plan_run_deps(
    config: ControlPlaneConfig,
    db: Db,
    refresher: Arc<dyn IntegrationBranchRefresher>,
    planner: Arc<dyn PlanningPhaseRunner>,
) -> Router {
    router_with_full_deps(
        config,
        db,
        event_bus::EventBus::new(),
        PlanRunDeps {
            refresher,
            planner,
            ..default_test_deps()
        },
    )
}

/// Same as `router_with_plan_run_deps` but exposes the `EventBus` for tests
/// that want to subscribe to live SSE deltas through the same channel the
/// router publishes to.
pub fn router_with_plan_run_deps_and_bus(
    config: ControlPlaneConfig,
    db: Db,
    event_bus: event_bus::EventBus,
    refresher: Arc<dyn IntegrationBranchRefresher>,
    planner: Arc<dyn PlanningPhaseRunner>,
) -> Router {
    router_with_full_deps(
        config,
        db,
        event_bus,
        PlanRunDeps {
            refresher,
            planner,
            ..default_test_deps()
        },
    )
}

/// Build a router with the full Plan Run dependency surface, including the
/// Assignment Worktree provisioner and Issue Source lifecycle writer used
/// by the claim path. Tests that exercise the claim flow drive this entry
/// point so they can inject fakes for every seam.
pub fn router_with_plan_run_full_deps(
    config: ControlPlaneConfig,
    db: Db,
    refresher: Arc<dyn IntegrationBranchRefresher>,
    planner: Arc<dyn PlanningPhaseRunner>,
    worktree: Arc<dyn AssignmentWorktreeProvisioner>,
    lifecycle: Arc<dyn IssueLifecycleWriter>,
) -> Router {
    router_with_plan_run_all_deps(
        config,
        db,
        refresher,
        planner,
        worktree,
        lifecycle,
        Arc::new(FakeImplementationPhaseRunner::with_stdout(
            r#"<impl>{"outcome":"ready_for_review","summary":"stub","commits":[],"verification":[],"gaps":[]}</impl>"#,
        )),
        Arc::new(FakeReviewPhaseRunner::with_stdout(
            r#"<review>{"outcome":"approved","findings":[],"summary":"stub approved","verification":[],"gaps":[]}</review>"#,
        )),
    )
}

/// Build a router with every Plan Run dependency seam injected
/// (planning, claim, implementation, review). The merge / push / cleanup
/// seams default to test fakes so existing tests continue to compile.
pub fn router_with_plan_run_all_deps(
    config: ControlPlaneConfig,
    db: Db,
    refresher: Arc<dyn IntegrationBranchRefresher>,
    planner: Arc<dyn PlanningPhaseRunner>,
    worktree: Arc<dyn AssignmentWorktreeProvisioner>,
    lifecycle: Arc<dyn IssueLifecycleWriter>,
    implementation: Arc<dyn ImplementationPhaseRunner>,
    review: Arc<dyn ReviewPhaseRunner>,
) -> Router {
    router_with_full_deps(
        config,
        db,
        event_bus::EventBus::new(),
        PlanRunDeps {
            refresher,
            planner,
            worktree,
            lifecycle,
            implementation,
            review,
            ..default_test_deps()
        },
    )
}

/// Build a router with the full Plan Run dependency surface, including
/// the Merge Phase seams (issue #45). Used by tests that drive the
/// implementation → review → merge flow end-to-end.
#[allow(clippy::too_many_arguments)]
pub fn router_with_plan_run_merge_deps(
    config: ControlPlaneConfig,
    db: Db,
    refresher: Arc<dyn IntegrationBranchRefresher>,
    planner: Arc<dyn PlanningPhaseRunner>,
    worktree: Arc<dyn AssignmentWorktreeProvisioner>,
    lifecycle: Arc<dyn IssueLifecycleWriter>,
    implementation: Arc<dyn ImplementationPhaseRunner>,
    review: Arc<dyn ReviewPhaseRunner>,
    merger: Arc<dyn MergePhaseRunner>,
    pusher: Arc<dyn IntegrationBranchPusher>,
    cleaner: Arc<dyn AssignmentWorktreeCleaner>,
) -> Router {
    router_with_full_deps(
        config,
        db,
        event_bus::EventBus::new(),
        PlanRunDeps {
            refresher,
            planner,
            worktree,
            lifecycle,
            implementation,
            review,
            merger,
            pusher,
            cleaner,
        },
    )
}

fn router_with_full_deps(
    config: ControlPlaneConfig,
    db: Db,
    event_bus: event_bus::EventBus,
    plan_run_deps: PlanRunDeps,
) -> Router {
    let asset_dir = config.dashboard_asset_dir.clone();
    let index = asset_dir.join("index.html");
    let state = Arc::new(AppState {
        config,
        db,
        event_bus,
        plan_run_deps,
    });

    Router::new()
        .route("/health", get(health))
        .route("/api/app-info", get(app_info))
        .route("/api/openapi.json", get(openapi_json))
        .route("/api/docs", get(api_docs))
        .route("/api/projects", post(create_project).get(list_projects))
        .route("/api/projects/{id}", get(get_project))
        .route(
            "/api/projects/{id}/trust",
            axum::routing::put(trust_project),
        )
        .route(
            "/api/projects/{id}/issue-source-candidates",
            get(list_issue_source_candidates),
        )
        .route(
            "/api/projects/{id}/issue-source",
            axum::routing::put(enable_issue_source),
        )
        .route(
            "/api/projects/{id}/issue-source/sync",
            post(sync_issue_source),
        )
        .route(
            "/api/projects/{id}/issue-source/sync-status",
            get(get_issue_source_sync_status),
        )
        .route(
            "/api/projects/{id}/planning-snapshot",
            get(get_planning_snapshot),
        )
        .route("/api/projects/{id}/activity", get(get_project_activity))
        .route("/api/projects/{id}/snapshot", get(get_project_snapshot))
        .route("/api/projects/{id}/events", get(get_project_events))
        .merge(test_endpoints_router())
        .route(
            "/api/projects/{id}/source-issues/{source_id}/lifecycle-status",
            axum::routing::put(update_lifecycle_status),
        )
        .route(
            "/api/projects/{id}/execution-config",
            axum::routing::put(set_execution_config),
        )
        .route(
            "/api/projects/{id}/plan-runs",
            post(start_plan_run).get(list_plan_runs),
        )
        .route(
            "/api/projects/{id}/assignments/{assignment_id}/re-enable",
            post(re_enable_assignment),
        )
        .route("/api/{*path}", get(api_not_found).post(api_not_found))
        .fallback_service(ServeDir::new(asset_dir).fallback(ServeFile::new(index)))
        .with_state(state)
}

pub async fn serve(config: ControlPlaneConfig) -> anyhow::Result<()> {
    let db = persistence::connect(&config.database_url).await?;
    persistence::migrate(&db).await?;

    let listener = tokio::net::TcpListener::bind(config.bind_address).await?;
    let local_addr = listener.local_addr()?;
    eprintln!("agentic-afk Local Control Plane listening on http://{local_addr}");
    axum::serve(listener, router(config, db))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// Run database migrations.
pub async fn run_migrate(database_url: &str) -> anyhow::Result<()> {
    let db = persistence::connect(database_url).await?;
    persistence::migrate(&db).await?;
    eprintln!("Migrations applied successfully.");
    Ok(())
}

/// Seed the development Project idempotently.
pub async fn run_seed_dev(database_url: &str, dev_path: &str) -> anyhow::Result<()> {
    let db = persistence::connect(database_url).await?;
    persistence::migrate(&db).await?;
    let project = persistence::seed_dev_project(&db, dev_path).await?;
    eprintln!("Dev project seeded: {} -> {}", project.id.0, project.path);
    Ok(())
}

#[utoipa::path(get, path = "/health", responses((status = OK, body = HealthResponse)))]
async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
    })
}

#[utoipa::path(get, path = "/api/app-info", responses((status = OK, body = AppInfoResponse)))]
async fn app_info(State(state): State<Arc<AppState>>) -> Json<AppInfoResponse> {
    Json(AppInfoResponse {
        app_name: "agentic-afk".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        api_status: "connected".to_string(),
        config: state.config.effective_config(),
    })
}

#[utoipa::path(
    post,
    path = "/api/projects",
    request_body = CreateProjectRequest,
    responses(
        (status = CREATED, body = ProjectResponse),
        (status = CONFLICT, body = ProblemDetail, content_type = "application/problem+json"),
        (status = UNPROCESSABLE_ENTITY, body = ProblemDetail, content_type = "application/problem+json"),
        (status = INTERNAL_SERVER_ERROR, body = ProblemDetail, content_type = "application/problem+json")
    )
)]
async fn create_project(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateProjectRequest>,
) -> Response {
    match persistence::create_project(&state.db, &request).await {
        Ok(project) => (StatusCode::CREATED, Json(with_git_summary(project))).into_response(),
        Err(e) => persistence_error_to_response(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/projects",
    responses(
        (status = OK, body = [ProjectResponse]),
        (status = INTERNAL_SERVER_ERROR, body = ProblemDetail, content_type = "application/problem+json")
    )
)]
async fn list_projects(State(state): State<Arc<AppState>>) -> Response {
    match persistence::list_projects(&state.db).await {
        Ok(projects) => Json(
            projects
                .into_iter()
                .map(with_git_summary)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => persistence_error_to_response(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/projects/{id}",
    params(("id" = String, Path, description = "Project ID")),
    responses(
        (status = OK, body = ProjectResponse),
        (status = NOT_FOUND, body = ProblemDetail, content_type = "application/problem+json"),
        (status = INTERNAL_SERVER_ERROR, body = ProblemDetail, content_type = "application/problem+json")
    )
)]
async fn get_project(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match persistence::get_project(&state.db, &id).await {
        Ok(project) => Json(with_git_summary(project)).into_response(),
        Err(e) => persistence_error_to_response(e),
    }
}

#[utoipa::path(
    put,
    path = "/api/projects/{id}/trust",
    params(("id" = String, Path, description = "Project ID")),
    responses(
        (status = OK, body = ProjectResponse),
        (status = NOT_FOUND, body = ProblemDetail, content_type = "application/problem+json"),
        (status = INTERNAL_SERVER_ERROR, body = ProblemDetail, content_type = "application/problem+json")
    )
)]
async fn trust_project(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match persistence::trust_project(&state.db, &id).await {
        Ok(project) => {
            let project = with_git_summary(project);
            crate::project_event_publisher::publish_project_changed(
                &state.event_bus,
                &id,
                project.clone(),
            );
            Json(project).into_response()
        }
        Err(e) => persistence_error_to_response(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/projects/{id}/issue-source-candidates",
    params(("id" = String, Path, description = "Project ID")),
    responses(
        (status = OK, body = [IssueSourceCandidate]),
        (status = NOT_FOUND, body = ProblemDetail, content_type = "application/problem+json"),
        (status = INTERNAL_SERVER_ERROR, body = ProblemDetail, content_type = "application/problem+json")
    )
)]
async fn list_issue_source_candidates(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    match persistence::get_project(&state.db, &id).await {
        Ok(project) => Json(discover_issue_source_candidates(&project)).into_response(),
        Err(e) => persistence_error_to_response(e),
    }
}

#[utoipa::path(
    put,
    path = "/api/projects/{id}/issue-source",
    params(("id" = String, Path, description = "Project ID")),
    request_body = EnableIssueSourceRequest,
    responses(
        (status = OK, body = ProjectResponse),
        (status = NOT_FOUND, body = ProblemDetail, content_type = "application/problem+json"),
        (status = UNPROCESSABLE_ENTITY, body = ProblemDetail, content_type = "application/problem+json"),
        (status = INTERNAL_SERVER_ERROR, body = ProblemDetail, content_type = "application/problem+json")
    )
)]
async fn enable_issue_source(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(request): Json<EnableIssueSourceRequest>,
) -> Response {
    match persistence::enable_issue_source(&state.db, &id, &request).await {
        Ok(project) => {
            let project = with_git_summary(project);
            crate::project_event_publisher::publish_project_changed(
                &state.event_bus,
                &id,
                project.clone(),
            );
            let candidates = discover_issue_source_candidates(&project);
            crate::project_event_publisher::publish_issue_source_candidates_changed(
                &state.event_bus,
                &id,
                candidates,
            );
            crate::project_event_publisher::publish_planning_snapshot_changed(
                &state.event_bus,
                &id,
                persistence::get_planning_snapshot(&state.db, &id).await.ok(),
            );
            Json(project).into_response()
        }
        Err(e) => persistence_error_to_response(e),
    }
}

#[utoipa::path(
    post,
    path = "/api/projects/{id}/issue-source/sync",
    params(("id" = String, Path, description = "Project ID")),
    responses(
        (status = OK, body = IssueSourceSyncResponse),
        (status = NOT_FOUND, body = ProblemDetail, content_type = "application/problem+json"),
        (status = UNPROCESSABLE_ENTITY, body = ProblemDetail, content_type = "application/problem+json"),
        (status = INTERNAL_SERVER_ERROR, body = ProblemDetail, content_type = "application/problem+json")
    )
)]
async fn sync_issue_source(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let project = match persistence::get_project(&state.db, &id).await {
        Ok(project) => project,
        Err(e) => return persistence_error_to_response(e),
    };

    let Some(source) = project.enabled_issue_source.clone() else {
        return sync_problem_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "urn:agentic-afk:issue-source-not-enabled",
            "Unprocessable Entity",
            "Project has no enabled Issue Source".to_string(),
        );
    };

    crate::project_event_publisher::publish_issue_source_sync_started(&state.event_bus, &id);

    let sync_result = match source.kind.as_str() {
        "local_markdown" => read_local_markdown_issues(&project.path, &source.locator),
        "github" => read_github_issues(&state.config.gh_binary_path, &source.locator),
        _ => Err(format!(
            "manual sync is not supported for {} Issue Sources yet",
            source.kind
        )),
    };

    match sync_result {
        Ok(issues) => {
            let synced_at = current_sync_timestamp();
            match persistence::replace_planning_snapshot(
                &state.db, &id, &source, &issues, &synced_at,
            )
            .await
            {
                Ok(response) => {
                    crate::project_event_publisher::publish_issue_source_sync_completed(
                        &state.event_bus,
                        &id,
                        response.clone(),
                    );
                    let planning = persistence::get_planning_snapshot(&state.db, &id).await.ok();
                    crate::project_event_publisher::publish_planning_snapshot_changed(
                        &state.event_bus,
                        &id,
                        planning,
                    );
                    Json(response).into_response()
                }
                Err(e) => persistence_error_to_response(e),
            }
        }
        Err(detail) => {
            let _ = persistence::record_sync_failure(&state.db, &id, &source, &detail).await;
            crate::project_event_publisher::publish_issue_source_sync_failed(
                &state.event_bus,
                &id,
                &detail,
            );
            sync_problem_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "urn:agentic-afk:issue-source-sync-failed",
                "Unprocessable Entity",
                detail,
            )
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/projects/{id}/issue-source/sync-status",
    params(("id" = String, Path, description = "Project ID")),
    responses(
        (status = OK, body = IssueSourceSyncStatusResponse),
        (status = NOT_FOUND, body = ProblemDetail, content_type = "application/problem+json"),
        (status = UNPROCESSABLE_ENTITY, body = ProblemDetail, content_type = "application/problem+json"),
        (status = INTERNAL_SERVER_ERROR, body = ProblemDetail, content_type = "application/problem+json")
    )
)]
async fn get_issue_source_sync_status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    match persistence::get_issue_source_sync_status(&state.db, &id).await {
        Ok(status) => Json(status).into_response(),
        Err(e) => persistence_error_to_response(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/projects/{id}/planning-snapshot",
    params(("id" = String, Path, description = "Project ID")),
    responses(
        (status = OK, body = PlanningSnapshotResponse),
        (status = NOT_FOUND, body = ProblemDetail, content_type = "application/problem+json"),
        (status = INTERNAL_SERVER_ERROR, body = ProblemDetail, content_type = "application/problem+json")
    )
)]
async fn get_planning_snapshot(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    match persistence::get_planning_snapshot(&state.db, &id).await {
        Ok(snapshot) => Json(snapshot).into_response(),
        Err(e) => persistence_error_to_response(e),
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ActivityQuery {
    pub limit: Option<i64>,
}

#[utoipa::path(
    get,
    path = "/api/projects/{id}/activity",
    params(
        ("id" = String, Path, description = "Project ID"),
        ("limit" = Option<i64>, Query, description = "Max entries to return (default 50, max 200)")
    ),
    responses(
        (status = OK, body = Vec<ProjectActivityEntryResponse>),
        (status = NOT_FOUND, body = ProblemDetail, content_type = "application/problem+json"),
        (status = INTERNAL_SERVER_ERROR, body = ProblemDetail, content_type = "application/problem+json")
    )
)]
async fn get_project_activity(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<ActivityQuery>,
) -> Response {
    if let Err(error) = persistence::get_project(&state.db, &id).await {
        return persistence_error_to_response(error);
    }
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let entries = match persistence::list_project_activity(&state.db, &id, limit).await {
        Ok(entries) => entries,
        Err(error) => return persistence_error_to_response(error),
    };
    let responses: Vec<ProjectActivityEntryResponse> = entries
        .into_iter()
        .map(|entry| ProjectActivityEntryResponse {
            id: entry.id,
            project_id: entry.project_id,
            assignment_id: entry.assignment_id,
            kind: entry.kind,
            detail: entry.detail,
            recorded_at: entry.recorded_at,
        })
        .collect();
    Json(responses).into_response()
}

/// Bundle the current Project state into one response. Composed of the
/// four existing per-panel reads (`project`, `planning-snapshot`,
/// `assignment-state`, `activity`) plus the latest event-bus `sequence` at
/// assembly time, so the Dashboard can hydrate its reactive store and
/// immediately subscribe to the SSE stream from that sequence (ADR-0032).
#[utoipa::path(
    get,
    path = "/api/projects/{id}/snapshot",
    params(("id" = String, Path, description = "Project ID")),
    responses(
        (status = OK, body = ProjectSnapshotResponse),
        (status = NOT_FOUND, body = ProblemDetail, content_type = "application/problem+json"),
        (status = INTERNAL_SERVER_ERROR, body = ProblemDetail, content_type = "application/problem+json")
    )
)]
async fn get_project_snapshot(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let project = match persistence::get_project(&state.db, &id).await {
        Ok(project) => with_git_summary(project),
        Err(error) => return persistence_error_to_response(error),
    };

    // Planning snapshot may not exist yet (no Issue Source enabled or never
    // synced). Treat missing snapshot as `None` rather than 404 for the
    // bundle, since other panels still have data to show.
    let planning_snapshot = match persistence::get_planning_snapshot(&state.db, &id).await {
        Ok(snapshot) => Some(snapshot),
        Err(PersistenceError::SnapshotNotFound(_)) => None,
        Err(error) => return persistence_error_to_response(error),
    };

    let activity_entries = match persistence::list_project_activity(&state.db, &id, 50).await {
        Ok(entries) => entries,
        Err(error) => return persistence_error_to_response(error),
    };
    let activity: Vec<ProjectActivityEntryResponse> = activity_entries
        .into_iter()
        .map(|entry| ProjectActivityEntryResponse {
            id: entry.id,
            project_id: entry.project_id,
            assignment_id: entry.assignment_id,
            kind: entry.kind,
            detail: entry.detail,
            recorded_at: entry.recorded_at,
        })
        .collect();

    let issue_source_candidates = discover_issue_source_candidates(&project);

    let execution_config = match persistence::get_project_execution_config(&state.db, &id).await {
        Ok(config) => config,
        Err(error) => return persistence_error_to_response(error),
    };
    let active_plan_run = match persistence::get_active_plan_run(&state.db, &id).await {
        Ok(plan_run) => plan_run,
        Err(error) => return persistence_error_to_response(error),
    };
    let recent_plan_runs = match persistence::list_recent_plan_runs(&state.db, &id, 20).await {
        Ok(runs) => runs,
        Err(error) => return persistence_error_to_response(error),
    };

    let sequence = state.event_bus.latest_sequence(&ProjectId(id.clone()));
    Json(ProjectSnapshotResponse {
        snapshot: ProjectSnapshot {
            project,
            planning_snapshot,
            activity,
            issue_source_candidates,
            execution_config,
            active_plan_run,
            recent_plan_runs,
        },
        sequence,
    })
    .into_response()
}

/// Server-Sent Events stream of typed `ProjectEvent` deltas for one Project.
///
/// Honors `Last-Event-ID`: when set to a sequence still resident in the
/// per-Project ring buffer, the stream replays events with `sequence >
/// Last-Event-ID` before emitting live events. When the requested sequence
/// predates the ring (or the server has restarted), the first emitted event
/// is `Resync`, signalling the client to re-hydrate via `/snapshot`.
#[derive(Debug, Deserialize)]
struct EventsQuery {
    last_event_id: Option<u64>,
}

/// Test-only request body for `POST /api/_test/projects/{id}/activity`.
#[derive(Debug, Deserialize)]
struct TestRecordActivityRequest {
    kind: String,
    #[serde(default)]
    detail: Option<String>,
}

/// Mounted only when `AGENTIC_AFK_TEST_ENDPOINTS=1`. Records a Project
/// Activity entry via the production `activity_publisher`, so Playwright can
/// drive the live-update flow without needing the full assignment pipeline
/// (worktrunk + codex binaries) available in CI.
fn test_endpoints_router() -> Router<Arc<AppState>> {
    if std::env::var("AGENTIC_AFK_TEST_ENDPOINTS").as_deref() != Ok("1") {
        return Router::new();
    }
    Router::new()
        .route(
            "/api/_test/projects/{id}/activity",
            post(test_record_activity),
        )
        .route(
            "/api/_test/projects/{id}/project-event",
            post(test_publish_project_event),
        )
        .route(
            "/api/_test/projects/{id}/planning-snapshot",
            post(test_seed_planning_snapshot),
        )
}

/// Test-only request body: a list of fully-formed Source Issue rows the
/// caller wants reflected in the persisted planning snapshot. Lets the
/// Playwright e2e seed the snapshot without depending on a real Issue
/// Source sync.
#[derive(Debug, Deserialize)]
struct TestSeedPlanningSnapshotRequest {
    source: IssueSource,
    issues: Vec<SourceIssueSnapshot>,
}

async fn test_seed_planning_snapshot(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(request): Json<TestSeedPlanningSnapshotRequest>,
) -> Response {
    // Make sure the project knows about this Issue Source so the claim path
    // can use it for lifecycle write-back.
    if let Err(error) = persistence::enable_issue_source(
        &state.db,
        &id,
        &EnableIssueSourceRequest {
            kind: request.source.kind.clone(),
            locator: request.source.locator.clone(),
        },
    )
    .await
    {
        return persistence_error_to_response(error);
    }
    match persistence::replace_planning_snapshot(
        &state.db,
        &id,
        &request.source,
        &request.issues,
        "unix:1",
    )
    .await
    {
        Ok(_) => {
            if let Ok(snapshot) = persistence::get_planning_snapshot(&state.db, &id).await {
                crate::project_event_publisher::publish_planning_snapshot_changed(
                    &state.event_bus,
                    &id,
                    Some(snapshot),
                );
            }
            StatusCode::OK.into_response()
        }
        Err(error) => persistence_error_to_response(error),
    }
}

/// Test-only: publish an arbitrary `ProjectEvent` for `id` so Playwright can
/// drive Plan Run lifecycle flows without needing worktrunk/codex binaries.
async fn test_publish_project_event(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(event): Json<ProjectEvent>,
) -> Response {
    state.event_bus.publish(&ProjectId(id), event);
    StatusCode::OK.into_response()
}

async fn test_record_activity(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(request): Json<TestRecordActivityRequest>,
) -> Response {
    match crate::activity_publisher::record_project_activity(
        &state.db,
        &state.event_bus,
        &id,
        None,
        &request.kind,
        request.detail.as_deref(),
    )
    .await
    {
        Ok(entry) => {
            let wire = ProjectActivityEntryResponse {
                id: entry.id,
                project_id: entry.project_id,
                assignment_id: entry.assignment_id,
                kind: entry.kind,
                detail: entry.detail,
                recorded_at: entry.recorded_at,
            };
            (StatusCode::OK, Json(wire)).into_response()
        }
        Err(error) => persistence_error_to_response(error),
    }
}

async fn get_project_events(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<EventsQuery>,
    headers: axum::http::HeaderMap,
) -> Response {
    if let Err(error) = persistence::get_project(&state.db, &id).await {
        return persistence_error_to_response(error);
    }
    // `Last-Event-ID` header (sent by browsers on EventSource auto-reconnect)
    // takes precedence. On first connect the browser does not send the
    // header, so the Dashboard passes the snapshot sequence via the
    // `last_event_id` query parameter instead.
    let last_event_id = headers
        .get(axum::http::header::HeaderName::from_static("last-event-id"))
        .and_then(|value| value.to_str().ok())
        .and_then(|raw| raw.parse::<u64>().ok())
        .or(query.last_event_id);
    let project_id = ProjectId(id);
    let stream = state.event_bus.subscribe(&project_id, last_event_id);
    use futures_util::StreamExt as _;
    let sse_stream = stream.map(|sequenced| {
        let event = axum::response::sse::Event::default()
            .id(sequenced.sequence.to_string())
            .json_data(&sequenced.event)
            .unwrap_or_else(|_| axum::response::sse::Event::default());
        Ok::<_, std::convert::Infallible>(event)
    });
    axum::response::sse::Sse::new(sse_stream)
        .keep_alive(axum::response::sse::KeepAlive::default())
        .into_response()
}

#[utoipa::path(
    put,
    path = "/api/projects/{id}/source-issues/{source_id}/lifecycle-status",
    params(
        ("id" = String, Path, description = "Project ID"),
        ("source_id" = String, Path, description = "Source Issue ID")
    ),
    request_body = SetLifecycleStatusRequest,
    responses(
        (status = OK, body = SourceIssueSnapshot),
        (status = NOT_FOUND, body = ProblemDetail, content_type = "application/problem+json"),
        (status = UNPROCESSABLE_ENTITY, body = ProblemDetail, content_type = "application/problem+json"),
        (status = INTERNAL_SERVER_ERROR, body = ProblemDetail, content_type = "application/problem+json")
    )
)]
async fn update_lifecycle_status(
    State(state): State<Arc<AppState>>,
    Path((id, source_id)): Path<(String, String)>,
    Json(request): Json<SetLifecycleStatusRequest>,
) -> Response {
    let project = match persistence::get_project(&state.db, &id).await {
        Ok(project) => project,
        Err(e) => return persistence_error_to_response(e),
    };

    let Some(source) = project.enabled_issue_source else {
        return sync_problem_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "urn:agentic-afk:issue-source-not-enabled",
            "Unprocessable Entity",
            "Project has no enabled Issue Source".to_string(),
        );
    };

    if source.kind != "local_markdown" {
        return sync_problem_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "urn:agentic-afk:lifecycle-write-back-not-supported",
            "Unprocessable Entity",
            "Lifecycle write-back is only supported for local_markdown Issue Sources".to_string(),
        );
    }

    let valid_statuses = ["ready", "claimed", "running", "blocked", "completed"];
    if !valid_statuses.contains(&request.lifecycle_status.as_str()) {
        return sync_problem_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "urn:agentic-afk:invalid-lifecycle-status",
            "Unprocessable Entity",
            format!(
                "Invalid lifecycle_status: {}. Must be one of: ready, claimed, running, blocked, completed",
                request.lifecycle_status
            ),
        );
    }

    let project_root = match std::fs::canonicalize(&project.path) {
        Ok(path) => path,
        Err(error) => {
            return sync_problem_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "urn:agentic-afk:internal-error",
                "Internal Server Error",
                format!("failed to resolve Project path: {error}"),
            );
        }
    };

    let source_dir = match std::fs::canonicalize(project_root.join(&source.locator)) {
        Ok(path) => path,
        Err(error) => {
            return sync_problem_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "urn:agentic-afk:internal-error",
                "Internal Server Error",
                format!("failed to read local markdown Issue Source: {error}"),
            );
        }
    };

    if !source_dir.starts_with(&project_root) {
        return sync_problem_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "urn:agentic-afk:internal-error",
            "Internal Server Error",
            "local markdown Issue Source must be inside the Project path".to_string(),
        );
    }

    let issue_path = source_dir.join(format!("{source_id}.md"));
    let raw_text = match std::fs::read_to_string(&issue_path) {
        Ok(text) => text,
        Err(_) => {
            return sync_problem_response(
                StatusCode::NOT_FOUND,
                "urn:agentic-afk:source-issue-not-found",
                "Not Found",
                format!("Source Issue not found: {source_id}"),
            );
        }
    };

    let updated_text = update_markdown_lifecycle_status(raw_text, &request.lifecycle_status);

    if let Err(error) = std::fs::write(&issue_path, updated_text) {
        return sync_problem_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "urn:agentic-afk:internal-error",
            "Internal Server Error",
            format!("failed to write Source Issue file: {error}"),
        );
    }

    let updated_raw = match std::fs::read_to_string(&issue_path) {
        Ok(text) => text,
        Err(error) => {
            return sync_problem_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "urn:agentic-afk:internal-error",
                "Internal Server Error",
                format!("failed to read updated Source Issue file: {error}"),
            );
        }
    };

    let snapshot = parse_local_markdown_issue(source_id, updated_raw, 0);
    Json(snapshot).into_response()
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
    Ok(parse_local_markdown_issue(
        source_id.to_string(),
        updated_raw,
        0,
    ))
}

pub(crate) fn write_assignment_lifecycle(
    gh_binary_path: &std::path::Path,
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

fn write_github_lifecycle(
    gh_binary_path: &std::path::Path,
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

fn read_local_markdown_issues(
    project_path: &str,
    locator: &str,
) -> Result<Vec<SourceIssueSnapshot>, String> {
    let project_root = std::fs::canonicalize(project_path)
        .map_err(|error| format!("failed to resolve Project path: {error}"))?;
    let source_dir = std::fs::canonicalize(project_root.join(locator))
        .map_err(|error| format!("failed to read local markdown Issue Source: {error}"))?;
    if !source_dir.starts_with(&project_root) {
        return Err("local markdown Issue Source must be inside the Project path".to_string());
    }

    let entries = std::fs::read_dir(&source_dir)
        .map_err(|error| format!("failed to read local markdown Issue Source: {error}"))?;
    let mut paths = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        .collect::<Vec<_>>();
    paths.sort();

    let mut issues = Vec::new();
    for (index, path) in paths.into_iter().enumerate() {
        let raw_text = std::fs::read_to_string(&path)
            .map_err(|error| format!("failed to read Source Issue {}: {error}", path.display()))?;
        let source_id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| format!("invalid Source Issue file name: {}", path.display()))?
            .to_string();
        issues.push(parse_local_markdown_issue(
            source_id,
            raw_text,
            i64::try_from(index + 1).unwrap_or(i64::MAX),
        ));
    }

    Ok(issues)
}

#[derive(Debug, Deserialize)]
struct GitHubIssue {
    number: i64,
    title: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    labels: Vec<GitHubLabel>,
}

#[derive(Debug, Deserialize)]
struct GitHubLabel {
    name: String,
}

fn read_github_issues(
    gh_binary_path: &std::path::Path,
    locator: &str,
) -> Result<Vec<SourceIssueSnapshot>, String> {
    let auth = Command::new(gh_binary_path)
        .args(["auth", "status"])
        .output()
        .map_err(|error| format!("failed to run gh auth status: {error}"))?;
    if !auth.status.success() {
        return Err(format!(
            "gh is not authenticated: {}",
            command_output(&auth)
        ));
    }

    let output = Command::new(gh_binary_path)
        .args([
            "issue",
            "list",
            "--repo",
            locator,
            "--state",
            "open",
            "--limit",
            "1000",
            "--json",
            "number,title,body,labels",
        ])
        .output()
        .map_err(|error| format!("failed to run gh issue list: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed to sync GitHub Issue Source: {}",
            command_output(&output)
        ));
    }

    let mut issues = serde_json::from_slice::<Vec<GitHubIssue>>(&output.stdout)
        .map_err(|error| format!("failed to parse gh issue list output: {error}"))?
        .into_iter()
        .map(parse_github_issue)
        .collect::<Vec<_>>();
    issues.sort_by(|left, right| {
        left.source_order
            .cmp(&right.source_order)
            .then_with(|| left.source_id.cmp(&right.source_id))
    });
    Ok(issues)
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

fn parse_github_issue(issue: GitHubIssue) -> SourceIssueSnapshot {
    let source_id = issue.number.to_string();
    let raw_text = issue.body.unwrap_or_default();
    let mut snapshot = parse_local_markdown_issue(source_id, raw_text, issue.number);
    snapshot.title = if issue.title.trim().is_empty() {
        snapshot.source_id.clone()
    } else {
        issue.title
    };
    snapshot.readiness = if issue
        .labels
        .iter()
        .any(|label| label.name == "ready-for-agent")
    {
        "ready".to_string()
    } else {
        "not-ready".to_string()
    };
    snapshot.lifecycle_status = issue
        .labels
        .iter()
        .find_map(|label| label.name.strip_prefix("agentic-afk:"))
        .filter(|status| matches!(*status, "claimed" | "running" | "blocked" | "completed"))
        .unwrap_or("ready")
        .to_string();
    snapshot
}

fn parse_local_markdown_issue(
    source_id: String,
    raw_text: String,
    fallback_order: i64,
) -> SourceIssueSnapshot {
    let title = raw_text
        .lines()
        .find_map(|line| line.trim().strip_prefix("# ").map(str::trim))
        .filter(|title| !title.is_empty())
        .unwrap_or(&source_id)
        .to_string();

    let mut readiness = "not-ready".to_string();
    let mut lifecycle_status = "ready".to_string();
    let mut parent_issue = None;
    let mut issue_dependencies = Vec::new();
    let mut source_order = fallback_order;

    for line in raw_text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        match key.as_str() {
            "readiness" | "ready" => {
                let normalized = value.to_ascii_lowercase();
                readiness = if matches!(normalized.as_str(), "ready" | "true" | "yes") {
                    "ready".to_string()
                } else {
                    "not-ready".to_string()
                };
            }
            "lifecycle status" => {
                let normalized = value.to_ascii_lowercase();
                lifecycle_status = if matches!(
                    normalized.as_str(),
                    "claimed" | "running" | "blocked" | "completed"
                ) {
                    normalized
                } else {
                    "ready".to_string()
                };
            }
            "parent issue" | "parent" => {
                parent_issue = normalize_issue_ref(value);
            }
            "issue dependencies" | "dependencies" => {
                issue_dependencies = value
                    .split([',', ' ', '\t'])
                    .filter_map(normalize_issue_ref)
                    .collect();
            }
            "source order" => {
                if let Ok(parsed) = value.parse::<i64>() {
                    source_order = parsed;
                }
            }
            _ => {}
        }
    }

    SourceIssueSnapshot {
        source_id,
        title,
        readiness,
        lifecycle_status,
        parent_issue,
        issue_dependencies,
        source_order,
        raw_text,
    }
}

fn update_markdown_lifecycle_status(raw_text: String, lifecycle_status: &str) -> String {
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

fn normalize_issue_ref(value: &str) -> Option<String> {
    let trimmed = value
        .trim()
        .trim_matches(|c: char| matches!(c, '-' | '*' | '[' | ']' | '(' | ')' | '`'))
        .trim_start_matches('#')
        .trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn current_sync_timestamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("unix:{seconds}")
}

fn with_git_summary(mut project: ProjectResponse) -> ProjectResponse {
    project.git_summary = summarize_project_path(&project.path);
    project
}

fn discover_issue_source_candidates(project: &ProjectResponse) -> Vec<IssueSourceCandidate> {
    let mut candidates = Vec::new();
    let project_path = PathBuf::from(&project.path);

    if let Some(locator) = discover_github_locator(&project_path) {
        candidates.push(candidate(project, "github", &locator));
    }

    for relative_path in [".scratch/issues", "issues", "docs/issues"] {
        if project_path.join(relative_path).is_dir() {
            candidates.push(candidate(project, "local_markdown", relative_path));
        }
    }

    candidates
}

fn candidate(project: &ProjectResponse, kind: &str, locator: &str) -> IssueSourceCandidate {
    let enabled = project
        .enabled_issue_source
        .as_ref()
        .is_some_and(|source| source.kind == kind && source.locator == locator);

    IssueSourceCandidate {
        kind: kind.to_string(),
        locator: locator.to_string(),
        enabled,
    }
}

fn discover_github_locator(project_path: &std::path::Path) -> Option<String> {
    let config = std::fs::read_to_string(project_path.join(".git/config")).ok()?;
    config.lines().find_map(|line| {
        let (_, url) = line.split_once('=')?;
        github_locator_from_url(url.trim())
    })
}

fn github_locator_from_url(url: &str) -> Option<String> {
    let path = if let Some(path) = url.strip_prefix("git@github.com:") {
        path
    } else if let Some(path) = url.strip_prefix("https://github.com/") {
        path
    } else if let Some(path) = url.strip_prefix("ssh://git@github.com/") {
        path
    } else {
        return None;
    };

    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.split('/');
    let owner = parts.next()?.trim();
    let repo = parts.next()?.trim();
    if owner.is_empty() || repo.is_empty() {
        return None;
    }

    Some(format!("{owner}/{repo}"))
}

pub(crate) fn persistence_error_to_response(err: PersistenceError) -> Response {
    let (status, problem_type, title) = match &err {
        PersistenceError::NotFound(_) => (
            StatusCode::NOT_FOUND,
            "urn:agentic-afk:not-found",
            "Not Found",
        ),
        PersistenceError::Duplicate(_) => (
            StatusCode::CONFLICT,
            "urn:agentic-afk:duplicate",
            "Conflict",
        ),
        PersistenceError::InvalidPath(_) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "urn:agentic-afk:invalid-path",
            "Unprocessable Entity",
        ),
        PersistenceError::InvalidIssueSource(_) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "urn:agentic-afk:invalid-issue-source",
            "Unprocessable Entity",
        ),
        PersistenceError::SnapshotNotFound(_) => (
            StatusCode::NOT_FOUND,
            "urn:agentic-afk:planning-snapshot-not-found",
            "Not Found",
        ),
        PersistenceError::ActiveAssignment(_) => (
            StatusCode::CONFLICT,
            "urn:agentic-afk:active-assignment",
            "Conflict",
        ),
        PersistenceError::AssignmentNotFound(_) => (
            StatusCode::NOT_FOUND,
            "urn:agentic-afk:assignment-not-found",
            "Not Found",
        ),
        PersistenceError::Database(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "urn:agentic-afk:internal-error",
            "Internal Server Error",
        ),
    };

    let problem = ProblemDetail {
        problem_type: problem_type.to_string(),
        title: title.to_string(),
        status: status.as_u16(),
        detail: err.to_string(),
    };

    (
        status,
        [("content-type", "application/problem+json")],
        Json(problem),
    )
        .into_response()
}

pub(crate) fn sync_problem_response(
    status: StatusCode,
    problem_type: &str,
    title: &str,
    detail: String,
) -> Response {
    (
        status,
        [("content-type", "application/problem+json")],
        Json(ProblemDetail {
            problem_type: problem_type.to_string(),
            title: title.to_string(),
            status: status.as_u16(),
            detail,
        }),
    )
        .into_response()
}

// --- Plan Run handlers (ADR-0034) ---

async fn set_execution_config(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(request): Json<SetProjectExecutionConfigRequest>,
) -> Response {
    if request.max_parallel_tasks <= 0 || request.review_retry_limit <= 0 {
        return sync_problem_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "urn:agentic-afk:invalid-execution-config",
            "Unprocessable Entity",
            "max_parallel_tasks and review_retry_limit must be positive".to_string(),
        );
    }
    if request.integration_branch.trim().is_empty() {
        return sync_problem_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "urn:agentic-afk:invalid-execution-config",
            "Unprocessable Entity",
            "integration_branch must not be empty".to_string(),
        );
    }
    match persistence::set_project_execution_config(&state.db, &id, &request).await {
        Ok(config) => {
            crate::project_event_publisher::publish_project_execution_config_changed(
                &state.event_bus,
                &id,
                config.clone(),
            );
            Json(config).into_response()
        }
        Err(error) => persistence_error_to_response(error),
    }
}

/// Human re-enable for a blocked Issue Assignment (issue #44). Clears the
/// blocked Lifecycle Status both on the assignment row and on the upstream
/// Issue Source, and resets the review rejection counter so a later Plan
/// Run may pick up the Source Issue again. Does NOT redefine
/// `ready-for-agent` readiness — that remains a separate Source Issue
/// concern.
async fn re_enable_assignment(
    State(state): State<Arc<AppState>>,
    Path((id, assignment_id)): Path<(String, String)>,
) -> Response {
    let assignment = match persistence::get_project_assignment(&state.db, &id, &assignment_id).await
    {
        Ok(assignment) => assignment,
        Err(error) => return persistence_error_to_response(error),
    };
    if assignment.status != "blocked" {
        return sync_problem_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "urn:agentic-afk:assignment-not-blocked",
            "Unprocessable Entity",
            format!(
                "Issue Assignment {assignment_id} is not blocked (status={})",
                assignment.status
            ),
        );
    }
    let project = match persistence::get_project(&state.db, &id).await {
        Ok(project) => project,
        Err(error) => return persistence_error_to_response(error),
    };
    let project = with_git_summary(project);

    let updated = match persistence::re_enable_blocked_assignment(&state.db, &assignment_id).await {
        Ok(updated) => updated,
        Err(error) => return persistence_error_to_response(error),
    };

    // PRD #38: human re-enable clears blocked lifecycle state without
    // redefining `ready-for-agent` readiness. The Source Issue's
    // `ready-for-agent` label (GitHub) / `ready` frontmatter status
    // (local markdown) still governs readiness, so re-enable does NOT
    // write the `ready` lifecycle back. The assignment row + cleared
    // block reason are the authoritative re-enable signal; a subsequent
    // Issue Source sync repopulates the eligible bucket from upstream
    // readiness state. We intentionally avoid overloading the lifecycle
    // value here.
    let _ = &project.enabled_issue_source;
    let _ = &assignment.source_id;

    crate::project_event_publisher::publish_assignment_status_changed(
        &state.event_bus,
        &id,
        updated.clone(),
    );

    Json(updated).into_response()
}

async fn list_plan_runs(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    if let Err(error) = persistence::get_project(&state.db, &id).await {
        return persistence_error_to_response(error);
    }
    match persistence::list_recent_plan_runs(&state.db, &id, 50).await {
        Ok(runs) => Json(runs).into_response(),
        Err(error) => persistence_error_to_response(error),
    }
}

async fn start_plan_run(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let project = match persistence::get_project(&state.db, &id).await {
        Ok(project) => project,
        Err(error) => return persistence_error_to_response(error),
    };
    if !project.trusted {
        return sync_problem_response(
            StatusCode::FORBIDDEN,
            "urn:agentic-afk:project-untrusted",
            "Forbidden",
            "Project must be trusted before starting a Plan Run".to_string(),
        );
    }
    let execution_config = match persistence::get_project_execution_config(&state.db, &id).await {
        Ok(Some(config)) => config,
        Ok(None) => {
            // PRD #34/#21: platform defaults seed the Project Execution
            // Config on first Plan Run so a developer can start unattended
            // work without a separate config step. Integration Branch
            // defaults from the detected Git default branch (falling back
            // to `main`); Max Parallel Tasks and Review Retry Limit get
            // conservative defaults the developer can override later.
            let project_path = std::path::Path::new(&project.path);
            let detected_default_branch =
                agentic_afk_git_summary::detect_default_branch(project_path)
                    .unwrap_or_else(|| "main".to_string());
            let default_request = SetProjectExecutionConfigRequest {
                integration_branch: detected_default_branch,
                max_parallel_tasks: 3,
                review_retry_limit: 3,
            };
            match persistence::set_project_execution_config(&state.db, &id, &default_request).await
            {
                Ok(config) => {
                    crate::project_event_publisher::publish_project_execution_config_changed(
                        &state.event_bus,
                        &id,
                        config.clone(),
                    );
                    config
                }
                Err(error) => return persistence_error_to_response(error),
            }
        }
        Err(error) => return persistence_error_to_response(error),
    };
    match persistence::get_active_plan_run(&state.db, &id).await {
        Ok(Some(_)) => {
            return sync_problem_response(
                StatusCode::CONFLICT,
                "urn:agentic-afk:active-plan-run",
                "Conflict",
                "Project already has an active Plan Run".to_string(),
            );
        }
        Ok(None) => {}
        Err(error) => return persistence_error_to_response(error),
    }

    let project_path = std::path::Path::new(&project.path);
    let baseline = match state
        .plan_run_deps
        .refresher
        .refresh(project_path, &execution_config.integration_branch)
    {
        Ok(baseline) => baseline,
        Err(error) => {
            return sync_problem_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "urn:agentic-afk:integration-branch-refresh-failed",
                "Internal Server Error",
                error.to_string(),
            );
        }
    };

    let plan_run = match persistence::create_plan_run(
        &state.db,
        &id,
        &execution_config.integration_branch,
        &baseline.commit_sha,
    )
    .await
    {
        Ok(run) => run,
        Err(error) => return persistence_error_to_response(error),
    };
    crate::project_event_publisher::publish_plan_run_started(
        &state.event_bus,
        &id,
        plan_run.clone(),
    );

    let eligible = persistence::get_planning_snapshot(&state.db, &id)
        .await
        .map(|snapshot| snapshot.eligible)
        .unwrap_or_default();
    let project_instructions = load_project_instructions(&project.path);
    let prompt = render_planning_prompt(
        &project_instructions,
        &project,
        &execution_config,
        &baseline,
        &eligible,
    );
    let planner_stdout = match state.plan_run_deps.planner.run(&prompt) {
        Ok(stdout) => stdout,
        Err(error) => {
            let _ = persistence::record_plan_run_phase_output(
                &state.db,
                &plan_run.id,
                "planning",
                "failed",
                &serde_json::json!({ "error": error.to_string() }),
            )
            .await;
            let finished = persistence::finish_plan_run(&state.db, &plan_run.id, "failed").await;
            if let Ok(run) = finished {
                crate::project_event_publisher::publish_plan_run_completed(
                    &state.event_bus,
                    &id,
                    run,
                );
            }
            return sync_problem_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "urn:agentic-afk:planning-phase-failed",
                "Internal Server Error",
                error.to_string(),
            );
        }
    };

    let parsed = match agentic_afk_orchestrator::parse_planning_output(&planner_stdout) {
        Ok(parsed) => parsed,
        Err(error) => {
            let _ = persistence::record_plan_run_phase_output(
                &state.db,
                &plan_run.id,
                "planning",
                "failed",
                &serde_json::json!({ "error": error }),
            )
            .await;
            let finished = persistence::finish_plan_run(&state.db, &plan_run.id, "failed").await;
            if let Ok(run) = finished {
                crate::project_event_publisher::publish_plan_run_completed(
                    &state.event_bus,
                    &id,
                    run,
                );
            }
            return sync_problem_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "urn:agentic-afk:planning-output-unparseable",
                "Internal Server Error",
                error,
            );
        }
    };

    if parsed.is_empty {
        return finalize_empty_planning(&state, &id, &plan_run.id, &parsed.body).await;
    }

    finalize_selection_planning(&state, &project, &id, &plan_run, &baseline, &parsed.body).await
}

async fn finalize_empty_planning(
    state: &AppState,
    project_id: &str,
    plan_run_id: &str,
    body: &serde_json::Value,
) -> Response {
    let phase_output = match persistence::record_plan_run_phase_output(
        &state.db,
        plan_run_id,
        "planning",
        "succeeded_empty",
        body,
    )
    .await
    {
        Ok(output) => output,
        Err(error) => return persistence_error_to_response(error),
    };
    crate::project_event_publisher::publish_plan_run_phase_completed(
        &state.event_bus,
        project_id,
        plan_run_id,
        phase_output,
    );
    let finished =
        match persistence::finish_plan_run(&state.db, plan_run_id, "succeeded_empty").await {
            Ok(run) => run,
            Err(error) => return persistence_error_to_response(error),
        };
    crate::project_event_publisher::publish_plan_run_completed(
        &state.event_bus,
        project_id,
        finished.clone(),
    );
    (StatusCode::CREATED, Json(finished)).into_response()
}

async fn finalize_selection_planning(
    state: &AppState,
    project: &ProjectResponse,
    project_id: &str,
    plan_run: &PlanRunResponse,
    baseline: &RefreshedBaseline,
    body: &serde_json::Value,
) -> Response {
    let parsed = agentic_afk_orchestrator::ParsedPlanningOutput {
        is_empty: false,
        body: body.clone(),
    };
    let selections = match agentic_afk_orchestrator::extract_planner_selections(&parsed) {
        Ok(selections) => selections,
        Err(error) => {
            return fail_planning_phase(
                state,
                project_id,
                &plan_run.id,
                &error,
                "urn:agentic-afk:planning-output-unparseable",
            )
            .await;
        }
    };

    let snapshot = match persistence::get_planning_snapshot(&state.db, project_id).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return fail_planning_phase(
                state,
                project_id,
                &plan_run.id,
                &error.to_string(),
                "urn:agentic-afk:planning-snapshot-missing",
            )
            .await;
        }
    };

    let eligible_by_id: std::collections::HashMap<&str, &SourceIssueSnapshot> = snapshot
        .eligible
        .iter()
        .map(|issue| (issue.source_id.as_str(), issue))
        .collect();

    let exec_config_lookup =
        match persistence::get_project_execution_config(&state.db, project_id).await {
            Ok(Some(config)) => config,
            Ok(None) => {
                return fail_planning_phase(
                    state,
                    project_id,
                    &plan_run.id,
                    "Project Execution Config disappeared during Planning Phase",
                    "urn:agentic-afk:execution-config-missing",
                )
                .await;
            }
            Err(error) => return persistence_error_to_response(error),
        };

    // Issue #46: the Planning Phase may return up to Max Parallel Tasks
    // selections for one Plan Run. Selections beyond the configured cap
    // force the planner to converge for now rather than implicitly
    // truncating the batch.
    let max_parallel = exec_config_lookup.max_parallel_tasks.max(1) as usize;
    if selections.len() > max_parallel {
        return fail_planning_phase(
            state,
            project_id,
            &plan_run.id,
            &format!(
                "Planning Phase returned {} issues but Project Max Parallel Tasks is {}",
                selections.len(),
                max_parallel
            ),
            "urn:agentic-afk:planning-exceeds-max-parallel",
        )
        .await;
    }

    let Some(source) = project.enabled_issue_source.clone() else {
        return fail_planning_phase(
            state,
            project_id,
            &plan_run.id,
            "Project has no enabled Issue Source for claim write-back",
            "urn:agentic-afk:issue-source-missing",
        )
        .await;
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
            return fail_planning_phase(
                state,
                project_id,
                &plan_run.id,
                &format!(
                    "Planning Phase selected Source Issue {} which is not in the eligible set",
                    selection.source_issue_id
                ),
                "urn:agentic-afk:planning-selection-ineligible",
            )
            .await;
        };
        let assignment = match persistence::create_plan_run_assignment(
            &state.db,
            &plan_run.id,
            project_id,
            &source,
            issue,
            &selection.branch,
            &selection.selection_summary,
        )
        .await
        {
            Ok(assignment) => assignment,
            Err(error) => return persistence_error_to_response(error),
        };

        let worktree_path = match state.plan_run_deps.worktree.provision(
            project_path,
            &baseline.commit_sha,
            &selection.branch,
        ) {
            Ok(path) => path,
            Err(error) => {
                let _ = persistence::release_issue_assignment(&state.db, &assignment.id).await;
                return fail_planning_phase(
                    state,
                    project_id,
                    &plan_run.id,
                    &error.to_string(),
                    "urn:agentic-afk:assignment-worktree-failed",
                )
                .await;
            }
        };
        let worktree_path_str = worktree_path.to_string_lossy().into_owned();
        let assignment = match persistence::set_assignment_worktree(
            &state.db,
            &assignment.id,
            &worktree_path_str,
        )
        .await
        {
            Ok(assignment) => assignment,
            Err(error) => return persistence_error_to_response(error),
        };

        if let Err(error) = state
            .plan_run_deps
            .lifecycle
            .write_claimed(&issue.source_id)
        {
            let _ = persistence::release_issue_assignment(&state.db, &assignment.id).await;
            return fail_planning_phase(
                state,
                project_id,
                &plan_run.id,
                &error.to_string(),
                "urn:agentic-afk:issue-source-lifecycle-failed",
            )
            .await;
        }

        crate::project_event_publisher::publish_assignment_created(
            &state.event_bus,
            project_id,
            assignment.clone(),
        );
        claimed.push(assignment);
    }

    let phase_output = match persistence::record_plan_run_phase_output(
        &state.db,
        &plan_run.id,
        "planning",
        "succeeded",
        body,
    )
    .await
    {
        Ok(output) => output,
        Err(error) => return persistence_error_to_response(error),
    };
    crate::project_event_publisher::publish_plan_run_phase_completed(
        &state.event_bus,
        project_id,
        &plan_run.id,
        phase_output,
    );

    // Issue #46: drive implementation+review for every claimed assignment
    // concurrently (bounded by Max Parallel Tasks since claim already
    // capped). Each task owns its own Review Loop and finishes with a
    // per-assignment `AssignmentBatchOutcome`. The merge phase runs
    // sequentially across reviewed successful assignments afterward so
    // the Integration Branch sees one merge at a time.
    let outcomes = match run_parallel_implement_review(
        state,
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
        Err(response) => {
            // A hard phase failure short-circuits the parallel tranche.
            // Finish the Plan Run as `failed` so the Dashboard records
            // the terminal state alongside the per-assignment phase
            // failure already persisted by `fail_assignment_phase`.
            if let Ok(run) =
                persistence::finish_plan_run(&state.db, &plan_run.id, "failed").await
            {
                crate::project_event_publisher::publish_plan_run_completed(
                    &state.event_bus,
                    project_id,
                    run,
                );
            }
            return response;
        }
    };

    finalize_parallel_plan_run(
        state,
        project,
        project_id,
        plan_run,
        baseline,
        &exec_config_lookup,
        outcomes,
    )
    .await
}

/// Per-assignment outcome of the parallel implementation+review tranche
/// (issue #46). The merge phase then consumes the reviewed successes one at
/// a time so the Integration Branch sees a single merge attempt per
/// reviewed assignment.
#[derive(Clone, Debug)]
struct AssignmentBatchOutcome {
    assignment: IssueAssignmentResponse,
    /// Phase Output body recorded for the approving Review Phase, used as
    /// the merge prompt's reviewed evidence. `None` when the assignment
    /// blocked before reaching `reviewed`.
    review_body: Option<serde_json::Value>,
}

async fn run_parallel_implement_review(
    state: &AppState,
    project: &ProjectResponse,
    project_id: &str,
    plan_run: &PlanRunResponse,
    baseline: &RefreshedBaseline,
    exec_config: &ProjectExecutionConfigResponse,
    claimed: &[IssueAssignmentResponse],
) -> Result<Vec<AssignmentBatchOutcome>, Response> {
    use tokio::task::JoinSet;

    let mut join_set: JoinSet<(
        IssueAssignmentResponse,
        Result<Option<serde_json::Value>, Response>,
    )> = JoinSet::new();
    for assignment in claimed {
        let state = state.clone();
        let project = project.clone();
        let project_id = project_id.to_string();
        let plan_run = plan_run.clone();
        let baseline = baseline.clone();
        let exec_config = exec_config.clone();
        let assignment = assignment.clone();
        join_set.spawn(async move {
            let outcome = run_assignment_implement_review(
                &state,
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
                return Err(sync_problem_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "urn:agentic-afk:assignment-task-panic",
                    "Internal Server Error",
                    format!("assignment task panicked: {error}"),
                ));
            }
        };
        let review_body = outcome?;
        // Re-read the assignment so the latest status (reviewed / blocked)
        // is captured even if the orchestrator updated it after the task
        // returned.
        let refreshed = match persistence::get_assignment(&state.db, &assignment.id).await {
            Ok(value) => value,
            Err(error) => return Err(persistence_error_to_response(error)),
        };
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
async fn run_assignment_implement_review(
    state: &AppState,
    project: &ProjectResponse,
    project_id: &str,
    plan_run: &PlanRunResponse,
    baseline: &RefreshedBaseline,
    exec_config: &ProjectExecutionConfigResponse,
    assignment: &IssueAssignmentResponse,
) -> Result<Option<serde_json::Value>, Response> {
    let project_instructions = load_project_instructions(&project.path);
    let raw_text = persistence::get_assignment_source_raw_text(&state.db, &assignment.id)
        .await
        .map_err(persistence_error_to_response)?;

    let mut review_findings = String::new();
    let mut loop_iteration: i64 = 0;
    loop {
        loop_iteration += 1;

        let assignment_state =
            persistence::set_assignment_status(&state.db, &assignment.id, "implementing", None)
                .await
                .map_err(persistence_error_to_response)?;
        crate::project_event_publisher::publish_assignment_status_changed(
            &state.event_bus,
            project_id,
            assignment_state,
        );

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
        let impl_stdout = match state.plan_run_deps.implementation.run(&impl_prompt) {
            Ok(stdout) => stdout,
            Err(error) => {
                return Err(fail_assignment_phase(
                    state,
                    project_id,
                    assignment,
                    "implementation",
                    &error.to_string(),
                    "urn:agentic-afk:implementation-phase-failed",
                )
                .await);
            }
        };
        let impl_parsed = match agentic_afk_orchestrator::parse_implementation_output(&impl_stdout)
        {
            Ok(parsed) => parsed,
            Err(error) => {
                return Err(fail_assignment_phase(
                    state,
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
                state,
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
            &state.db,
            &plan_run.id,
            &assignment.id,
            "implementation",
            &impl_parsed.outcome,
            &impl_parsed.body,
        )
        .await
        .map_err(persistence_error_to_response)?;
        crate::project_event_publisher::publish_plan_run_phase_completed(
            &state.event_bus,
            project_id,
            &plan_run.id,
            impl_output,
        );

        let assignment_state =
            persistence::set_assignment_status(&state.db, &assignment.id, "implemented", None)
                .await
                .map_err(persistence_error_to_response)?;
        crate::project_event_publisher::publish_assignment_status_changed(
            &state.event_bus,
            project_id,
            assignment_state,
        );

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
        let review_stdout = match state.plan_run_deps.review.run(&review_prompt) {
            Ok(stdout) => stdout,
            Err(error) => {
                return Err(fail_assignment_phase(
                    state,
                    project_id,
                    assignment,
                    "review",
                    &error.to_string(),
                    "urn:agentic-afk:review-phase-failed",
                )
                .await);
            }
        };
        let review_parsed = match agentic_afk_orchestrator::parse_review_output(&review_stdout) {
            Ok(parsed) => parsed,
            Err(error) => {
                return Err(fail_assignment_phase(
                    state,
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
            &state.db,
            &plan_run.id,
            &assignment.id,
            "review",
            &review_parsed.outcome,
            &review_parsed.body,
        )
        .await
        .map_err(persistence_error_to_response)?;
        crate::project_event_publisher::publish_plan_run_phase_completed(
            &state.event_bus,
            project_id,
            &plan_run.id,
            review_output,
        );

        if review_parsed.outcome == "approved" {
            let reviewed =
                persistence::set_assignment_status(&state.db, &assignment.id, "reviewed", None)
                    .await
                    .map_err(persistence_error_to_response)?;
            crate::project_event_publisher::publish_assignment_status_changed(
                &state.event_bus,
                project_id,
                reviewed,
            );
            return Ok(Some(review_parsed.body));
        }

        let rejection_count = persistence::increment_review_rejection(&state.db, &assignment.id)
            .await
            .map_err(persistence_error_to_response)?;
        review_findings = extract_review_findings(&review_parsed.body);

        if rejection_count >= exec_config.review_retry_limit {
            let reason = format!(
                "Review Loop exhausted: {rejection_count} rejection(s) reached the Project Review Retry Limit ({}).",
                exec_config.review_retry_limit
            );
            block_assignment_for_loop(state, project, project_id, assignment, &reason).await?;
            return Ok(None);
        }
        if loop_iteration > exec_config.review_retry_limit + 1 {
            let reason =
                format!("Review Loop ran {loop_iteration} iterations without converging; blocking.");
            block_assignment_for_loop(state, project, project_id, assignment, &reason).await?;
            return Ok(None);
        }
    }
}

/// Block an assignment that exhausted its Review Loop without finishing
/// the surrounding Plan Run. Used by the parallel tranche so blocked
/// assignments stay outside the Merge Phase while reviewed peers continue.
async fn block_assignment_for_loop(
    state: &AppState,
    project: &ProjectResponse,
    project_id: &str,
    assignment: &IssueAssignmentResponse,
    reason: &str,
) -> Result<(), Response> {
    let blocked = persistence::block_assignment(&state.db, &assignment.id, reason)
        .await
        .map_err(persistence_error_to_response)?;
    crate::project_event_publisher::publish_assignment_status_changed(
        &state.event_bus,
        project_id,
        blocked,
    );
    if let Some(source) = project.enabled_issue_source.as_ref() {
        if let Err(error) = write_assignment_lifecycle(
            &state.config.gh_binary_path,
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
/// and return a Response so the caller can short-circuit. Unlike
/// `fail_assignment` this does NOT finish the surrounding Plan Run — the
/// parallel orchestrator finishes the Plan Run once all peers have
/// completed.
async fn fail_assignment_phase(
    state: &AppState,
    project_id: &str,
    assignment: &IssueAssignmentResponse,
    phase: &str,
    error: &str,
    problem_type: &str,
) -> Response {
    let _ = persistence::record_assignment_phase_output(
        &state.db,
        assignment.plan_run_id.as_deref().unwrap_or_default(),
        &assignment.id,
        phase,
        "failed",
        &serde_json::json!({ "error": error }),
    )
    .await;
    if let Ok(updated) =
        persistence::set_assignment_status(&state.db, &assignment.id, "blocked", Some(error)).await
    {
        crate::project_event_publisher::publish_assignment_status_changed(
            &state.event_bus,
            project_id,
            updated,
        );
    }
    sync_problem_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        problem_type,
        "Internal Server Error",
        error.to_string(),
    )
}

/// Finish a Plan Run after the parallel implementation + review tranche
/// finishes. Merges reviewed assignments one at a time, cleans both merged
/// and blocked worktrees, and writes the appropriate terminal Plan Run
/// state. Mixed outcomes finish as `succeeded` since reviewed work merged;
/// only all-blocked Plan Runs finish as `failed`.
async fn finalize_parallel_plan_run(
    state: &AppState,
    project: &ProjectResponse,
    project_id: &str,
    plan_run: &PlanRunResponse,
    baseline: &RefreshedBaseline,
    exec_config: &ProjectExecutionConfigResponse,
    outcomes: Vec<AssignmentBatchOutcome>,
) -> Response {
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
                let merging = match persistence::set_assignment_status(
                    &state.db,
                    &merge_assignment.id,
                    "merging",
                    None,
                )
                .await
                {
                    Ok(value) => value,
                    Err(error) => return persistence_error_to_response(error),
                };
                crate::project_event_publisher::publish_assignment_status_changed(
                    &state.event_bus,
                    project_id,
                    merging.clone(),
                );

                let prompt = render_merge_prompt(
                    &project_instructions,
                    project,
                    plan_run,
                    baseline,
                    exec_config,
                    &merge_assignment,
                    review_body,
                );
                let merge_stdout = match state.plan_run_deps.merger.run(&prompt) {
                    Ok(stdout) => stdout,
                    Err(error) => {
                        let _ = persistence::record_assignment_phase_output(
                            &state.db,
                            &plan_run.id,
                            &merge_assignment.id,
                            "merge",
                            "failed",
                            &serde_json::json!({ "error": error.to_string() }),
                        )
                        .await;
                        let blocked = match persistence::block_assignment(
                            &state.db,
                            &merge_assignment.id,
                            &error.to_string(),
                        )
                        .await
                        {
                            Ok(value) => value,
                            Err(error) => return persistence_error_to_response(error),
                        };
                        crate::project_event_publisher::publish_assignment_status_changed(
                            &state.event_bus,
                            project_id,
                            blocked.clone(),
                        );
                        blocked_assignments.push(blocked);
                        continue;
                    }
                };
                let merge_parsed =
                    match agentic_afk_orchestrator::parse_merge_output(&merge_stdout) {
                        Ok(parsed) => parsed,
                        Err(error) => {
                            let _ = persistence::record_assignment_phase_output(
                                &state.db,
                                &plan_run.id,
                                &merge_assignment.id,
                                "merge",
                                "failed",
                                &serde_json::json!({ "error": error }),
                            )
                            .await;
                            let blocked = match persistence::block_assignment(
                                &state.db,
                                &merge_assignment.id,
                                &error,
                            )
                            .await
                            {
                                Ok(value) => value,
                                Err(error) => return persistence_error_to_response(error),
                            };
                            crate::project_event_publisher::publish_assignment_status_changed(
                                &state.event_bus,
                                project_id,
                                blocked.clone(),
                            );
                            blocked_assignments.push(blocked);
                            continue;
                        }
                    };
                let merge_output = match persistence::record_assignment_phase_output(
                    &state.db,
                    &plan_run.id,
                    &merge_assignment.id,
                    "merge",
                    &merge_parsed.outcome,
                    &merge_parsed.body,
                )
                .await
                {
                    Ok(value) => value,
                    Err(error) => return persistence_error_to_response(error),
                };
                crate::project_event_publisher::publish_plan_run_phase_completed(
                    &state.event_bus,
                    project_id,
                    &plan_run.id,
                    merge_output,
                );

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
                    let blocked = match persistence::block_assignment(
                        &state.db,
                        &merge_assignment.id,
                        &reason,
                    )
                    .await
                    {
                        Ok(value) => value,
                        Err(error) => return persistence_error_to_response(error),
                    };
                    crate::project_event_publisher::publish_assignment_status_changed(
                        &state.event_bus,
                        project_id,
                        blocked.clone(),
                    );
                    if let Some(source) = project.enabled_issue_source.as_ref() {
                        if let Err(error) = write_assignment_lifecycle(
                            &state.config.gh_binary_path,
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
                let merged = match persistence::set_assignment_status(
                    &state.db,
                    &merge_assignment.id,
                    "merged",
                    None,
                )
                .await
                {
                    Ok(value) => value,
                    Err(error) => return persistence_error_to_response(error),
                };
                crate::project_event_publisher::publish_assignment_status_changed(
                    &state.event_bus,
                    project_id,
                    merged.clone(),
                );
                // PRD #35: Source Issue completion only after the verified
                // Integration Branch push. The completed lifecycle write-back
                // is deferred until after `pusher.push` succeeds below.
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
        if let Err(error) = state
            .plan_run_deps
            .pusher
            .push(project_path, &exec_config.integration_branch)
        {
            // Push failure: surface the error but keep the merged
            // assignments as `merged` (the integration already happened
            // locally). The Plan Run finishes as `failed` so the
            // developer notices.
            for assignment in &merged_assignments {
                let _ = persistence::record_assignment_phase_output(
                    &state.db,
                    &plan_run.id,
                    &assignment.id,
                    "merge",
                    "failed",
                    &serde_json::json!({ "error": error.to_string() }),
                )
                .await;
            }
            if let Ok(run) =
                persistence::finish_plan_run(&state.db, &plan_run.id, "failed").await
            {
                crate::project_event_publisher::publish_plan_run_completed(
                    &state.event_bus,
                    project_id,
                    run,
                );
            }
            return sync_problem_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "urn:agentic-afk:integration-branch-push-failed",
                "Internal Server Error",
                error.to_string(),
            );
        }

        // Push succeeded: now complete the Source Issues. PRD #35 requires
        // completion only after the verified Integration Branch push so
        // upstream lifecycle state never claims work the developer did not
        // actually receive.
        if let Some(source) = project.enabled_issue_source.as_ref() {
            for assignment in &merged_assignments {
                if let Err(error) = write_assignment_lifecycle(
                    &state.config.gh_binary_path,
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
    // worktrees (issue #46 acceptance criterion). Phase Outputs are
    // already durable on the Plan Run row, so cleanup is safe.
    let mut cleanup_targets: Vec<IssueAssignmentResponse> = Vec::new();
    cleanup_targets.extend(merged_assignments.iter().cloned());
    cleanup_targets.extend(blocked_assignments.iter().cloned());
    for assignment in &cleanup_targets {
        if assignment.worktree_path.is_empty() {
            continue;
        }
        let worktree_path = std::path::Path::new(&assignment.worktree_path);
        if let Err(error) = state.plan_run_deps.cleaner.cleanup(
            project_path,
            worktree_path,
            &assignment.branch,
        ) {
            eprintln!(
                "warning: failed to clean up Assignment Worktree for {} after Plan Run finish: {error}",
                assignment.source_id
            );
        }
    }

    // Plan Run terminal state: any merged → succeeded (partial-success
    // path covered by acceptance criterion #3). All-blocked / nothing
    // reviewed → failed. Empty selections never reach this function (the
    // empty-planning path returns earlier).
    let terminal_state = if merged_count > 0 {
        "succeeded"
    } else if reviewed_count == 0 && outcomes.iter().all(|o| o.review_body.is_none()) {
        "failed"
    } else {
        "failed"
    };

    let finished = match persistence::finish_plan_run(&state.db, &plan_run.id, terminal_state)
        .await
    {
        Ok(run) => run,
        Err(error) => return persistence_error_to_response(error),
    };
    crate::project_event_publisher::publish_plan_run_completed(
        &state.event_bus,
        project_id,
        finished,
    );

    let refreshed = match persistence::get_plan_run(&state.db, &plan_run.id).await {
        Ok(run) => run,
        Err(error) => return persistence_error_to_response(error),
    };
    (StatusCode::CREATED, Json(refreshed)).into_response()
}

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
    let template = include_str!("../../../crates/orchestrator/prompts/plan-run/merge.md");
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

fn load_project_instructions(project_path: &str) -> String {
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
    let template = include_str!("../../../crates/orchestrator/prompts/plan-run/implement.md");
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
    let template = include_str!("../../../crates/orchestrator/prompts/plan-run/review.md");
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

async fn fail_planning_phase(
    state: &AppState,
    project_id: &str,
    plan_run_id: &str,
    error: &str,
    problem_type: &str,
) -> Response {
    let _ = persistence::record_plan_run_phase_output(
        &state.db,
        plan_run_id,
        "planning",
        "failed",
        &serde_json::json!({ "error": error }),
    )
    .await;
    if let Ok(run) = persistence::finish_plan_run(&state.db, plan_run_id, "failed").await {
        crate::project_event_publisher::publish_plan_run_completed(
            &state.event_bus,
            project_id,
            run,
        );
    }
    sync_problem_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        problem_type,
        "Internal Server Error",
        error.to_string(),
    )
}

fn render_planning_prompt(
    project_instructions: &str,
    project: &ProjectResponse,
    config: &ProjectExecutionConfigResponse,
    baseline: &RefreshedBaseline,
    eligible: &[SourceIssueSnapshot],
) -> String {
    let template = include_str!("../../../crates/orchestrator/prompts/plan-run/plan.md");
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

async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

async fn api_docs() -> impl IntoResponse {
    Html(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>agentic-afk API</title>
  <script id="api-reference" data-url="/api/openapi.json"></script>
  <script src="https://cdn.jsdelivr.net/npm/@scalar/api-reference"></script>
</head>
<body></body>
</html>"#,
    )
}

async fn api_not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        [("content-type", "application/problem+json")],
        Json(ProblemDetail {
            problem_type: "urn:agentic-afk:not-found".to_string(),
            title: "Not Found".to_string(),
            status: 404,
            detail: "API route not found".to_string(),
        }),
    )
        .into_response()
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_markdown_parser_normalizes_scheduling_metadata_only() {
        let issue = parse_local_markdown_issue(
            "issue-10".to_string(),
            "# Sync local issues\n\nReadiness: READY\nParent Issue: #8\nIssue Dependencies: #9, local-3\nSource Order: 10\n\n## Acceptance criteria\n- preserve this raw text\n".to_string(),
            99,
        );

        assert_eq!(issue.source_id, "issue-10");
        assert_eq!(issue.title, "Sync local issues");
        assert_eq!(issue.readiness, "ready");
        assert_eq!(issue.lifecycle_status, "ready");
        assert_eq!(issue.parent_issue.as_deref(), Some("8"));
        assert_eq!(issue.issue_dependencies, vec!["9", "local-3"]);
        assert_eq!(issue.source_order, 10);
        assert!(issue.raw_text.contains("preserve this raw text"));
    }

    #[test]
    fn local_markdown_parser_uses_file_order_and_not_ready_defaults() {
        let issue = parse_local_markdown_issue(
            "fallback-id".to_string(),
            "Body without metadata".to_string(),
            4,
        );

        assert_eq!(issue.title, "fallback-id");
        assert_eq!(issue.readiness, "not-ready");
        assert_eq!(issue.lifecycle_status, "ready");
        assert_eq!(issue.parent_issue, None);
        assert!(issue.issue_dependencies.is_empty());
        assert_eq!(issue.source_order, 4);
    }

    #[test]
    fn lifecycle_write_back_replaces_existing_line() {
        let raw = "# Title\n\nReadiness: ready\nLifecycle Status: ready\n\nBody".to_string();
        let updated = update_markdown_lifecycle_status(raw, "claimed");
        assert!(updated.contains("Lifecycle Status: claimed"));
        assert!(!updated.contains("Lifecycle Status: ready"));
        assert!(updated.contains("Readiness: ready"));
        assert!(updated.contains("Body"));
    }

    #[test]
    fn lifecycle_write_back_adds_line_when_missing() {
        let raw = "# Title\n\nReadiness: ready\n\nBody".to_string();
        let updated = update_markdown_lifecycle_status(raw, "running");
        assert!(updated.contains("Lifecycle Status: running"));
        assert!(updated.contains("Readiness: ready"));
        assert!(updated.contains("Body"));
    }

    #[test]
    fn lifecycle_write_back_adds_line_after_title_when_no_other_metadata() {
        let raw = "# Title\n\nBody".to_string();
        let updated = update_markdown_lifecycle_status(raw, "blocked");
        assert!(updated.contains("Lifecycle Status: blocked"));
        assert!(updated.starts_with("# Title\n"));
        assert!(updated.contains("Body"));
    }

    #[test]
    fn lifecycle_write_back_preserves_raw_text_with_no_title() {
        let raw = "Just body text".to_string();
        let updated = update_markdown_lifecycle_status(raw, "completed");
        assert!(updated.contains("Lifecycle Status: completed"));
        assert!(updated.contains("Just body text"));
    }

    #[test]
    fn local_markdown_parser_reads_all_lifecycle_statuses() {
        for (value, expected) in [
            ("claimed", "claimed"),
            ("running", "running"),
            ("blocked", "blocked"),
            ("completed", "completed"),
            ("ready", "ready"),
            ("READY", "ready"),
            ("CLAIMED", "claimed"),
            ("bogus", "ready"),
        ] {
            let raw = format!("# Title\n\nLifecycle Status: {}\n", value);
            let issue = parse_local_markdown_issue("id".to_string(), raw, 1);
            assert_eq!(
                issue.lifecycle_status, expected,
                "lifecycle_status for input '{}' should be '{}'",
                value, expected
            );
        }
    }
}
