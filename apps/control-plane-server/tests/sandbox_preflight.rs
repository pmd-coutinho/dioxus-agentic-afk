//! Integration test (issue #73): a Plan Run trigger fails with a 422
//! problem-JSON response carrying the stable URN for each of the four
//! sandbox preflight failure variants, and no Plan Run row or Source
//! Issue Lifecycle write happens.

use std::sync::Arc;

use agentic_afk_contracts::{CreateProjectRequest, ProjectResponse};
use agentic_afk_control_plane_server::{
    ControlPlaneConfig, FakePlanningPhaseRunner, PlanRunDeps, RefreshedBaseline,
    StaticIntegrationBranchRefresher, event_bus::EventBus, router_with_full_deps_and_preflight,
};
use agentic_afk_orchestrator::{
    RejectingSandboxPreflight, SandboxFailureTemplate, SandboxPreflightCheck,
};
use agentic_afk_persistence as persistence;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde::Deserialize;
use serde_json::Value;
use tower::ServiceExt;

#[derive(Debug, Deserialize)]
struct ProblemDetailJson {
    #[serde(rename = "type")]
    problem_type: String,
    status: u16,
    detail: Option<String>,
}

fn config(database_url: &str) -> ControlPlaneConfig {
    ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "target/dx/agentic-afk-dashboard/release/web/public".into(),
        database_url: database_url.into(),
        gh_binary_path: "gh".into(),
        worktrunk_binary_path: "wt".into(),
        codex_binary_path: "codex".into(),
        docker_binary_path: "docker".into(),
        codex_auth_path: "/dev/null".into(),
    }
}

async fn build_router(
    preflight: Arc<dyn SandboxPreflightCheck>,
) -> (axum::Router, persistence::Db) {
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();

    let refresher = Arc::new(StaticIntegrationBranchRefresher::new(RefreshedBaseline {
        commit_sha: "baseline".to_string(),
    }));
    let planner = Arc::new(FakePlanningPhaseRunner::with_stdout(
        r#"<plan>{"issues":[],"summary":"unused"}</plan>"#,
    ));
    let deps = PlanRunDeps {
        refresher,
        planner,
        ..PlanRunDeps::default_test_deps()
    };
    let router = router_with_full_deps_and_preflight(
        config("sqlite::memory:"),
        db.clone(),
        EventBus::new(),
        deps,
        preflight,
    );
    (router, db)
}

async fn create_and_trust_project(
    router: &axum::Router,
    project_dir: &std::path::Path,
) -> ProjectResponse {
    let create_resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/projects")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&CreateProjectRequest {
                        path: project_dir.to_string_lossy().into_owned(),
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let project_bytes = create_resp.into_body().collect().await.unwrap().to_bytes();
    let project: ProjectResponse = serde_json::from_slice(&project_bytes).unwrap();

    let trust_resp = router
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
    assert_eq!(trust_resp.status(), StatusCode::OK);
    project
}

async fn run_case(template: SandboxFailureTemplate, expected_urn: &str) {
    let project_dir = std::env::temp_dir().join(format!(
        "agentic-afk-preflight-it-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&project_dir).unwrap();

    let preflight = Arc::new(RejectingSandboxPreflight::new(template));
    let (router, db) = build_router(preflight).await;
    let project = create_and_trust_project(&router, &project_dir).await;

    let trigger_resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/projects/{}/plan-runs", project.id.0))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        trigger_resp.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "expected 422 for preflight failure"
    );
    let body = trigger_resp.into_body().collect().await.unwrap().to_bytes();
    let problem: ProblemDetailJson = serde_json::from_slice(&body).expect("problem json parses");
    assert_eq!(problem.status, 422);
    assert_eq!(problem.problem_type, expected_urn);
    assert!(problem.detail.is_some());

    // Confirm no Plan Run row was created — Source Issue Lifecycle state
    // is untouched because Source Issues were never registered, but we
    // can at least verify the plan-runs collection is empty.
    let plan_runs_resp = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/projects/{}/plan-runs", project.id.0))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(plan_runs_resp.status(), StatusCode::OK);
    let runs_body = plan_runs_resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    let runs_json: Value = serde_json::from_slice(&runs_body).unwrap();
    assert_eq!(
        runs_json.as_array().map(|a| a.len()).unwrap_or(0),
        0,
        "preflight failure must not create a Plan Run row"
    );

    let _ = db; // keep alive
    let _ = std::fs::remove_dir_all(&project_dir);
}

#[tokio::test]
async fn docker_unavailable_returns_422_with_urn() {
    run_case(
        SandboxFailureTemplate::DockerUnavailable("connection refused".into()),
        "urn:agentic-afk:sandbox-docker-unavailable",
    )
    .await;
}

#[tokio::test]
async fn codex_auth_missing_returns_422_with_urn() {
    run_case(
        SandboxFailureTemplate::CodexAuthMissing("/home/dev/.codex/auth.json".into()),
        "urn:agentic-afk:sandbox-codex-auth-missing",
    )
    .await;
}

#[tokio::test]
async fn mise_toml_missing_returns_422_with_urn() {
    run_case(
        SandboxFailureTemplate::MiseTomlMissing("/path/to/project".into()),
        "urn:agentic-afk:sandbox-mise-toml-missing",
    )
    .await;
}

#[tokio::test]
async fn runtime_image_build_failed_returns_422_with_urn_and_stderr_detail() {
    run_case(
        SandboxFailureTemplate::RuntimeImageBuildFailed(
            "step 5/7: COPY entrypoint.sh — file not found".into(),
        ),
        "urn:agentic-afk:sandbox-runtime-image-build-failed",
    )
    .await;
}
