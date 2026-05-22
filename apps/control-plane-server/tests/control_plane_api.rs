use agentic_afk_contracts::{
    CreateProjectRequest, EnableIssueSourceRequest, HealthResponse, IssueAssignmentResponse,
    PlanningSnapshotResponse, ProblemDetail, ProjectResponse,
    SourceIssueSnapshot,
};
use agentic_afk_control_plane_server::{ControlPlaneConfig, router};
use agentic_afk_persistence::{self as persistence};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use tower::ServiceExt;

fn temp_project_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("agentic-afk-{name}-{}", std::process::id()))
}

async fn test_router() -> axum::Router {
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "target/dx/agentic-afk-dashboard/release/web/public".into(),
        database_url: "sqlite::memory:".into(),
        gh_binary_path: "gh".into(),
        worktrunk_binary_path: "wt".into(),
        codex_binary_path: "codex".into(),
    };
    router(config, db)
}

async fn test_router_with_db() -> (axum::Router, persistence::Db) {
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "target/dx/agentic-afk-dashboard/release/web/public".into(),
        database_url: "sqlite::memory:".into(),
        gh_binary_path: "gh".into(),
        worktrunk_binary_path: "wt".into(),
        codex_binary_path: "codex".into(),
    };
    (router(config, db.clone()), db)
}

async fn test_router_with_execution(
    worktrunk_binary_path: PathBuf,
    codex_binary_path: PathBuf,
) -> (axum::Router, persistence::Db) {
    test_router_with_execution_and_gh(worktrunk_binary_path, codex_binary_path, "gh".into()).await
}

async fn test_router_with_execution_and_gh(
    worktrunk_binary_path: PathBuf,
    codex_binary_path: PathBuf,
    gh_binary_path: PathBuf,
) -> (axum::Router, persistence::Db) {
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "target/dx/agentic-afk-dashboard/release/web/public".into(),
        database_url: "sqlite::memory:".into(),
        gh_binary_path,
        worktrunk_binary_path,
        codex_binary_path,
    };
    (router(config, db.clone()), db)
}

fn write_fake_command(name: &str, body: &str) -> PathBuf {
    let path = temp_project_path(name);
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();
    }
    path
}

fn setup_git_project_with_remote(name: &str) -> (PathBuf, PathBuf) {
    let project_path = temp_project_path(name);
    let remote_path = temp_project_path(&format!("{name}-remote"));
    std::fs::create_dir_all(&project_path).unwrap();
    std::fs::create_dir_all(&remote_path).unwrap();
    run_git(&project_path, &["init", "-b", "main"]);
    run_git(&project_path, &["config", "user.name", "Agentic AFK Test"]);
    run_git(
        &project_path,
        &["config", "user.email", "agentic-afk@example.invalid"],
    );
    std::fs::write(project_path.join("README.md"), "test\n").unwrap();
    run_git(&project_path, &["add", "README.md"]);
    run_git(&project_path, &["commit", "-m", "initial"]);
    run_git(&remote_path, &["init", "--bare"]);
    run_git(
        &project_path,
        &["remote", "add", "origin", "git@github.com:owner/repo.git"],
    );
    run_git(
        &project_path,
        &[
            "remote",
            "set-url",
            "--push",
            "origin",
            remote_path.to_str().unwrap(),
        ],
    );
    let rewrite_key = format!("url.{}.insteadOf", remote_path.to_str().unwrap());
    run_git(
        &project_path,
        &["config", &rewrite_key, "git@github.com:owner/repo.git"],
    );
    run_git(&project_path, &["push", "-u", "origin", "main"]);
    (project_path, remote_path)
}

fn run_git(path: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
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
    assert_eq!(app_info.config.dashboard_asset_dir, "target/dx/agentic-afk-dashboard/release/web/public");
    assert_eq!(app_info.config.database_url, "sqlite::memory:");
}

