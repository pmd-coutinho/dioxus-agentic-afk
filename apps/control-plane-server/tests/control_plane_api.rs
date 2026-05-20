use agentic_afk_contracts::{CreateProjectRequest, HealthResponse, ProblemDetail, ProjectResponse};
use agentic_afk_control_plane_server::{ControlPlaneConfig, router};
use agentic_afk_persistence::{self as persistence};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use std::path::PathBuf;
use tower::ServiceExt;

async fn test_router() -> axum::Router {
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "apps/dashboard/dist".into(),
        database_url: "sqlite::memory:".into(),
    };
    router(config, db)
}

async fn test_router_with_db() -> (axum::Router, persistence::Db) {
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "apps/dashboard/dist".into(),
        database_url: "sqlite::memory:".into(),
    };
    (router(config, db.clone()), db)
}

#[tokio::test]
async fn local_control_plane_reports_health_and_truthful_app_info() {
    let app = test_router().await;

    let health_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(health_response.status(), StatusCode::OK);
    let health_body = health_response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    let health: HealthResponse = serde_json::from_slice(&health_body).unwrap();
    assert_eq!(health.status, "ok");

    let app_info_response = app
        .oneshot(
            Request::builder()
                .uri("/api/app-info")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(app_info_response.status(), StatusCode::OK);
    let app_info_body = app_info_response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    let app_info: agentic_afk_contracts::AppInfoResponse =
        serde_json::from_slice(&app_info_body).unwrap();
    assert_eq!(app_info.app_name, "agentic-afk");
    assert_eq!(app_info.api_status, "connected");
    assert_eq!(app_info.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(app_info.config.bind_address, "127.0.0.1:0");
    assert_eq!(app_info.config.dashboard_asset_dir, "apps/dashboard/dist");
    assert_eq!(app_info.config.database_url, "sqlite::memory:");
}

#[tokio::test]
async fn dashboard_shell_loads_from_the_local_control_plane() {
    let dashboard_asset_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/dashboard/dist")
        .canonicalize()
        .unwrap();
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir,
        database_url: "sqlite::memory:".into(),
    };

    let response = router(config, db)
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(body.contains("agentic-afk-dashboard"));
    assert!(body.contains("/api/app-info"));
    assert!(body.contains("/api/projects"));
    assert!(body.contains("Git Summary"));
}

#[tokio::test]
async fn openapi_document_describes_project_api_and_problem_responses() {
    let app = test_router().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let openapi: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(openapi["openapi"], "3.1.0");
    assert!(openapi["paths"]["/api/projects"]["post"].is_object());
    assert!(openapi["paths"]["/api/projects"]["get"].is_object());
    assert!(openapi["paths"]["/api/projects/{id}"]["get"].is_object());
    assert!(openapi["components"]["schemas"]["CreateProjectRequest"].is_object());
    assert!(openapi["components"]["schemas"]["ProjectResponse"].is_object());
    assert!(openapi["components"]["schemas"]["ProblemDetail"].is_object());
    assert!(
        openapi["paths"]["/api/projects"]["post"]["responses"]["409"]["content"]
            ["application/problem+json"]
            .is_object()
    );
    assert!(
        openapi["paths"]["/api/projects/{id}"]["get"]["responses"]["404"]["content"]
            ["application/problem+json"]
            .is_object()
    );
}

#[tokio::test]
async fn scalar_api_reference_loads_the_generated_openapi_document() {
    let app = test_router().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/docs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("@scalar/api-reference"));
    assert!(html.contains(r#"data-url="/api/openapi.json""#));
}

#[tokio::test]
async fn create_project_via_api() {
    let app = test_router().await;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/projects")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&CreateProjectRequest {
                        path: "/tmp".to_string(),
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let project: ProjectResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(project.path, "/tmp");
    assert!(!project.id.0.is_empty());
    assert_eq!(project.git_summary, None);
}

#[tokio::test]
async fn git_backed_project_response_includes_derived_git_summary() {
    let app = test_router().await;
    let project_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/projects")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&CreateProjectRequest {
                        path: project_path.display().to_string(),
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let project: ProjectResponse = serde_json::from_slice(&body).unwrap();
    let git_summary = project
        .git_summary
        .expect("Git-backed Project should include Git Summary");
    assert!(git_summary.branch.is_some());
    assert!(git_summary.head.is_some());
}

#[tokio::test]
async fn project_reads_include_derived_git_summary_without_persisting_it() {
    let (app, db) = test_router_with_db().await;
    let project_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let persisted = persistence::create_project(
        &db,
        &CreateProjectRequest {
            path: project_path.display().to_string(),
        },
    )
    .await
    .unwrap();
    assert_eq!(persisted.git_summary, None);

    let list_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/projects")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_body = list_response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    let projects: Vec<ProjectResponse> = serde_json::from_slice(&list_body).unwrap();
    assert!(projects[0].git_summary.is_some());

    let get_response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/projects/{}", persisted.id.0))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get_response.status(), StatusCode::OK);
    let get_body = get_response.into_body().collect().await.unwrap().to_bytes();
    let project: ProjectResponse = serde_json::from_slice(&get_body).unwrap();
    assert!(project.git_summary.is_some());
}

#[tokio::test]
async fn malformed_git_metadata_returns_graceful_no_summary_state() {
    let app = test_router().await;
    let project_path =
        std::env::temp_dir().join(format!("agentic-afk-malformed-git-{}", std::process::id()));
    std::fs::create_dir_all(&project_path).unwrap();
    std::fs::write(project_path.join(".git"), "not a gitdir").unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/projects")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&CreateProjectRequest {
                        path: project_path.display().to_string(),
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    std::fs::remove_dir_all(&project_path).unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let project: ProjectResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(project.git_summary, None);
}

#[tokio::test]
async fn list_projects_via_api_empty() {
    let app = test_router().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/projects")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let projects: Vec<ProjectResponse> = serde_json::from_slice(&body).unwrap();
    assert!(projects.is_empty());
}

#[tokio::test]
async fn get_project_not_found_returns_problem_json() {
    let app = test_router().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/projects/nonexistent-id")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "application/problem+json"
    );
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let problem: ProblemDetail = serde_json::from_slice(&body).unwrap();
    assert_eq!(problem.status, 404);
    assert_eq!(problem.problem_type, "urn:agentic-afk:not-found");
}

#[tokio::test]
async fn create_project_invalid_path_returns_problem_json() {
    let app = test_router().await;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/projects")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&CreateProjectRequest {
                        path: "/nonexistent/path".to_string(),
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "application/problem+json"
    );
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let problem: ProblemDetail = serde_json::from_slice(&body).unwrap();
    assert_eq!(problem.status, 422);
    assert_eq!(problem.problem_type, "urn:agentic-afk:invalid-path");
}

#[tokio::test]
async fn create_duplicate_project_returns_conflict() {
    let (app, _db) = test_router_with_db().await;

    // Create first
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/projects")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&CreateProjectRequest {
                        path: "/tmp".to_string(),
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Duplicate
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/projects")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&CreateProjectRequest {
                        path: "/tmp".to_string(),
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "application/problem+json"
    );
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let problem: ProblemDetail = serde_json::from_slice(&body).unwrap();
    assert_eq!(problem.status, 409);
    assert_eq!(problem.problem_type, "urn:agentic-afk:duplicate");
}
