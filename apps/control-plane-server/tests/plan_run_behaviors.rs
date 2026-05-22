//! Phase C behavior cycles for issue #41: failure modes and history surface
//! for the empty Plan Run path.

use agentic_afk_contracts::{
    CreateProjectRequest, PlanRunResponse, ProjectResponse, SetProjectExecutionConfigRequest,
};
use agentic_afk_control_plane_server::{
    ControlPlaneConfig, FakePlanningPhaseRunner, IntegrationBranchRefresher, PlanRunPhaseError,
    PlanningPhaseRunner, RefreshedBaseline, StaticIntegrationBranchRefresher,
    router_with_plan_run_deps,
};
use agentic_afk_persistence::{self as persistence};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde::de::DeserializeOwned;
use std::path::PathBuf;
use std::sync::Arc;
use tower::ServiceExt;

fn temp_project_path(name: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "agentic-afk-plan-run-bhv-{name}-{}-{nonce}",
        std::process::id()
    ))
}

async fn read_json<T: DeserializeOwned>(response: axum::response::Response) -> T {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn read_text(response: axum::response::Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn test_config() -> ControlPlaneConfig {
    ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "target/dx/agentic-afk-dashboard/release/web/public".into(),
        database_url: "sqlite::memory:".into(),
        gh_binary_path: "gh".into(),
        worktrunk_binary_path: "wt".into(),
        codex_binary_path: "codex".into(),
    }
}

async fn fresh_router(
    refresher: Arc<dyn IntegrationBranchRefresher>,
    planner: Arc<dyn PlanningPhaseRunner>,
) -> axum::Router {
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    router_with_plan_run_deps(test_config(), db, refresher, planner)
}

async fn make_project(router: &axum::Router) -> ProjectResponse {
    let dir = temp_project_path("p");
    std::fs::create_dir_all(&dir).unwrap();
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/projects")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&CreateProjectRequest {
                        path: dir.to_string_lossy().into_owned(),
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    read_json(resp).await
}

async fn trust(router: &axum::Router, project_id: &str) {
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/projects/{project_id}/trust"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

async fn set_config(router: &axum::Router, project_id: &str) {
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/projects/{project_id}/execution-config"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&SetProjectExecutionConfigRequest {
                        integration_branch: "main".into(),
                        max_parallel_tasks: 2,
                        review_retry_limit: 1,
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

async fn post_plan_run(router: &axum::Router, project_id: &str) -> axum::response::Response {
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/projects/{project_id}/plan-runs"))
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap()
}

fn empty_plan_planner() -> Arc<dyn PlanningPhaseRunner> {
    Arc::new(FakePlanningPhaseRunner::with_stdout(
        r#"<plan>{"issues":[],"summary":"none"}</plan>"#,
    ))
}

fn static_refresher() -> Arc<dyn IntegrationBranchRefresher> {
    Arc::new(StaticIntegrationBranchRefresher::new(RefreshedBaseline {
        commit_sha: "deadbeef".into(),
    }))
}

#[tokio::test]
async fn start_plan_run_returns_403_for_untrusted_project() {
    let router = fresh_router(static_refresher(), empty_plan_planner()).await;
    let project = make_project(&router).await;
    // intentionally skip trust
    set_config(&router, &project.id.0).await;
    let resp = post_plan_run(&router, &project.id.0).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = read_text(resp).await;
    assert!(
        body.contains("urn:agentic-afk:project-untrusted"),
        "unexpected body: {body}"
    );
}

#[tokio::test]
async fn start_plan_run_auto_creates_execution_config_with_platform_defaults() {
    // PRD #34/#21: when no Execution Config exists for a trusted Project, the
    // first Plan Run auto-creates one using platform defaults so the developer
    // does not have to configure execution before kicking off unattended work.
    let router = fresh_router(static_refresher(), empty_plan_planner()).await;
    let project = make_project(&router).await;
    trust(&router, &project.id.0).await;
    let resp = post_plan_run(&router, &project.id.0).await;
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Inspect the auto-created config through the snapshot route the
    // Dashboard hydrates from.
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/projects/{}/snapshot", project.id.0))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = read_json(resp).await;
    let config = body
        .pointer("/snapshot/execution_config")
        .expect("snapshot exposes execution_config");
    // Platform defaults: max_parallel_tasks=3, review_retry_limit=3, and
    // integration_branch falls back to "main" when origin/HEAD is not
    // detectable in the bare temp Project directory.
    assert_eq!(config["max_parallel_tasks"].as_i64(), Some(3));
    assert_eq!(config["review_retry_limit"].as_i64(), Some(3));
    assert_eq!(config["integration_branch"].as_str(), Some("main"));
}

#[tokio::test]
async fn second_plan_run_returns_409_while_first_is_active() {
    // PlanningPhaseRunner is sync, so to exercise the "active" path we seed
    // a running Plan Run directly via persistence and check the route refuses
    // a second start — same observable contract as a real concurrent start.
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let router =
        router_with_plan_run_deps(test_config(), db.clone(), static_refresher(), empty_plan_planner());
    let project = make_project(&router).await;
    trust(&router, &project.id.0).await;
    set_config(&router, &project.id.0).await;

    // Seed an already-running Plan Run directly.
    persistence::create_plan_run(&db, &project.id.0, "main", "abc")
        .await
        .unwrap();

    let resp = post_plan_run(&router, &project.id.0).await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = read_text(resp).await;
    assert!(
        body.contains("urn:agentic-afk:active-plan-run"),
        "unexpected body: {body}"
    );
}

#[tokio::test]
async fn list_plan_runs_returns_history_newest_first() {
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let router =
        router_with_plan_run_deps(test_config(), db.clone(), static_refresher(), empty_plan_planner());
    let project = make_project(&router).await;
    trust(&router, &project.id.0).await;
    set_config(&router, &project.id.0).await;

    let first = post_plan_run(&router, &project.id.0).await;
    assert_eq!(first.status(), StatusCode::CREATED);
    let first: PlanRunResponse = read_json(first).await;

    // ensure the second start_at is strictly later than the first.
    std::thread::sleep(std::time::Duration::from_millis(1100));

    let second = post_plan_run(&router, &project.id.0).await;
    assert_eq!(second.status(), StatusCode::CREATED);
    let second: PlanRunResponse = read_json(second).await;

    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/projects/{}/plan-runs", project.id.0))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let history: Vec<PlanRunResponse> = read_json(resp).await;
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].id, second.id);
    assert_eq!(history[1].id, first.id);
}

#[tokio::test]
async fn planner_failure_records_failed_phase_output_and_failed_state() {
    struct FailingPlanner;
    impl PlanningPhaseRunner for FailingPlanner {
        fn run(&self, _prompt: &str) -> Result<String, PlanRunPhaseError> {
            Err(PlanRunPhaseError::Planning("codex blew up".to_string()))
        }
    }
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let router = router_with_plan_run_deps(
        test_config(),
        db.clone(),
        static_refresher(),
        Arc::new(FailingPlanner),
    );
    let project = make_project(&router).await;
    trust(&router, &project.id.0).await;
    set_config(&router, &project.id.0).await;

    let resp = post_plan_run(&router, &project.id.0).await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let runs = persistence::list_recent_plan_runs(&db, &project.id.0, 10)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].state, "failed");
    assert_eq!(runs[0].phase_outputs.len(), 1);
    assert_eq!(runs[0].phase_outputs[0].phase, "planning");
    assert_eq!(runs[0].phase_outputs[0].outcome, "failed");
}