#[tokio::test]
async fn dashboard_shell_loads_from_the_local_control_plane() {
    let dashboard_asset_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/dx/agentic-afk-dashboard/release/web/public")
        .canonicalize()
        .unwrap();
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir,
        database_url: "sqlite::memory:".into(),
        gh_binary_path: "gh".into(),
        worktrunk_binary_path: "wt".into(),
        codex_binary_path: "codex".into(),
    };

    let response = router(config, db)
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(body.contains(r#"<div id="main">"#));
    assert!(body.contains("agentic-afk-dashboard"));
    assert!(body.contains("/assets/"));
}

#[tokio::test]
async fn dashboard_browser_routes_fallback_without_claiming_api_paths() {
    let dashboard_asset_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/dx/agentic-afk-dashboard/release/web/public")
        .canonicalize()
        .unwrap();
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir,
        database_url: "sqlite::memory:".into(),
        gh_binary_path: "gh".into(),
        worktrunk_binary_path: "wt".into(),
        codex_binary_path: "codex".into(),
    };
    let app = router(config, db);

    for uri in ["/projects", "/projects/example-project-id", "/settings"] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("agentic-afk-dashboard"), "{uri}");
    }

    let api_response = app
        .oneshot(
            Request::builder()
                .uri("/api/not-a-dashboard-route")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(api_response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        api_response.headers().get("content-type").unwrap(),
        "application/problem+json"
    );
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
    assert!(openapi["paths"]["/api/projects/{id}/issue-source-candidates"]["get"].is_object());
    assert!(openapi["paths"]["/api/projects/{id}/trust"]["put"].is_object());
    assert!(openapi["paths"]["/api/projects/{id}/issue-source"]["put"].is_object());
    assert!(openapi["paths"]["/api/projects/{id}/issue-source/sync"]["post"].is_object());
    assert!(openapi["paths"]["/api/projects/{id}/issue-source/sync-status"]["get"].is_object());
    assert!(openapi["paths"]["/api/projects/{id}/planning-snapshot"]["get"].is_object());
    assert!(openapi["paths"]["/api/projects/{id}/activity"]["get"].is_object());
    assert!(openapi["components"]["schemas"]["ProjectActivityEntryResponse"].is_object());
    assert!(openapi["components"]["schemas"]["CreateProjectRequest"].is_object());
    assert!(openapi["components"]["schemas"]["EnableIssueSourceRequest"].is_object());
    assert!(openapi["components"]["schemas"]["IssueSource"].is_object());
    assert!(openapi["components"]["schemas"]["IssueSourceCandidate"].is_object());
    assert!(openapi["components"]["schemas"]["IssueSourceSyncResponse"].is_object());
    assert!(openapi["components"]["schemas"]["IssueSourceSyncStatusResponse"].is_object());
    assert!(openapi["components"]["schemas"]["PlanningSnapshotResponse"].is_object());
    assert!(openapi["components"]["schemas"]["IssueAssignmentResponse"].is_object());
    assert!(openapi["components"]["schemas"]["SourceIssueSnapshot"].is_object());
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
    assert!(
        openapi["paths"]["/api/projects/{id}/issue-source"]["put"]["responses"]["422"]["content"]
            ["application/problem+json"]
            .is_object()
    );
    assert!(
        openapi["paths"]["/api/projects/{id}/issue-source/sync"]["post"]["responses"]["422"]
            ["content"]["application/problem+json"]
            .is_object()
    );
    assert!(
        openapi["paths"]["/api/projects/{id}/issue-source/sync-status"]["get"]["responses"]["422"]
            ["content"]["application/problem+json"]
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

    assert_eq!(response.status(), StatusCode::CREATED);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let project: ProjectResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(project.path, "/tmp");
    assert!(!project.id.0.is_empty());
    assert_eq!(project.trusted, false);
    assert_eq!(project.git_summary, None);
    assert_eq!(project.trusted, false);

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
    assert_eq!(projects[0].trusted, false);

    let get_response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/projects/{}", project.id.0))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get_response.status(), StatusCode::OK);
    let get_body = get_response.into_body().collect().await.unwrap().to_bytes();
    let project: ProjectResponse = serde_json::from_slice(&get_body).unwrap();
    assert_eq!(project.trusted, false);
}

#[tokio::test]
async fn malformed_git_metadata_returns_graceful_no_summary_state() {
    let app = test_router().await;
    let project_path =
        std::env::temp_dir().join(format!("agentic-afk-malformed-git-{}", std::process::id()));
    std::fs::create_dir_all(&project_path).unwrap();
    std::fs::write(project_path.join(".git"), "not a gitdir").unwrap();

    let response = app
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
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/projects")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&CreateProjectRequest {
                        path: "/nonexistent/path/that/does/not/exist".to_string(),
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

#[tokio::test]
async fn trust_project_via_api() {
    let app = test_router().await;

    let response = app
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

    assert_eq!(response.status(), StatusCode::CREATED);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let project: ProjectResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(project.trusted, false);

    let trust_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/projects/{}/trust", project.id.0))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(trust_response.status(), StatusCode::OK);
    let trust_body = trust_response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    let trusted_project: ProjectResponse = serde_json::from_slice(&trust_body).unwrap();
    assert_eq!(trusted_project.trusted, true);
    assert_eq!(trusted_project.id, project.id);
}

#[tokio::test]
async fn trust_project_is_idempotent() {
    let app = test_router().await;

    let response = app
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

    assert_eq!(response.status(), StatusCode::CREATED);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let project: ProjectResponse = serde_json::from_slice(&body).unwrap();

    for _ in 0..2 {
        let trust_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/projects/{}/trust", project.id.0))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(trust_response.status(), StatusCode::OK);
        let trust_body = trust_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let trusted_project: ProjectResponse = serde_json::from_slice(&trust_body).unwrap();
        assert_eq!(trusted_project.trusted, true);
    }
}

#[tokio::test]
async fn trust_nonexistent_project_returns_404() {
    let app = test_router().await;

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/projects/nonexistent-id/trust")
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
async fn github_sync_excludes_ready_issues_with_active_afk_lifecycle_labels() {
    let project_path = temp_project_path("github-active-sync");
    std::fs::create_dir_all(&project_path).unwrap();
    let fake_gh = write_fake_command(
        "github-active-sync-fake-gh",
        "#!/bin/sh\nif [ \"$1\" = \"auth\" ]; then exit 0; fi\nif [ \"$1\" = \"issue\" ] && [ \"$2\" = \"list\" ]; then\n  printf '[{\"number\":21,\"title\":\"Running\",\"labels\":[{\"name\":\"ready-for-agent\"},{\"name\":\"agentic-afk:running\"}]},{\"number\":22,\"title\":\"Ready\",\"labels\":[{\"name\":\"ready-for-agent\"}]}]\\n'\n  exit 0\nfi\nexit 9\n",
    );
    let noop = write_fake_command("github-active-sync-noop", "#!/bin/sh\nexit 0\n");
    let (app, _db) = test_router_with_execution_and_gh(noop.clone(), noop, fake_gh).await;
    let project = create_trusted_github_project(&app, &project_path, "owner/repo").await;

    sync_issue_source(&app, &project).await;
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/projects/{}/planning-snapshot", project.id.0))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let snapshot: PlanningSnapshotResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(snapshot.eligible[0].source_id, "22");
    assert_eq!(snapshot.active[0].source_id, "21");
    assert_eq!(snapshot.active[0].readiness, "ready");
    assert_eq!(snapshot.active[0].lifecycle_status, "running");
}

#[tokio::test]
async fn project_list_includes_trusted_field() {
    let app = test_router().await;

    let response = app
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

    assert_eq!(response.status(), StatusCode::CREATED);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let created: ProjectResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(created.trusted, false);

    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/projects/{}/trust", created.id.0))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

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
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].trusted, true);

    let get_response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/projects/{}", created.id.0))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get_response.status(), StatusCode::OK);
    let get_body = get_response.into_body().collect().await.unwrap().to_bytes();
    let project: ProjectResponse = serde_json::from_slice(&get_body).unwrap();
    assert_eq!(project.trusted, true);
}

#[tokio::test]
async fn git_summary_and_trust_are_both_returned() {
    let app = test_router().await;

    let response = app
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

    assert_eq!(response.status(), StatusCode::CREATED);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let created: ProjectResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(created.trusted, false);
    assert_eq!(created.git_summary, None);

    let trust_response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/projects/{}/trust", created.id.0))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(trust_response.status(), StatusCode::OK);
    let trust_body = trust_response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    let trusted: ProjectResponse = serde_json::from_slice(&trust_body).unwrap();
    assert_eq!(trusted.trusted, true);
    assert_eq!(trusted.git_summary, None);
}

fn setup_local_markdown_project(name: &str) -> (PathBuf, PathBuf) {
    let project_path = temp_project_path(name);
    let issues_dir = project_path.join(".scratch/issues");
    std::fs::create_dir_all(&issues_dir).unwrap();
    (project_path, issues_dir)
}

async fn create_and_enable_local_markdown_project(
    app: &axum::Router,
    project_path: &std::path::Path,
) -> ProjectResponse {
    let response = app
        .clone()
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

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/projects/{}/issue-source", project.id.0))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&EnableIssueSourceRequest {
                        kind: "local_markdown".to_string(),
                        locator: ".scratch/issues".to_string(),
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    project
}

async fn create_trusted_local_markdown_project(
    app: &axum::Router,
    project_path: &std::path::Path,
) -> ProjectResponse {
    let project = create_and_enable_local_markdown_project(app, project_path).await;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/projects/{}/trust", project.id.0))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).unwrap()
}

async fn sync_local_markdown_project(app: &axum::Router, project: &ProjectResponse) {
    sync_issue_source(app, project).await;
}

async fn sync_issue_source(app: &axum::Router, project: &ProjectResponse) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/projects/{}/issue-source/sync", project.id.0))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

async fn create_trusted_github_project(
    app: &axum::Router,
    project_path: &std::path::Path,
    locator: &str,
) -> ProjectResponse {
    let create_response = app
        .clone()
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
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let body = create_response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    let project: ProjectResponse = serde_json::from_slice(&body).unwrap();
    let source_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/projects/{}/issue-source", project.id.0))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&EnableIssueSourceRequest {
                        kind: "github".to_string(),
                        locator: locator.to_string(),
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(source_response.status(), StatusCode::OK);
    let trust_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/projects/{}/trust", project.id.0))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(trust_response.status(), StatusCode::OK);
    let body = trust_response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    serde_json::from_slice(&body).unwrap()
}

async fn start_source_issue_assignment(
    app: &axum::Router,
    project: &ProjectResponse,
    source_id: &str,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/projects/{}/source-issues/{source_id}/assignment",
                    project.id.0
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn lifecycle_status_write_back_updates_markdown_file() {
    let (project_path, issues_dir) = setup_local_markdown_project("lifecycle-write-back");
    let issue_path = issues_dir.join("test-issue.md");
    std::fs::write(
        &issue_path,
        "# Test Issue\n\nReadiness: ready\n\nBody text here.\n",
    )
    .unwrap();

    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "target/dx/agentic-afk-dashboard/release/web/public".into(),
        database_url: "sqlite::memory:".into(),
        gh_binary_path: "gh".into(),
        worktrunk_binary_path: "wt".into(),
        codex_binary_path: "codex".into(),
    };
    let app = router(config, db.clone());

    let project = create_and_enable_local_markdown_project(&app, &project_path).await;

    // Sync first so the snapshot exists
    let sync_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/projects/{}/issue-source/sync", project.id.0))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(sync_response.status(), StatusCode::OK);

    // Update lifecycle status
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/api/projects/{}/source-issues/test-issue/lifecycle-status",
                    project.id.0
                ))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"lifecycle_status":"claimed"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let snapshot: SourceIssueSnapshot = serde_json::from_slice(&body).unwrap();
    assert_eq!(snapshot.lifecycle_status, "claimed");
    assert_eq!(snapshot.source_id, "test-issue");

    // Verify file was updated
    let updated_raw = std::fs::read_to_string(&issue_path).unwrap();
    assert!(updated_raw.contains("Lifecycle Status: claimed"));
    assert!(!updated_raw.contains("Lifecycle Status: ready"));
    assert!(updated_raw.contains("Readiness: ready"));
    assert!(updated_raw.contains("Body text here."));

    std::fs::remove_dir_all(&project_path).unwrap();
}

