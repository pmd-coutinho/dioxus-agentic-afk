use agentic_afk_contracts::{
    CreateProjectRequest, EnableIssueSourceRequest, HealthResponse, IssueAssignmentResponse,
    PlanningSnapshotResponse, ProblemDetail, ProjectAssignmentStateResponse, ProjectResponse,
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
use tower::ServiceExt;

fn temp_project_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("agentic-afk-{name}-{}", std::process::id()))
}

async fn test_router() -> axum::Router {
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "apps/dashboard/dist".into(),
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
        dashboard_asset_dir: "apps/dashboard/dist".into(),
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
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "apps/dashboard/dist".into(),
        database_url: "sqlite::memory:".into(),
        gh_binary_path: "gh".into(),
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
        .join("../../apps/dashboard/dist")
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
    assert!(openapi["paths"]["/api/projects/{id}/assignment-state"]["get"].is_object());
    assert!(
        openapi["paths"]["/api/projects/{id}/source-issues/{source_id}/assignment"]["post"]
            .is_object()
    );
    assert!(openapi["components"]["schemas"]["CreateProjectRequest"].is_object());
    assert!(openapi["components"]["schemas"]["EnableIssueSourceRequest"].is_object());
    assert!(openapi["components"]["schemas"]["IssueSource"].is_object());
    assert!(openapi["components"]["schemas"]["IssueSourceCandidate"].is_object());
    assert!(openapi["components"]["schemas"]["IssueSourceSyncResponse"].is_object());
    assert!(openapi["components"]["schemas"]["IssueSourceSyncStatusResponse"].is_object());
    assert!(openapi["components"]["schemas"]["PlanningSnapshotResponse"].is_object());
    assert!(openapi["components"]["schemas"]["IssueAssignmentResponse"].is_object());
    assert!(openapi["components"]["schemas"]["ProjectAssignmentStateResponse"].is_object());
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
async fn start_assignment_rejects_untrusted_project() {
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

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/projects/{}/source-issues/ready-issue/assignment",
                    project.id.0
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let problem: ProblemDetail = serde_json::from_slice(&body).unwrap();
    assert_eq!(problem.problem_type, "urn:agentic-afk:project-untrusted");
}

#[tokio::test]
async fn start_assignment_runs_local_markdown_issue_through_worktrunk_and_codex() {
    let (project_path, issues_dir) = setup_local_markdown_project("assignment-start");
    std::fs::create_dir_all(project_path.join(".git")).unwrap();
    std::fs::write(
        issues_dir.join("ready-issue.md"),
        "# First assignment\n\nReadiness: ready\nSource Order: 1\n\n## Acceptance criteria\n- start it\n",
    )
    .unwrap();

    let worktree_path = temp_project_path("assignment-worktree");
    let fake_wt = write_fake_command(
        "assignment-fake-wt",
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nmkdir -p '{}'\nprintf '{{\"path\":\"{}\"}}\\n'\n",
            worktree_path.display(),
            worktree_path.display()
        ),
    );
    let fake_codex = write_fake_command(
        "assignment-fake-codex",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nlast=''\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"--output-last-message\" ]; then shift; last=\"$1\"; fi\n  shift\ndone\nprintf '{\"outcome\":\"Blocked\",\"summary\":\"need input\"}\\n' > \"$last\"\n",
    );
    let (app, _db) = test_router_with_execution(fake_wt, fake_codex).await;
    let project = create_trusted_local_markdown_project(&app, &project_path).await;

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

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/projects/{}/source-issues/ready-issue/assignment",
                    project.id.0
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let assignment: IssueAssignmentResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(assignment.source_id, "ready-issue");
    assert_eq!(assignment.branch, "agentic-afk/local-markdown-ready-issue");
    assert_eq!(
        assignment.worktree_path,
        worktree_path.display().to_string()
    );
    assert_eq!(assignment.status, "blocked");
    assert_eq!(
        assignment
            .latest_attempt
            .unwrap()
            .terminal_outcome
            .unwrap()
            .outcome,
        "Blocked"
    );
    assert!(
        std::fs::read_to_string(issues_dir.join("ready-issue.md"))
            .unwrap()
            .contains("Lifecycle Status: blocked")
    );
}

