use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use agentic_afk_contracts::{
    AppInfoResponse, AssignmentAttemptResponse, AssignmentTerminalOutcome, AutoReplanState,
    CreateProjectRequest, EffectiveConfig, EnableIssueSourceRequest, HealthResponse,
    IssueAssignmentResponse, IssueSource, IssueSourceCandidate, IssueSourceSyncResponse,
    IssueSourceSyncStatusResponse, PauseReason, PlanningSnapshotResponse, ProblemDetail,
    ProjectActivityEntryResponse, ProjectEvent, ProjectId, ProjectResponse, ProjectSnapshot,
    ProjectSnapshotResponse, SetProjectExecutionConfigRequest, SourceIssueSnapshot,
};
use agentic_afk_git_summary::summarize_project_path;
pub use agentic_afk_orchestrator::{
    AssignmentWorktreeCleaner, AssignmentWorktreeProvisioner, CoordinatorError, EventPublisher,
    FakeAssignmentWorktreeCleaner, FakeImplementationPhaseRunner, FakeIntegrationBranchPusher,
    FakeLifecycleWriter, FakeMergePhaseRunner, FakePlanningPhaseRunner, FakeReviewPhaseRunner,
    FakeWorktreeProvisioner, GitAssignmentWorktreeCleaner, GitIntegrationBranchPusher,
    GitIntegrationBranchRefresher, ImplementationPhaseRunner, IntegrationBranchPusher,
    IntegrationBranchRefresher, IssueLifecycleWriter, LifecycleStatus, MergePhaseRunner,
    PerSourceImplementationPhaseRunner, PerSourceMergePhaseRunner, PerSourceReviewPhaseRunner,
    PlanRunDeps, PlanRunPhaseError, PlannerSelection,
    PlanningPhaseRunner, RefreshedBaseline,
    ReviewPhaseRunner, StaticIntegrationBranchRefresher, UnimplementedAssignmentWorktreeCleaner,
    UnimplementedImplementationPhaseRunner, UnimplementedIntegrationBranchPusher,
    UnimplementedIntegrationBranchRefresher, UnimplementedLifecycleWriter,
    UnimplementedMergePhaseRunner, UnimplementedPlanningPhaseRunner,
    UnimplementedReviewPhaseRunner, UnimplementedWorktreeProvisioner,
    WorktrunkAssignmentWorktreeProvisioner, update_markdown_lifecycle_status,
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

pub mod control_plane_events;
pub mod event_bus;

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

// `PlanRunDeps` lives in the orchestrator crate (issue #48). Tests and
// production wiring both go through the orchestrator's typed seam.

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

fn plan_run_deps_from_env(config: &ControlPlaneConfig) -> PlanRunDeps {
    let mode = std::env::var("AGENTIC_AFK_TEST_PLAN_RUN_STUBS").ok();
    let Some(mode) = mode else {
        // Production wiring: real git refresher / pusher / cleaner, the
        // Worktrunk-backed provisioner, the four Codex phase runners
        // (resolved per Plan Run against the project path), and the GH /
        // local-markdown lifecycle writer.
        return PlanRunDeps::production(
            config.worktrunk_binary_path.clone(),
            config.codex_binary_path.clone(),
            config.gh_binary_path.clone(),
        );
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
    PlanRunDeps::default_test_deps()
}

/// Build a router that shares a caller-provided `EventBus`. Useful for
/// tests that need to publish activity through the same bus the SSE
/// endpoint subscribes from.
pub fn router_with_bus(config: ControlPlaneConfig, db: Db, event_bus: event_bus::EventBus) -> Router {
    let deps = plan_run_deps_from_env(&config);
    router_with_full_deps(config, db, event_bus, deps)
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

/// Same as `router_with_plan_run_merge_deps` but accepts a shared
/// `EventBus`, so tests can subscribe to the SSE delta seam (issue #51 /
/// ADR-0037) while driving the implementation → review → merge → push
/// flow end-to-end.
#[allow(clippy::too_many_arguments)]
pub fn router_with_plan_run_merge_deps_and_bus(
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
    event_bus: event_bus::EventBus,
) -> Router {
    router_with_full_deps(
        config,
        db,
        event_bus,
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
            production_codex_binary: None,
            production_gh_binary: None,
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
            production_codex_binary: None,
            production_gh_binary: None,
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
        .route("/api/projects/{id}/auto-replan/arm", post(arm_auto_replan))
        .route(
            "/api/projects/{id}/auto-replan/disarm",
            post(disarm_auto_replan),
        )
        .route(
            "/api/projects/{id}/auto-replan/resume",
            post(resume_auto_replan),
        )
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
            "/api/projects/{id}/source-issues/{source_id}/re-enable",
            post(re_enable_source_issue),
        )
        .route(
            "/api/projects/{id}/assignments/{assignment_id}/retry-push",
            post(retry_push_assignment),
        )
        .route(
            "/api/projects/{id}/assignments/{assignment_id}/abandon-staged",
            post(abandon_staged_assignment),
        )
        .route(
            "/api/projects/{id}/source-issues/{source_id}/prd",
            post(mark_prd).delete(unmark_prd),
        )
        .route("/api/{*path}", get(api_not_found).post(api_not_found))
        .fallback_service(ServeDir::new(asset_dir).fallback(ServeFile::new(index)))
        .with_state(state)
}

pub async fn serve(config: ControlPlaneConfig) -> anyhow::Result<()> {
    let db = persistence::connect(&config.database_url).await?;
    persistence::migrate(&db).await?;

    // ADR-0042 S2: synchronous boot recovery between migrations and the
    // HTTP bind. Reconciles any in_flight/interrupted phase rows left
    // behind by a prior orchestrator process (SIGTERM, crash, OOM) so
    // dashboards reconnect into a clean state instead of staring at
    // assignments stuck at `implementing` with no live coordinator.
    let _recovery = agentic_afk_orchestrator::boot_recovery_scanner::run(
        &db,
        &agentic_afk_orchestrator::boot_recovery_scanner::NoopEventPublisher,
    )
    .await?;

    let listener = tokio::net::TcpListener::bind(config.bind_address).await?;
    let local_addr = listener.local_addr()?;
    eprintln!("agentic-afk Local Control Plane listening on http://{local_addr}");
    // ADR-0042 S1: install the ShutdownCoordinator as axum's graceful-
    // shutdown future. On SIGTERM/SIGINT it flips every `in_flight`
    // phase row to `interrupted`, SIGTERMs the captured child PIDs, and
    // sleeps the configured grace window before axum tears down the
    // listener — leaving a clean recovery surface for the next boot.
    let shutdown_db = db.clone();
    axum::serve(listener, router(config, db))
        .with_graceful_shutdown(async move {
            let report = agentic_afk_orchestrator::shutdown_coordinator::await_shutdown(
                shutdown_db,
            )
            .await;
            if report.rows_marked_interrupted > 0 {
                eprintln!(
                    "shutdown: marked {} in-flight phase row(s) interrupted; signalled {} PID(s)",
                    report.rows_marked_interrupted,
                    report.pids_signalled.len()
                );
            }
        })
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
            crate::control_plane_events::publish_project_changed(
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
            crate::control_plane_events::publish_project_changed(
                &state.event_bus,
                &id,
                project.clone(),
            );
            let candidates = discover_issue_source_candidates(&project);
            crate::control_plane_events::publish_issue_source_candidates_changed(
                &state.event_bus,
                &id,
                candidates,
            );
            crate::control_plane_events::publish_planning_snapshot_changed(
                &state.event_bus,
                &id,
                persistence::get_planning_snapshot(&state.db, &id)
                    .await
                    .ok()
                    .map(agentic_afk_planning_snapshot::normalize),
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

    let result =
        crate::control_plane_events::during_issue_source_sync(&state.event_bus, &id, || async {
            let issues = match match source.kind.as_str() {
                "local_markdown" => read_local_markdown_issues(&project.path, &source.locator),
                "github" => read_github_issues(&state.config.gh_binary_path, &source.locator),
                _ => Err(format!(
                    "manual sync is not supported for {} Issue Sources yet",
                    source.kind
                )),
            } {
                Ok(issues) => issues,
                Err(detail) => {
                    let _ =
                        persistence::record_sync_failure(&state.db, &id, &source, &detail).await;
                    return Err(crate::control_plane_events::SyncErr::Source(detail));
                }
            };

            let synced_at = current_sync_timestamp();
            let response = match persistence::replace_planning_snapshot(
                &state.db, &id, &source, &issues, &synced_at,
            )
            .await
            {
                Ok(response) => response,
                Err(e) => {
                    // Best-effort mirror the failure into the sync-status row
                    // so the Dashboard "last failure" surface matches the
                    // source-fetch failure path.
                    let _ = persistence::record_sync_failure(
                        &state.db,
                        &id,
                        &source,
                        &format!("failed to persist Planning Snapshot: {e}"),
                    )
                    .await;
                    return Err(crate::control_plane_events::SyncErr::Persistence(e));
                }
            };

            let planning = persistence::get_planning_snapshot(&state.db, &id)
                .await
                .ok()
                .map(agentic_afk_planning_snapshot::normalize);
            crate::control_plane_events::publish_planning_snapshot_changed(
                &state.event_bus,
                &id,
                planning,
            );
            Ok(response)
        })
        .await;

    match result {
        Ok(response) => Json(response).into_response(),
        Err(crate::control_plane_events::SyncErr::Source(detail)) => sync_problem_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "urn:agentic-afk:issue-source-sync-failed",
            "Unprocessable Entity",
            detail,
        ),
        Err(crate::control_plane_events::SyncErr::Persistence(e)) => {
            persistence_error_to_response(e)
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
        Ok(raw) => Json(agentic_afk_planning_snapshot::normalize(raw)).into_response(),
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

async fn arm_auto_replan(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let project = match persistence::get_project(&state.db, &id).await {
        Ok(project) => project,
        Err(error) => return persistence_error_to_response(error),
    };
    if !project.trusted {
        return sync_problem_response(
            StatusCode::FORBIDDEN,
            "urn:agentic-afk:project-untrusted",
            "Forbidden",
            "Project must be trusted before arming Auto-Replan".to_string(),
        );
    }
    match project.auto_replan_state {
        AutoReplanState::Off => {
            auto_replan_transition(&state, &id, AutoReplanState::Armed, None, "AutoReplanArmed")
                .await
        }
        AutoReplanState::Armed | AutoReplanState::Paused => auto_replan_conflict(
            "Auto-Replan is not off",
            "Auto-Replan can only be armed from Off".to_string(),
        ),
    }
}

async fn disarm_auto_replan(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    if let Err(error) = persistence::get_project(&state.db, &id).await {
        return persistence_error_to_response(error);
    }
    auto_replan_transition(
        &state,
        &id,
        AutoReplanState::Off,
        None,
        "AutoReplanDisarmed",
    )
    .await
}

async fn resume_auto_replan(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let project = match persistence::get_project(&state.db, &id).await {
        Ok(project) => project,
        Err(error) => return persistence_error_to_response(error),
    };
    match project.auto_replan_state {
        AutoReplanState::Paused => {
            auto_replan_transition(
                &state,
                &id,
                AutoReplanState::Armed,
                None,
                "AutoReplanResumed",
            )
            .await
        }
        AutoReplanState::Off | AutoReplanState::Armed => auto_replan_conflict(
            "Auto-Replan is not paused",
            "Auto-Replan can only be resumed from Paused".to_string(),
        ),
    }
}

async fn pause_auto_replan_for_test(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(request): Json<TestPauseAutoReplanRequest>,
) -> Response {
    if let Err(error) = persistence::get_project(&state.db, &id).await {
        return persistence_error_to_response(error);
    }
    auto_replan_transition(
        &state,
        &id,
        AutoReplanState::Paused,
        Some(request.reason),
        "AutoReplanPaused",
    )
    .await
}

async fn auto_replan_transition(
    state: &Arc<AppState>,
    project_id: &str,
    next: AutoReplanState,
    reason: Option<PauseReason>,
    activity_kind: &str,
) -> Response {
    let store = persistence::AutoReplanStateStore::new(&state.db);
    let status = match store.set(project_id, next, reason).await {
        Ok(status) => status,
        Err(error) => return persistence_error_to_response(error),
    };
    if let Err(error) = crate::control_plane_events::record_activity(
        &state.db,
        &state.event_bus,
        project_id,
        None,
        activity_kind,
        reason.map(PauseReason::as_wire),
    )
    .await
    {
        return persistence_error_to_response(error);
    }
    crate::control_plane_events::publish_auto_replan_state_changed(
        &state.event_bus,
        project_id,
        status.state,
        status.pause_reason,
    );
    match persistence::get_project(&state.db, project_id).await {
        Ok(project) => Json(with_git_summary(project)).into_response(),
        Err(error) => persistence_error_to_response(error),
    }
}

fn auto_replan_conflict(title: &str, detail: String) -> Response {
    sync_problem_response(
        StatusCode::CONFLICT,
        "urn:agentic-afk:auto-replan-state-conflict",
        title,
        detail,
    )
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
        Ok(raw) => Some(agentic_afk_planning_snapshot::normalize(raw)),
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

#[derive(Debug, Deserialize)]
struct TestPauseAutoReplanRequest {
    reason: PauseReason,
}

/// Mounted only when `AGENTIC_AFK_TEST_ENDPOINTS=1`. Records a Project
/// Activity entry via the production `control_plane_events`, so Playwright can
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
        .route(
            "/api/_test/projects/{id}/auto-replan/pause",
            post(pause_auto_replan_for_test),
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
            if let Ok(raw) = persistence::get_planning_snapshot(&state.db, &id).await {
                crate::control_plane_events::publish_planning_snapshot_changed(
                    &state.event_bus,
                    &id,
                    Some(agentic_afk_planning_snapshot::normalize(raw)),
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
    match crate::control_plane_events::record_activity(
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

    let updated_text =
        agentic_afk_orchestrator::update_markdown_lifecycle_status(raw_text, &request.lifecycle_status);

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

// `update_markdown_lifecycle_status` moved to the orchestrator crate
// (issue #48); use `agentic_afk_orchestrator::update_markdown_lifecycle_status`.

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
        PersistenceError::PhaseOutputMismatch { .. } => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "urn:agentic-afk:phase-output-mismatch",
            "Internal Server Error",
        ),
        PersistenceError::InvalidAutoReplanState { .. }
        | PersistenceError::InvalidPauseReason { .. }
        | PersistenceError::InvalidAutoReplanTransition(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "urn:agentic-afk:auto-replan-persistence-error",
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
            crate::control_plane_events::publish_project_execution_config_changed(
                &state.event_bus,
                &id,
                config.clone(),
            );
            Json(config).into_response()
        }
        Err(error) => persistence_error_to_response(error),
    }
}

/// Source-Issue-keyed human re-enable (issue #55 / ADR-0038). Clears
/// the latest blocked Issue Assignment for the Source Issue (if one
/// still exists) and writes Lifecycle `Ready` back to the **Issue
/// Source** so the next Plan Run's Planning Snapshot buckets the
/// Source Issue as `eligible` instead of `active`. Write-back is
/// best-effort per ADR-0035: a failed upstream write does not abort
/// the local clear, surfaces as `writeback.ok == false`, and emits a
/// `lifecycle_writeback_failed` Project Activity entry. HTTP 200 even
/// when `writeback.ok == false`.
async fn re_enable_source_issue(
    State(state): State<Arc<AppState>>,
    Path((id, source_id)): Path<(String, String)>,
) -> Response {
    let project = match persistence::get_project(&state.db, &id).await {
        Ok(project) => with_git_summary(project),
        Err(error) => return persistence_error_to_response(error),
    };

    let resolved_deps = agentic_afk_orchestrator::coordinator::resolve_deps_for_project(
        &state.plan_run_deps,
        &project,
    );
    let events: Arc<dyn agentic_afk_orchestrator::EventPublisher> =
        Arc::new(EventBusPublisher::new(
            state.event_bus.clone(),
            id.clone(),
            state.db.clone(),
        ));

    match agentic_afk_orchestrator::re_enable_source_issue(
        &state.db,
        &events,
        &resolved_deps.lifecycle,
        &project,
        &source_id,
    )
    .await
    {
        Ok(outcome) => {
            let body = agentic_afk_contracts::ReEnableSourceIssueResponse {
                local_cleared: outcome.local_cleared,
                writeback: match outcome.writeback {
                    Ok(()) => agentic_afk_contracts::WritebackOutcomeResponse {
                        ok: true,
                        error: None,
                    },
                    Err(error) => agentic_afk_contracts::WritebackOutcomeResponse {
                        ok: false,
                        error: Some(error.0),
                    },
                },
            };
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(error) => coordinator_error_to_response(error),
    }
}

/// Operator-initiated Retry Push for a `merge_staged` Issue Assignment
/// (issue #53 / ADR-0037). Re-runs `git push` only — no fetch, no rebase,
/// no re-verify. Returns the typed Retry Push outcome plus the post-retry
/// Assignment shape so the Dashboard can update without an additional
/// fetch.
async fn retry_push_assignment(
    State(state): State<Arc<AppState>>,
    Path((id, assignment_id)): Path<(String, String)>,
) -> Response {
    let assignment = match persistence::get_project_assignment(&state.db, &id, &assignment_id).await
    {
        Ok(assignment) => assignment,
        Err(error) => return persistence_error_to_response(error),
    };
    let project = match persistence::get_project(&state.db, &id).await {
        Ok(project) => with_git_summary(project),
        Err(error) => return persistence_error_to_response(error),
    };

    let events: Arc<dyn agentic_afk_orchestrator::EventPublisher> =
        Arc::new(EventBusPublisher::new(
            state.event_bus.clone(),
            id.clone(),
            state.db.clone(),
        ));
    let resolved_deps = agentic_afk_orchestrator::coordinator::resolve_deps_for_project(
        &state.plan_run_deps,
        &project,
    );
    match agentic_afk_orchestrator::retry_push(
        &state.db,
        &events,
        &resolved_deps,
        &project,
        &assignment,
    )
    .await
    {
        Ok(result) => {
            let body = agentic_afk_contracts::RetryPushResponse {
                status: result.status.clone(),
                block_reason: result.block_reason.clone(),
            };
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(error) => coordinator_error_to_response(error),
    }
}

/// Operator-initiated Abandon Staged for a `merge_staged` Issue Assignment
/// (issue #54 / ADR-0037). Transitions `merge_staged` → `blocked` with
/// `BlockReason::AbandonedStaged` without any push attempt. The optional
/// `{ note }` becomes the freeform Block Reason `detail`. Worktree +
/// issue-branch cleanup proceeds because the assignment is now terminal.
async fn abandon_staged_assignment(
    State(state): State<Arc<AppState>>,
    Path((id, assignment_id)): Path<(String, String)>,
    body: Option<Json<agentic_afk_contracts::AbandonStagedRequest>>,
) -> Response {
    let assignment = match persistence::get_project_assignment(&state.db, &id, &assignment_id).await
    {
        Ok(assignment) => assignment,
        Err(error) => return persistence_error_to_response(error),
    };
    let project = match persistence::get_project(&state.db, &id).await {
        Ok(project) => with_git_summary(project),
        Err(error) => return persistence_error_to_response(error),
    };

    let note = body.and_then(|Json(req)| req.note).and_then(|note| {
        let trimmed = note.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

    let events: Arc<dyn agentic_afk_orchestrator::EventPublisher> =
        Arc::new(EventBusPublisher::new(
            state.event_bus.clone(),
            id.clone(),
            state.db.clone(),
        ));
    let resolved_deps = agentic_afk_orchestrator::coordinator::resolve_deps_for_project(
        &state.plan_run_deps,
        &project,
    );
    match agentic_afk_orchestrator::abandon_staged(
        &state.db,
        &events,
        &resolved_deps,
        &project,
        &assignment,
        note,
    )
    .await
    {
        Ok(result) => {
            let body = agentic_afk_contracts::AbandonStagedResponse {
                status: result.status.clone(),
                block_reason: result.block_reason.clone(),
            };
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(error) => coordinator_error_to_response(error),
    }
}

/// Mark a Source Issue as a Parent-Issue-style PRD so it is hidden from every
/// active Planning Snapshot bucket. The marking is local to this Project; it
/// is not written back to the upstream Issue Source.
async fn mark_prd(
    State(state): State<Arc<AppState>>,
    Path((id, source_id)): Path<(String, String)>,
) -> Response {
    if let Err(error) = persistence::get_project(&state.db, &id).await {
        return persistence_error_to_response(error);
    }
    if let Err(error) = persistence::mark_prd(&state.db, &id, &source_id).await {
        return persistence_error_to_response(error);
    }
    publish_planning_snapshot_after_prd_change(&state, &id).await;
    StatusCode::NO_CONTENT.into_response()
}

/// Remove a PRD marking so the Source Issue is once again bucketed normally on
/// the Planning Snapshot.
async fn unmark_prd(
    State(state): State<Arc<AppState>>,
    Path((id, source_id)): Path<(String, String)>,
) -> Response {
    if let Err(error) = persistence::get_project(&state.db, &id).await {
        return persistence_error_to_response(error);
    }
    if let Err(error) = persistence::unmark_prd(&state.db, &id, &source_id).await {
        return persistence_error_to_response(error);
    }
    publish_planning_snapshot_after_prd_change(&state, &id).await;
    StatusCode::NO_CONTENT.into_response()
}

/// Republish the Project's Planning Snapshot after a PRD marking change so
/// live Dashboards re-bucket without a manual refresh. Missing snapshot
/// (Project without an enabled Issue Source) is a no-op.
async fn publish_planning_snapshot_after_prd_change(state: &Arc<AppState>, project_id: &str) {
    let snapshot = persistence::get_planning_snapshot(&state.db, project_id)
        .await
        .ok()
        .map(agentic_afk_planning_snapshot::normalize);
    crate::control_plane_events::publish_planning_snapshot_changed(
        &state.event_bus,
        project_id,
        snapshot,
    );
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
            // Platform defaults seed the Project Execution Config on first
            // Plan Run so a developer can start unattended work without a
            // separate config step. Integration Branch defaults from the
            // detected Git default branch (falling back to `main`); Max
            // Parallel Tasks and Review Retry Limit get conservative
            // defaults the developer can override later.
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
                    crate::control_plane_events::publish_project_execution_config_changed(
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
    crate::control_plane_events::publish_plan_run_started(
        &state.event_bus,
        &id,
        plan_run.clone(),
    );

    // Delegate phase orchestration to the coordinator in the orchestrator
    // crate (issue #48). The handler keeps responsibility for request
    // validation, baseline refresh, row creation, and response mapping; the
    // coordinator owns planning, parallel implementation+review, merge,
    // push, cleanup, and source lifecycle transitions.
    let events: Arc<dyn agentic_afk_orchestrator::EventPublisher> =
        Arc::new(EventBusPublisher::new(
            state.event_bus.clone(),
            id.clone(),
            state.db.clone(),
        ));
    let inputs = agentic_afk_orchestrator::PlanRunInputs::new(
        project.clone(),
        plan_run.clone(),
        baseline.clone(),
        execution_config.clone(),
    );
    let effects = agentic_afk_orchestrator::PlanRunEffects {
        db: state.db.clone(),
        events,
        deps: state.plan_run_deps.clone(),
        gh_binary_path: state.config.gh_binary_path.clone(),
    };
    match agentic_afk_orchestrator::run_plan_run(&inputs, &effects).await
    {
        Ok(finished) => (StatusCode::CREATED, Json(finished)).into_response(),
        Err(error) => coordinator_error_to_response(error),
    }
}

/// Adapter that lets the orchestrator's `EventPublisher` trait publish to
/// the control-plane server's per-Project event bus through the existing
/// `control_plane_events` helpers.
struct EventBusPublisher {
    bus: event_bus::EventBus,
    project_id: String,
    /// Database handle used by `record_activity` to persist Project
    /// Activity entries through `control_plane_events::record_activity`.
    db: Db,
}

impl EventBusPublisher {
    fn new(bus: event_bus::EventBus, project_id: String, db: Db) -> Self {
        Self {
            bus,
            project_id,
            db,
        }
    }
}

impl agentic_afk_orchestrator::EventPublisher for EventBusPublisher {
    fn plan_run_started(
        &self,
        _project_id: &str,
        plan_run: agentic_afk_contracts::PlanRunResponse,
    ) {
        crate::control_plane_events::publish_plan_run_started(
            &self.bus,
            &self.project_id,
            plan_run,
        );
    }
    fn plan_run_completed(
        &self,
        _project_id: &str,
        plan_run: agentic_afk_contracts::PlanRunResponse,
    ) {
        crate::control_plane_events::publish_plan_run_completed(
            &self.bus,
            &self.project_id,
            plan_run,
        );
    }
    fn plan_run_phase_completed(
        &self,
        _project_id: &str,
        plan_run_id: &str,
        phase_output: agentic_afk_contracts::PhaseOutputResponse,
    ) {
        crate::control_plane_events::publish_plan_run_phase_completed(
            &self.bus,
            &self.project_id,
            plan_run_id,
            phase_output,
        );
    }
    fn assignment_created(
        &self,
        _project_id: &str,
        assignment: agentic_afk_contracts::IssueAssignmentResponse,
    ) {
        crate::control_plane_events::publish_assignment_created(
            &self.bus,
            &self.project_id,
            assignment,
        );
    }
    fn assignment_status_changed(
        &self,
        _project_id: &str,
        assignment: agentic_afk_contracts::IssueAssignmentResponse,
    ) {
        crate::control_plane_events::publish_assignment_status_changed(
            &self.bus,
            &self.project_id,
            assignment,
        );
    }
    fn record_activity(
        &self,
        _project_id: &str,
        assignment_id: Option<&str>,
        kind: &str,
        detail: Option<&str>,
    ) {
        // ADR-0035: post-claim lifecycle write-back failures are
        // surfaced as Project Activity rather than aborting the Plan
        // Run. The write itself is best-effort; spawn it so the
        // synchronous coordinator does not have to await DB I/O at
        // every lifecycle failure site. If the spawn outlives the
        // current Tokio runtime (test shutdown, etc) the activity is
        // lost — that is acceptable for the best-effort surface.
        let db = self.db.clone();
        let bus = self.bus.clone();
        let project_id = self.project_id.clone();
        let assignment_id = assignment_id.map(str::to_string);
        let kind = kind.to_string();
        let detail = detail.map(str::to_string);
        tokio::spawn(async move {
            if let Err(error) = crate::control_plane_events::record_activity(
                &db,
                &bus,
                &project_id,
                assignment_id.as_deref(),
                &kind,
                detail.as_deref(),
            )
            .await
            {
                eprintln!(
                    "warning: failed to record Project Activity entry ({kind}): {error}"
                );
            }
        });
    }
}

/// Map a `CoordinatorError` from the orchestrator crate back to an
/// RFC-7807 HTTP response.
fn coordinator_error_to_response(error: agentic_afk_orchestrator::CoordinatorError) -> Response {
    let status =
        StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let title = status.canonical_reason().unwrap_or("Error").to_string();
    sync_problem_response(status, &error.problem_type, &title, error.detail)
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
        let updated = agentic_afk_orchestrator::update_markdown_lifecycle_status(raw, "claimed");
        assert!(updated.contains("Lifecycle Status: claimed"));
        assert!(!updated.contains("Lifecycle Status: ready"));
        assert!(updated.contains("Readiness: ready"));
        assert!(updated.contains("Body"));
    }

    #[test]
    fn lifecycle_write_back_adds_line_when_missing() {
        let raw = "# Title\n\nReadiness: ready\n\nBody".to_string();
        let updated = agentic_afk_orchestrator::update_markdown_lifecycle_status(raw, "running");
        assert!(updated.contains("Lifecycle Status: running"));
        assert!(updated.contains("Readiness: ready"));
        assert!(updated.contains("Body"));
    }

    #[test]
    fn lifecycle_write_back_adds_line_after_title_when_no_other_metadata() {
        let raw = "# Title\n\nBody".to_string();
        let updated = agentic_afk_orchestrator::update_markdown_lifecycle_status(raw, "blocked");
        assert!(updated.contains("Lifecycle Status: blocked"));
        assert!(updated.starts_with("# Title\n"));
        assert!(updated.contains("Body"));
    }

    #[test]
    fn lifecycle_write_back_preserves_raw_text_with_no_title() {
        let raw = "Just body text".to_string();
        let updated = agentic_afk_orchestrator::update_markdown_lifecycle_status(raw, "completed");
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