#[tokio::test]
async fn planning_snapshot_reflects_lifecycle_status_exclusions() {
    let (project_path, issues_dir) = setup_local_markdown_project("lifecycle-snapshot");
    std::fs::write(
        issues_dir.join("eligible.md"),
        "# Eligible\n\nReadiness: ready\n\nBody\n",
    )
    .unwrap();
    std::fs::write(
        issues_dir.join("claimed.md"),
        "# Claimed\n\nReadiness: ready\nLifecycle Status: claimed\n\nBody\n",
    )
    .unwrap();
    std::fs::write(
        issues_dir.join("completed.md"),
        "# Completed\n\nReadiness: ready\nLifecycle Status: completed\n\nBody\n",
    )
    .unwrap();
    std::fs::write(
        issues_dir.join("not-ready.md"),
        "# Not Ready\n\nReadiness: not-ready\n\nBody\n",
    )
    .unwrap();

    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "target/dx/agentic-afk-dashboard/release/web/public".into(),
        database_url: "sqlite::memory:".into(),
        gh_binary_path: "gh".into(),
        worktrunk_binary_path: "wt".into(),
        codex_binary_path: "codex".into(),
    };
    let app = router(config, db.clone());

    let project = create_and_enable_local_markdown_project(&app, &project_path).await;

    let sync_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/projects/{}/issue-source/sync", project.id.0))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(sync_response.status(), StatusCode::OK);

    let snapshot_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/projects/{}/planning-snapshot", project.id.0))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(snapshot_response.status(), StatusCode::OK);
    let body = snapshot_response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    let snapshot: PlanningSnapshotResponse = serde_json::from_slice(&body).unwrap();

    let eligible_ids: Vec<_> = snapshot
        .eligible
        .iter()
        .map(|i| i.source_id.clone())
        .collect();
    let active_ids: Vec<_> = snapshot
        .active
        .iter()
        .map(|i| i.source_id.clone())
        .collect();
    let completed_ids: Vec<_> = snapshot
        .completed
        .iter()
        .map(|i| i.source_id.clone())
        .collect();
    let non_ready_ids: Vec<_> = snapshot
        .non_ready
        .iter()
        .map(|i| i.source_id.clone())
        .collect();

    assert!(eligible_ids.contains(&"eligible".to_string()));
    assert!(active_ids.contains(&"claimed".to_string()));
    assert!(completed_ids.contains(&"completed".to_string()));
    assert!(non_ready_ids.contains(&"not-ready".to_string()));

    std::fs::remove_dir_all(&project_path).unwrap();
}