#[tokio::test]
async fn assignment_state_shows_waiting_ready_issues_and_rejects_second_start() {
    let (project_path, issues_dir) = setup_local_markdown_project("assignment-waiting");
    std::fs::create_dir_all(project_path.join(".git")).unwrap();
    for (source_id, order) in [("first", 1), ("second", 2)] {
        std::fs::write(
            issues_dir.join(format!("{source_id}.md")),
            format!("# {source_id}\n\nReadiness: ready\nSource Order: {order}\n"),
        )
        .unwrap();
    }

    let fake_wt = write_fake_command(
        "assignment-waiting-fake-wt",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nworktree=\"/tmp/agentic-afk-waiting-worktree-$$\"\nmkdir -p \"$worktree\"\nprintf '{\"path\":\"%s\"}\\n' \"$worktree\"\n",
    );
    let fake_codex = write_fake_command(
        "assignment-waiting-fake-codex",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"--output-last-message\" ]; then shift; last=\"$1\"; fi\n  shift\ndone\nprintf '{\"outcome\":\"Blocked\",\"summary\":\"still active\"}\\n' > \"$last\"\n",
    );
    let (app, _db) = test_router_with_execution(fake_wt, fake_codex).await;
    let project = create_trusted_local_markdown_project(&app, &project_path).await;
    let sync = app
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
    assert_eq!(sync.status(), StatusCode::OK);

    let first_start = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/projects/{}/source-issues/first/assignment",
                    project.id.0
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first_start.status(), StatusCode::CREATED);

    let state = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/projects/{}/assignment-state", project.id.0))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(state.status(), StatusCode::OK);
    let body = state.into_body().collect().await.unwrap().to_bytes();
    let state: ProjectAssignmentStateResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(state.active_assignment.unwrap().source_id, "first");
    assert_eq!(state.waiting_ready_issue_count, 1);

    let second_start = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/projects/{}/source-issues/second/assignment",
                    project.id.0
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second_start.status(), StatusCode::CONFLICT);
    let body = second_start.into_body().collect().await.unwrap().to_bytes();
    let problem: ProblemDetail = serde_json::from_slice(&body).unwrap();
    assert_eq!(problem.problem_type, "urn:agentic-afk:active-assignment");
}

#[tokio::test]
async fn local_markdown_ready_for_proposal_blocks_without_a_change_proposal_target() {
    let (project_path, issues_dir) = setup_local_markdown_project("assignment-ready");
    std::fs::create_dir_all(project_path.join(".git")).unwrap();
    std::fs::write(
        issues_dir.join("ready.md"),
        "# Ready\n\nReadiness: ready\nSource Order: 1\n",
    )
    .unwrap();
    let fake_wt = write_fake_command(
        "assignment-ready-fake-wt",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nworktree=\"/tmp/agentic-afk-ready-worktree-$$\"\nmkdir -p \"$worktree\"\nprintf '{\"path\":\"%s\"}\\n' \"$worktree\"\n",
    );
    let fake_codex = write_fake_command(
        "assignment-ready-fake-codex",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"--output-last-message\" ]; then shift; last=\"$1\"; fi\n  shift\ndone\nprintf '{\"outcome\":\"ReadyForProposal\",\"summary\":\"checks passed\"}\\n' > \"$last\"\n",
    );
    let (app, _db) = test_router_with_execution(fake_wt, fake_codex).await;
    let project = create_trusted_local_markdown_project(&app, &project_path).await;
    sync_local_markdown_project(&app, &project).await;

    let response = start_source_issue_assignment(&app, &project, "ready").await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let assignment: IssueAssignmentResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(assignment.status, "blocked");
    assert_eq!(
        assignment.status_detail.as_deref(),
        Some("local markdown Issue Source has no Change Proposal target")
    );
    assert_eq!(
        assignment
            .latest_attempt
            .unwrap()
            .terminal_outcome
            .unwrap()
            .outcome,
        "ReadyForProposal"
    );
    assert!(
        std::fs::read_to_string(issues_dir.join("ready.md"))
            .unwrap()
            .contains("Lifecycle Status: blocked")
    );
}

