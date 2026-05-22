use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use agentic_afk_contracts::{
    AppInfoResponse, AssignmentAttemptResponse, AssignmentTerminalOutcome, CreateProjectRequest,
    EffectiveConfig, EnableIssueSourceRequest, HealthResponse, IssueAssignmentResponse,
    IssueSource, IssueSourceCandidate, IssueSourceSyncResponse, IssueSourceSyncStatusResponse,
    PhaseOutputResponse, PlanRunResponse, PlanningSnapshotResponse, ProblemDetail,
    ProjectActivityEntryResponse, ProjectEvent, ProjectExecutionConfigResponse, ProjectId,
    ProjectResponse, ProjectSnapshot, ProjectSnapshotResponse,
    SetProjectExecutionConfigRequest, SourceIssueSnapshot,
};
use agentic_afk_git_summary::summarize_project_path;
use agentic_afk_orchestrator::{
    codex_process_identity, create_assignment_worktree, preflight_binary, run_initial_codex,
};
pub use agentic_afk_orchestrator::{
    FakePlanningPhaseRunner, IntegrationBranchRefresher, PlanRunPhaseError, PlanningPhaseRunner,
    RefreshedBaseline, StaticIntegrationBranchRefresher, UnimplementedIntegrationBranchRefresher,
    UnimplementedPlanningPhaseRunner,
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
}

impl PlanRunDeps {
    pub fn unimplemented() -> Self {
        Self {
            refresher: Arc::new(UnimplementedIntegrationBranchRefresher),
            planner: Arc::new(UnimplementedPlanningPhaseRunner),
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
    if std::env::var("AGENTIC_AFK_TEST_PLAN_RUN_STUBS").as_deref() == Ok("1") {
        let refresher = Arc::new(StaticIntegrationBranchRefresher::new(RefreshedBaseline {
            commit_sha: "test-baseline".to_string(),
        }));
        let planner = Arc::new(FakePlanningPhaseRunner::with_stdout(
            r#"<plan>{"issues":[],"summary":"test stub: no eligible work"}</plan>"#,
        ));
        PlanRunDeps { refresher, planner }
    } else {
        PlanRunDeps::unimplemented()
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
        PlanRunDeps { refresher, planner },
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
    router_with_full_deps(config, db, event_bus, PlanRunDeps { refresher, planner })
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
}

/// Test-only: publish an arbitrary `ProjectEvent` for `id` so Playwright can
/// drive lifecycle flows (e.g. Start Assignment -> Attempt -> Proposal
/// Refreshed -> Verified) without needing worktrunk/codex binaries.
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


pub(crate) fn assignment_problem(problem_type: &str, detail: String) -> Response {
    sync_problem_response(
        StatusCode::UNPROCESSABLE_ENTITY,
        problem_type,
        "Unprocessable Entity",
        detail,
    )
}

fn assignment_branch(source: &IssueSource, source_id: &str) -> String {
    let identity = format!("{}-{source_id}", source.kind.replace('_', "-"));
    let identity = identity
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    format!("agentic-afk/{identity}")
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

fn preflight_github_assignment(
    gh_binary_path: &std::path::Path,
    project: &ProjectResponse,
    source: &IssueSource,
) -> Result<(), String> {
    preflight_github_auth(gh_binary_path)?;
    let project_locator = github_origin_locator(std::path::Path::new(&project.path))
        .ok_or_else(|| "Project Git remote does not identify a GitHub repository".to_string())?;
    if project_locator != source.locator {
        return Err(format!(
            "Project GitHub remote {project_locator} does not match Issue Source {}",
            source.locator
        ));
    }
    refresh_github_origin(std::path::Path::new(&project.path))?;
    ensure_github_lifecycle_labels(gh_binary_path, &source.locator)
}

fn refresh_github_origin(project_path: &std::path::Path) -> Result<(), String> {
    let output = Command::new("git")
        .current_dir(project_path)
        .args(["fetch", "--prune", "origin"])
        .output()
        .map_err(|error| format!("failed to refresh GitHub origin: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "failed to refresh GitHub origin before claim: {}",
            command_output(&output)
        ))
    }
}

fn github_origin_locator(project_path: &std::path::Path) -> Option<String> {
    let config = std::fs::read_to_string(project_path.join(".git/config")).ok()?;
    let mut in_origin = false;
    for line in config.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_origin = trimmed == r#"[remote "origin"]"#;
            continue;
        }
        if in_origin {
            let (key, url) = trimmed.split_once('=')?;
            if key.trim() == "url" {
                return github_locator_from_url(url.trim());
            }
        }
    }
    None
}

fn preflight_github_auth(gh_binary_path: &std::path::Path) -> Result<(), String> {
    let auth = Command::new(gh_binary_path)
        .args(["auth", "status"])
        .output()
        .map_err(|error| format!("failed to run gh auth status: {error}"))?;
    if auth.status.success() {
        Ok(())
    } else {
        Err(format!(
            "gh is not authenticated: {}",
            command_output(&auth)
        ))
    }
}

fn ensure_github_lifecycle_labels(
    gh_binary_path: &std::path::Path,
    locator: &str,
) -> Result<(), String> {
    for label in [
        "agentic-afk:claimed",
        "agentic-afk:running",
        "agentic-afk:blocked",
        "agentic-afk:completed",
    ] {
        let output = Command::new(gh_binary_path)
            .args([
                "label", "create", label, "--repo", locator, "--force", "--color", "57606a",
            ])
            .output()
            .map_err(|error| {
                format!("failed to provision GitHub lifecycle label {label}: {error}")
            })?;
        if !output.status.success() {
            return Err(format!(
                "failed to provision GitHub lifecycle label {label}: {}",
                command_output(&output)
            ));
        }
    }
    Ok(())
}

pub(crate) fn comment_github_issue(
    gh_binary_path: &std::path::Path,
    locator: &str,
    source_id: &str,
    body: &str,
) -> Result<(), String> {
    let output = Command::new(gh_binary_path)
        .args([
            "issue", "comment", source_id, "--repo", locator, "--body", body,
        ])
        .output()
        .map_err(|error| format!("failed to comment on GitHub Source Issue: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "failed to comment on GitHub Source Issue: {}",
            command_output(&output)
        ))
    }
}