#[tokio::test]
async fn lifecycle_status_write_back_preserves_raw_text() {
    let (project_path, issues_dir) = setup_local_markdown_project("lifecycle-preserve");
    let original =
        "# Title\n\nReadiness: ready\nParent Issue: #5\n\nKeep this paragraph.\n\n- list item\n";
    std::fs::write(issues_dir.join("preserve.md"), original).unwrap();

    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "target/dx/agentic-afk-dashboard/release/web/public".into(),
        database_url: "sqlite::memory:".into(),
        gh_binary_path: "gh".into(),
        worktrunk_binary_path: "wt".into(),
        codex_binary_path: "codex".into(),
    };
    let app = router(config, db.clone());

    let project = create_and_enable_local_markdown_project(&app, &project_path).await;

    let sync_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/projects/{}/issue-source/sync", project.id.0))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(sync_response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/api/projects/{}/source-issues/preserve/lifecycle-status",
                    project.id.0
                ))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"lifecycle_status":"blocked"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let updated_raw = std::fs::read_to_string(issues_dir.join("preserve.md")).unwrap();
    assert!(updated_raw.contains("Lifecycle Status: blocked"));
    assert!(updated_raw.contains("Keep this paragraph."));
    assert!(updated_raw.contains("- list item"));
    assert!(updated_raw.contains("Parent Issue: #5"));

    std::fs::remove_dir_all(&project_path).unwrap();
}

