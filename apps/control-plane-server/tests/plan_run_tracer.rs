//! Tracer test (issue #41): drive an empty Plan Run end-to-end through the
//! real router with the Integration Branch refresher and Codex planning
//! runner stubbed via the Plan Run dependency seam.

use agentic_afk_contracts::{
    CreateProjectRequest, PlanRunResponse, ProjectResponse, ProjectSnapshotResponse,
    SetProjectExecutionConfigRequest,
};
use agentic_afk_control_plane_server::{
    ControlPlaneConfig, FakePlanningPhaseRunner, RefreshedBaseline,
    StaticIntegrationBranchRefresher, router_with_plan_run_deps,
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
    std::env::temp_dir().join(format!(
        "agentic-afk-plan-run-{name}-{}",
        std::process::id()
    ))
}

async fn read_json<T: DeserializeOwned>(response: axum::response::Response) -> T {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn empty_plan_run_succeeds_and_appears_in_snapshot() {
    let project_dir = temp_project_path("tracer");
    std::fs::create_dir_all(&project_dir).unwrap();

    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "target/dx/agentic-afk-dashboard/release/web/public".into(),
        database_url: "sqlite::memory:".into(),
        gh_binary_path: "gh".into(),
        worktrunk_binary_path: "wt".into(),
        codex_binary_path: "codex".into(),
        docker_binary_path: "docker".into(),
        codex_auth_path: "/dev/null".into(),
    };
    let refresher = Arc::new(StaticIntegrationBranchRefresher::new(RefreshedBaseline {
        commit_sha: "abc1234".to_string(),
    }));
    let planner = Arc::new(FakePlanningPhaseRunner::with_stdout(
        r#"<plan>{"issues":[],"summary":"no eligible work"}</plan>"#,
    ));
    let router = router_with_plan_run_deps(config, db, refresher, planner);

    // Create + trust project, set execution config.
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
                            path: project_dir.to_string_lossy().into_owned(),
                        })
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let project_id = &project.id.0;

    let trust_response = router
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
    assert_eq!(trust_response.status(), StatusCode::OK);

    let config_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/projects/{project_id}/execution-config"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&SetProjectExecutionConfigRequest {
                        integration_branch: "main".into(),
                        max_parallel_tasks: 4,
                        review_retry_limit: 3,
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(config_response.status(), StatusCode::OK);

    // Trigger the Plan Run.
    let response = router
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
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let plan_run: PlanRunResponse = read_json(response).await;
    assert_eq!(plan_run.state, "succeeded_empty");
    assert_eq!(plan_run.integration_branch, "main");
    assert_eq!(plan_run.baseline_commit, "abc1234");
    assert_eq!(plan_run.finished_at.is_some(), true);
    assert_eq!(plan_run.phase_outputs.len(), 1);
    let planning = &plan_run.phase_outputs[0];
    assert_eq!(planning.phase, "planning");
    assert_eq!(planning.outcome, "succeeded_empty");
    assert_eq!(
        planning.body_json["summary"].as_str().unwrap(),
        "no eligible work"
    );
    assert_eq!(
        planning.body_json["selections"].as_array().unwrap().len(),
        0
    );
    assert_eq!(planning.body_json["phase"].as_str().unwrap(), "planning");

    // Snapshot reflects the finished Plan Run in history.
    let snapshot: ProjectSnapshotResponse = read_json(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/projects/{project_id}/snapshot"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(snapshot.snapshot.active_plan_run.is_none());
    assert_eq!(snapshot.snapshot.recent_plan_runs.len(), 1);
    assert_eq!(snapshot.snapshot.recent_plan_runs[0].id, plan_run.id);
    assert_eq!(
        snapshot.snapshot.recent_plan_runs[0].state,
        "succeeded_empty"
    );
    let cfg = snapshot.snapshot.execution_config.as_ref().unwrap();
    assert_eq!(cfg.integration_branch, "main");
    assert_eq!(cfg.max_parallel_tasks, 4);
    assert_eq!(cfg.review_retry_limit, 3);
}