fn create_github_change_proposal(
    gh_binary_path: &std::path::Path,
    source: &IssueSource,
    issue: &SourceIssueSnapshot,
    branch: &str,
    worktree_path: &std::path::Path,
) -> Result<String, String> {
    let push = Command::new("git")
        .current_dir(worktree_path)
        .args(["push", "--set-upstream", "origin", branch])
        .output()
        .map_err(|error| format!("failed to push Change Proposal branch: {error}"))?;
    if !push.status.success() {
        return Err(format!(
            "failed to push Change Proposal branch: {}",
            command_output(&push)
        ));
    }

    let body = format!("Fixes #{}\n\nCreated by agentic-afk.", issue.source_id);
    let proposal = Command::new(gh_binary_path)
        .args([
            "pr",
            "create",
            "--repo",
            &source.locator,
            "--head",
            branch,
            "--title",
            &issue.title,
            "--body",
            &body,
        ])
        .output()
        .map_err(|error| format!("failed to create GitHub Change Proposal: {error}"))?;
    if !proposal.status.success() {
        return Err(format!(
            "failed to create GitHub Change Proposal: {}",
            command_output(&proposal)
        ));
    }
    let url = String::from_utf8_lossy(&proposal.stdout).trim().to_string();
    if url.is_empty() {
        Err("GitHub Change Proposal creation did not return a URL".to_string())
    } else {
        comment_github_issue(
            gh_binary_path,
            &source.locator,
            &issue.source_id,
            &format!("Change Proposal created: {url}"),
        )?;
        Ok(url)
    }
}

pub(crate) async fn refresh_local_markdown_after_change(
    db: &Db,
    project: &ProjectResponse,
    source: &IssueSource,
) -> Result<(), String> {
    refresh_local_markdown_snapshot(db, project, source).await
}

pub(crate) fn write_assignment_lifecycle_for_abandon(
    gh_binary_path: &std::path::Path,
    project: &ProjectResponse,
    source: &IssueSource,
    source_id: &str,
) -> Result<(), String> {
    write_assignment_lifecycle(gh_binary_path, project, source, source_id, "ready")
}

async fn refresh_local_markdown_snapshot(
    db: &Db,
    project: &ProjectResponse,
    source: &IssueSource,
) -> Result<(), String> {
    let issues = read_local_markdown_issues(&project.path, &source.locator)?;
    persistence::replace_planning_snapshot(
        db,
        &project.id.0,
        source,
        &issues,
        &current_sync_timestamp(),
    )
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
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

pub(crate) fn write_github_lifecycle_pub(
    gh_binary_path: &std::path::Path,
    locator: &str,
    source_id: &str,
    lifecycle_status: &str,
) -> Result<(), String> {
    write_github_lifecycle(gh_binary_path, locator, source_id, lifecycle_status)
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
            return sync_problem_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "urn:agentic-afk:execution-config-missing",
                "Unprocessable Entity",
                "Project Execution Config must be set before starting a Plan Run".to_string(),
            );
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

    let prompt = render_planning_prompt(&project, &execution_config, &baseline);
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

    let outcome = if parsed.is_empty {
        "succeeded_empty"
    } else {
        "succeeded"
    };
    let phase_output = match persistence::record_plan_run_phase_output(
        &state.db,
        &plan_run.id,
        "planning",
        outcome,
        &parsed.body,
    )
    .await
    {
        Ok(output) => output,
        Err(error) => return persistence_error_to_response(error),
    };
    crate::project_event_publisher::publish_plan_run_phase_completed(
        &state.event_bus,
        &id,
        &plan_run.id,
        phase_output,
    );

    let finished_state = if parsed.is_empty {
        "succeeded_empty"
    } else {
        // Non-empty selection not exercised in this slice (#41); still finalize
        // so the Plan Run is not left running.
        "succeeded"
    };
    let finished = match persistence::finish_plan_run(&state.db, &plan_run.id, finished_state).await
    {
        Ok(run) => run,
        Err(error) => return persistence_error_to_response(error),
    };
    crate::project_event_publisher::publish_plan_run_completed(
        &state.event_bus,
        &id,
        finished.clone(),
    );
    (StatusCode::CREATED, Json(finished)).into_response()
}

fn render_planning_prompt(
    project: &ProjectResponse,
    config: &ProjectExecutionConfigResponse,
    baseline: &RefreshedBaseline,
) -> String {
    let template = include_str!("../../../crates/orchestrator/prompts/plan-run/plan.md");
    template
        .replace("{{PROJECT_INSTRUCTIONS}}", "")
        .replace("{{PROJECT_NAME}}", &project.path)
        .replace("{{INTEGRATION_BRANCH}}", &config.integration_branch)
        .replace("{{PLAN_RUN_BASELINE}}", &baseline.commit_sha)
        .replace(
            "{{MAX_PARALLEL_TASKS}}",
            &config.max_parallel_tasks.to_string(),
        )
        .replace("{{ELIGIBLE_SOURCE_ISSUES}}", "")
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