#[tokio::test]
async fn lifecycle_status_write_back_rejects_nonexistent_project() {
    let app = test_router().await;

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/projects/nonexistent-id/source-issues/issue/lifecycle-status")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"lifecycle_status":"claimed"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let problem: ProblemDetail = serde_json::from_slice(&body).unwrap();
    assert_eq!(problem.status, 404);
}

#[tokio::test]
async fn lifecycle_status_write_back_rejects_nonexistent_issue() {
    let (project_path, _issues_dir) = setup_local_markdown_project("lifecycle-no-issue");

    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "target/dx/agentic-afk-dashboard/release/web/public".into(),
        database_url: "sqlite::memory:".into(),
        gh_binary_path: "gh".into(),
        worktrunk_binary_path: "wt".into(),
        codex_binary_path: "codex".into(),
    };
    let app = router(config, db.clone());

    let project = create_and_enable_local_markdown_project(&app, &project_path).await;

    let sync_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/projects/{}/issue-source/sync", project.id.0))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(sync_response.status(), StatusCode::OK);

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/api/projects/{}/source-issues/missing-issue/lifecycle-status",
                    project.id.0
                ))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"lifecycle_status":"claimed"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let problem: ProblemDetail = serde_json::from_slice(&body).unwrap();
    assert_eq!(problem.status, 404);

    std::fs::remove_dir_all(&project_path).unwrap();
}

