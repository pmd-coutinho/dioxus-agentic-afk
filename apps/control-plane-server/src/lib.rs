use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use agentic_afk_contracts::{
    AppInfoResponse, CreateProjectRequest, EffectiveConfig, HealthResponse, ProblemDetail,
    ProjectResponse,
};
use agentic_afk_git_summary::summarize_project_path;
use agentic_afk_persistence::{self as persistence, Db, PersistenceError};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use tower_http::services::{ServeDir, ServeFile};
use utoipa::OpenApi;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControlPlaneConfig {
    pub bind_address: SocketAddr,
    pub dashboard_asset_dir: PathBuf,
    pub database_url: String,
}

impl ControlPlaneConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let bind_address = std::env::var("AGENTIC_AFK_BIND_ADDRESS")
            .unwrap_or_else(|_| "127.0.0.1:3637".to_string())
            .parse()?;
        let dashboard_asset_dir = std::env::var("AGENTIC_AFK_DASHBOARD_ASSET_DIR")
            .unwrap_or_else(|_| "apps/dashboard/dist".to_string())
            .into();
        let database_url = std::env::var("AGENTIC_AFK_DATABASE_URL")
            .unwrap_or_else(|_| "sqlite://agentic-afk.db".to_string());

        Ok(Self {
            bind_address,
            dashboard_asset_dir,
            database_url,
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
struct AppState {
    config: ControlPlaneConfig,
    db: Db,
}

#[derive(OpenApi)]
#[openapi(
    paths(health, app_info, create_project, list_projects, get_project),
    components(schemas(
        HealthResponse,
        AppInfoResponse,
        EffectiveConfig,
        CreateProjectRequest,
        ProjectResponse,
        ProblemDetail
    )),
    tags((name = "Local Control Plane", description = "Local Control Plane API"))
)]
struct ApiDoc;

pub fn router(config: ControlPlaneConfig, db: Db) -> Router {
    let asset_dir = config.dashboard_asset_dir.clone();
    let index = asset_dir.join("index.html");
    let state = Arc::new(AppState { config, db });

    Router::new()
        .route("/health", get(health))
        .route("/api/app-info", get(app_info))
        .route("/api/openapi.json", get(openapi_json))
        .route("/api/docs", get(api_docs))
        .route("/api/projects", post(create_project).get(list_projects))
        .route("/api/projects/{id}", get(get_project))
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

fn with_git_summary(mut project: ProjectResponse) -> ProjectResponse {
    project.git_summary = summarize_project_path(&project.path);
    project
}

fn persistence_error_to_response(err: PersistenceError) -> Response {
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