#[tokio::test]
async fn codex_failed_outcome_persists_failed_assignment_and_blocks_source_issue() {
    let (project_path, issues_dir) = setup_local_markdown_project("assignment-failed");
    std::fs::create_dir_all(project_path.join(".git")).unwrap();
    std::fs::write(
        issues_dir.join("failed.md"),
        "# Failed\n\nReadiness: ready\nSource Order: 1\n",
    )
    .unwrap();
    let fake_wt = write_fake_command(
        "assignment-failed-fake-wt",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nworktree=\"/tmp/agentic-afk-failed-worktree-$$\"\nmkdir -p \"$worktree\"\nprintf '{\"path\":\"%s\"}\\n' \"$worktree\"\n",
    );
    let fake_codex = write_fake_command(
        "assignment-failed-fake-codex",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"--output-last-message\" ]; then shift; last=\"$1\"; fi\n  shift\ndone\nprintf '{\"outcome\":\"Failed\",\"summary\":\"checks could not run\"}\\n' > \"$last\"\n",
    );
    let (app, _db) = test_router_with_execution(fake_wt, fake_codex).await;
    let project = create_trusted_local_markdown_project(&app, &project_path).await;
    sync_local_markdown_project(&app, &project).await;

    let response = start_source_issue_assignment(&app, &project, "failed").await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let assignment: IssueAssignmentResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(assignment.status, "failed");
    assert_eq!(
        assignment.status_detail.as_deref(),
        Some("checks could not run")
    );
    let attempt = assignment.latest_attempt.unwrap();
    assert!(attempt.process_id.is_some());
    assert_eq!(attempt.terminal_outcome.unwrap().outcome, "Failed");
    assert!(
        std::fs::read_to_string(issues_dir.join("failed.md"))
            .unwrap()
            .contains("Lifecycle Status: blocked")
    );
}

#[tokio::test]
async fn worktrunk_setup_failure_releases_claim_without_marking_source_issue_active() {
    let (project_path, issues_dir) = setup_local_markdown_project("assignment-setup-failure");
    std::fs::create_dir_all(project_path.join(".git")).unwrap();
    std::fs::write(
        issues_dir.join("setup.md"),
        "# Setup\n\nReadiness: ready\nSource Order: 1\n",
    )
    .unwrap();
    let fake_wt = write_fake_command(
        "assignment-setup-fake-wt",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nprintf 'branch exists\\n' >&2\nexit 9\n",
    );
    let fake_codex = write_fake_command("assignment-setup-fake-codex", "#!/bin/sh\nexit 0\n");
    let (app, _db) = test_router_with_execution(fake_wt, fake_codex).await;
    let project = create_trusted_local_markdown_project(&app, &project_path).await;
    sync_local_markdown_project(&app, &project).await;

    let response = start_source_issue_assignment(&app, &project, "setup").await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let problem: ProblemDetail = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        problem.problem_type,
        "urn:agentic-afk:assignment-setup-failed"
    );
    assert!(
        !std::fs::read_to_string(issues_dir.join("setup.md"))
            .unwrap()
            .contains("Lifecycle Status:")
    );

    let state = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/projects/{}/assignment-state", project.id.0))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = state.into_body().collect().await.unwrap().to_bytes();
    let state: ProjectAssignmentStateResponse = serde_json::from_slice(&body).unwrap();
    assert!(state.active_assignment.is_none());
}

#[tokio::test]
async fn non_git_project_fails_assignment_preflight_before_claim() {
    let (project_path, issues_dir) = setup_local_markdown_project("assignment-preflight");
    std::fs::write(
        issues_dir.join("preflight.md"),
        "# Preflight\n\nReadiness: ready\nSource Order: 1\n",
    )
    .unwrap();
    let fake_wt = write_fake_command("assignment-preflight-fake-wt", "#!/bin/sh\nexit 0\n");
    let fake_codex = write_fake_command("assignment-preflight-fake-codex", "#!/bin/sh\nexit 0\n");
    let (app, _db) = test_router_with_execution(fake_wt, fake_codex).await;
    let project = create_trusted_local_markdown_project(&app, &project_path).await;
    sync_local_markdown_project(&app, &project).await;

    let response = start_source_issue_assignment(&app, &project, "preflight").await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let problem: ProblemDetail = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        problem.problem_type,
        "urn:agentic-afk:assignment-preflight-failed"
    );
    assert!(
        !std::fs::read_to_string(issues_dir.join("preflight.md"))
            .unwrap()
            .contains("Lifecycle Status:")
    );
}