#[tokio::test]
async fn lifecycle_status_write_back_rejects_invalid_status() {
    let (project_path, issues_dir) = setup_local_markdown_project("lifecycle-invalid");
    std::fs::write(
        issues_dir.join("issue.md"),
        "# Issue\n\nReadiness: ready\n\nBody\n",
    )
    .unwrap();

    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "target/dx/agentic-afk-dashboard/release/web/public".into(),
        database_url: "sqlite::memory:".into(),
        gh_binary_path: "gh".into(),
        worktrunk_binary_path: "wt".into(),
        codex_binary_path: "codex".into(),
    };
    let app = router(config, db.clone());

    let project = create_and_enable_local_markdown_project(&app, &project_path).await;

    let sync_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/projects/{}/issue-source/sync", project.id.0))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(sync_response.status(), StatusCode::OK);

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/api/projects/{}/source-issues/issue/lifecycle-status",
                    project.id.0
                ))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"lifecycle_status":"bogus"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let problem: ProblemDetail = serde_json::from_slice(&body).unwrap();
    assert_eq!(problem.status, 422);
    assert_eq!(
        problem.problem_type,
        "urn:agentic-afk:invalid-lifecycle-status"
    );

    std::fs::remove_dir_all(&project_path).unwrap();
}

#[tokio::test]
async fn lifecycle_status_write_back_rejects_github_source() {
    let (project_path, _issues_dir) = setup_local_markdown_project("lifecycle-github");

    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "target/dx/agentic-afk-dashboard/release/web/public".into(),
        database_url: "sqlite::memory:".into(),
        gh_binary_path: "gh".into(),
        worktrunk_binary_path: "wt".into(),
        codex_binary_path: "codex".into(),
    };
    let app = router(config, db.clone());

    let create_response = app
        .clone()
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
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let body = create_response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    let project: ProjectResponse = serde_json::from_slice(&body).unwrap();

    // Enable github source
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/projects/{}/issue-source", project.id.0))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&EnableIssueSourceRequest {
                        kind: "github".to_string(),
                        locator: "owner/repo".to_string(),
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/api/projects/{}/source-issues/1/lifecycle-status",
                    project.id.0
                ))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"lifecycle_status":"claimed"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let problem: ProblemDetail = serde_json::from_slice(&body).unwrap();
    assert_eq!(problem.status, 422);
    assert_eq!(
        problem.problem_type,
        "urn:agentic-afk:lifecycle-write-back-not-supported"
    );

    std::fs::remove_dir_all(&project_path).unwrap();
}