#[tokio::test]
async fn unparseable_planner_output_records_failed_phase_output() {
    let router = fresh_router(
        static_refresher(),
        Arc::new(FakePlanningPhaseRunner::with_stdout(
            "no plan tags here at all",
        )),
    )
    .await;
    let project = make_project(&router).await;
    trust(&router, &project.id.0).await;
    set_config(&router, &project.id.0).await;
    let resp = post_plan_run(&router, &project.id.0).await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn execution_config_rejects_non_positive_concurrency() {
    let router = fresh_router(static_refresher(), empty_plan_planner()).await;
    let project = make_project(&router).await;
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/api/projects/{}/execution-config",
                    project.id.0
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&SetProjectExecutionConfigRequest {
                        integration_branch: "main".into(),
                        max_parallel_tasks: 0,
                        review_retry_limit: 1,
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn planning_prompt_substitutes_integration_branch_and_baseline() {
    let planner = Arc::new(FakePlanningPhaseRunner::with_stdout(
        r#"<plan>{"issues":[],"summary":"none"}</plan>"#,
    ));
    let planner_for_router: Arc<dyn PlanningPhaseRunner> = planner.clone();
    let router = fresh_router(
        Arc::new(StaticIntegrationBranchRefresher::new(RefreshedBaseline {
            commit_sha: "f00ba2".into(),
        })),
        planner_for_router,
    )
    .await;
    let project = make_project(&router).await;
    trust(&router, &project.id.0).await;
    let _ = router
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/api/projects/{}/execution-config",
                    project.id.0
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&SetProjectExecutionConfigRequest {
                        integration_branch: "trunk".into(),
                        max_parallel_tasks: 7,
                        review_retry_limit: 2,
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let resp = post_plan_run(&router, &project.id.0).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let prompt = planner.last_prompt().expect("planner was called");
    assert!(prompt.contains("Integration Branch: trunk"), "{prompt}");
    assert!(prompt.contains("Plan Run Baseline: f00ba2"), "{prompt}");
    assert!(prompt.contains("Max Parallel Tasks: 7"), "{prompt}");
}


#[tokio::test]
async fn planning_prompt_carries_project_instructions_and_source_brief() {
    // PRD User Story #22: planning, implementation, review, and merge prompts
    // include Project Instructions. The planning render path historically
    // hardcoded an empty `{{PROJECT_INSTRUCTIONS}}`; this test pins the
    // contract that the planning prompt receives the same AGENTS.md /
    // CLAUDE.md content the other phases do.
    let planner = Arc::new(FakePlanningPhaseRunner::with_stdout(
        r#"<plan>{"issues":[],"summary":"none"}</plan>"#,
    ));
    let planner_for_router: Arc<dyn PlanningPhaseRunner> = planner.clone();
    let router = fresh_router(static_refresher(), planner_for_router).await;

    // Manually create a Project whose path holds an AGENTS.md so
    // `load_project_instructions` returns project-specific text.
    let dir = temp_project_path("planning-instructions");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("AGENTS.md"),
        "# Project Instructions\nPlanning must respect repo conventions.",
    )
    .unwrap();

    let project: ProjectResponse = read_json(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/projects")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_string(&CreateProjectRequest {
                            path: dir.to_string_lossy().into_owned(),
                        })
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    trust(&router, &project.id.0).await;
    set_config(&router, &project.id.0).await;

    let resp = post_plan_run(&router, &project.id.0).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let prompt = planner.last_prompt().expect("planner was called");
    assert!(
        prompt.contains("Planning must respect repo conventions."),
        "planning prompt must include Project Instructions: {prompt}"
    );
    // Sanity: planner still gets the standard placeholders.
    assert!(prompt.contains("Integration Branch: main"), "{prompt}");
    assert!(prompt.contains("Plan Run Baseline: deadbeef"), "{prompt}");
}