#[tokio::test]
async fn codex_spawn_failure_preserves_failed_assignment_and_blocks_source_issue() {
    let (project_path, issues_dir) = setup_local_markdown_project("assignment-spawn-failure");
    std::fs::create_dir_all(project_path.join(".git")).unwrap();
    std::fs::write(
        issues_dir.join("spawn.md"),
        "# Spawn\n\nReadiness: ready\nSource Order: 1\n",
    )
    .unwrap();
    let worktree_path = temp_project_path("assignment-spawn-worktree");
    let fake_wt = write_fake_command(
        "assignment-spawn-fake-wt",
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nprintf '{{\"path\":\"{}\"}}\\n'\n",
            worktree_path.display()
        ),
    );
    let codex_preflight_only = write_fake_command(
        "assignment-spawn-fake-codex",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nprintf 'exec unavailable\\n' >&2\nexit 11\n",
    );
    let (app, _db) = test_router_with_execution(fake_wt, codex_preflight_only).await;
    let project = create_trusted_local_markdown_project(&app, &project_path).await;
    sync_local_markdown_project(&app, &project).await;

    let response = start_source_issue_assignment(&app, &project, "spawn").await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let assignment: IssueAssignmentResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(assignment.status, "failed");
    assert!(
        assignment
            .status_detail
            .as_deref()
            .unwrap()
            .contains("failed to spawn Codex")
    );
    assert_eq!(
        assignment
            .latest_attempt
            .unwrap()
            .terminal_outcome
            .unwrap()
            .outcome,
        "Failed"
    );
    assert!(
        std::fs::read_to_string(issues_dir.join("spawn.md"))
            .unwrap()
            .contains("Lifecycle Status: blocked")
    );
}

#[tokio::test]
async fn assignment_state_survives_control_plane_router_restart() {
    let (project_path, issues_dir) = setup_local_markdown_project("assignment-restart");
    std::fs::create_dir_all(project_path.join(".git")).unwrap();
    std::fs::write(
        issues_dir.join("restart.md"),
        "# Restart\n\nReadiness: ready\nSource Order: 1\n",
    )
    .unwrap();
    let fake_wt = write_fake_command(
        "assignment-restart-fake-wt",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nworktree=\"/tmp/agentic-afk-restart-worktree-$$\"\nmkdir -p \"$worktree\"\nprintf '{\"path\":\"%s\"}\\n' \"$worktree\"\n",
    );
    let fake_codex = write_fake_command(
        "assignment-restart-fake-codex",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"--output-last-message\" ]; then shift; last=\"$1\"; fi\n  shift\ndone\nprintf '{\"outcome\":\"Blocked\",\"summary\":\"persist me\"}\\n' > \"$last\"\n",
    );
    let (app, db) = test_router_with_execution(fake_wt.clone(), fake_codex.clone()).await;
    let project = create_trusted_local_markdown_project(&app, &project_path).await;
    sync_local_markdown_project(&app, &project).await;
    let response = start_source_issue_assignment(&app, &project, "restart").await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let restarted = router(
        ControlPlaneConfig {
            bind_address: "127.0.0.1:0".parse().unwrap(),
            dashboard_asset_dir: "apps/dashboard/dist".into(),
            database_url: "sqlite::memory:".into(),
            gh_binary_path: "gh".into(),
            worktrunk_binary_path: fake_wt,
            codex_binary_path: fake_codex,
        },
        db,
    );
    let state = restarted
        .oneshot(
            Request::builder()
                .uri(format!("/api/projects/{}/assignment-state", project.id.0))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(state.status(), StatusCode::OK);
    let body = state.into_body().collect().await.unwrap().to_bytes();
    let state: ProjectAssignmentStateResponse = serde_json::from_slice(&body).unwrap();
    let assignment = state.active_assignment.unwrap();
    assert_eq!(assignment.source_id, "restart");
    assert_eq!(
        assignment
            .latest_attempt
            .unwrap()
            .terminal_outcome
            .unwrap()
            .summary,
        "persist me"
    );
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
        dashboard_asset_dir: "apps/dashboard/dist".into(),
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
        dashboard_asset_dir: "apps/dashboard/dist".into(),
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
        dashboard_asset_dir: "apps/dashboard/dist".into(),
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
        dashboard_asset_dir: "apps/dashboard/dist".into(),
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
        dashboard_asset_dir: "apps/dashboard/dist".into(),
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
        dashboard_asset_dir: "apps/dashboard/dist".into(),
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
