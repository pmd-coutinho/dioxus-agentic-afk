use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use agentic_afk_contracts::{AppInfoResponse, EffectiveConfig, HealthResponse};
use axum::extract::State;
use axum::response::{Html, IntoResponse};
use axum::routing::get;
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
}

#[derive(OpenApi)]
#[openapi(
    paths(health, app_info),
    components(schemas(HealthResponse, AppInfoResponse, EffectiveConfig)),
    tags((name = "Local Control Plane", description = "Local Control Plane API"))
)]
struct ApiDoc;

pub fn router(config: ControlPlaneConfig) -> Router {
    let asset_dir = config.dashboard_asset_dir.clone();
    let index = asset_dir.join("index.html");
    let state = Arc::new(AppState { config });

    Router::new()
        .route("/health", get(health))
        .route("/api/app-info", get(app_info))
        .route("/api/openapi.json", get(openapi_json))
        .route("/api/docs", get(api_docs))
        .fallback_service(ServeDir::new(asset_dir).fallback(ServeFile::new(index)))
        .with_state(state)
}

pub async fn serve(config: ControlPlaneConfig) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(config.bind_address).await?;
    let local_addr = listener.local_addr()?;
    eprintln!("agentic-afk Local Control Plane listening on http://{local_addr}");
    axum::serve(listener, router(config))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
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
