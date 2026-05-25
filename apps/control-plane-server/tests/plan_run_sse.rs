//! Phase C: live SSE deltas published by an empty Plan Run.

use agentic_afk_contracts::{
    CreateProjectRequest, ProjectEvent, ProjectId, ProjectResponse,
    SetProjectExecutionConfigRequest,
};
use agentic_afk_control_plane_server::{
    ControlPlaneConfig, FakePlanningPhaseRunner, StaticIntegrationBranchRefresher,
    event_bus::EventBus, router_with_plan_run_deps_and_bus,
};
use agentic_afk_orchestrator::RefreshedBaseline;
use agentic_afk_persistence::{self as persistence};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures_util::StreamExt;
use http_body_util::BodyExt;
use serde::de::DeserializeOwned;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tower::ServiceExt;

fn temp_project_path(name: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "agentic-afk-plan-run-sse-{name}-{}-{nonce}",
        std::process::id()
    ))
}

async fn read_json<T: DeserializeOwned>(response: axum::response::Response) -> T {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn empty_plan_run_publishes_started_phase_completed_completed_in_order() {
    let dir = temp_project_path("seq");
    std::fs::create_dir_all(&dir).unwrap();

    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let bus = EventBus::new();
    let refresher = Arc::new(StaticIntegrationBranchRefresher::new(RefreshedBaseline {
        commit_sha: "cafebabe".into(),
    }));
    let planner = Arc::new(FakePlanningPhaseRunner::with_stdout(
        r#"<plan>{"issues":[],"summary":"none"}</plan>"#,
    ));
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
    let router = router_with_plan_run_deps_and_bus(config, db, bus.clone(), refresher, planner);

    // Bootstrap project + trust + config.
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
    let project_id = project.id.0.clone();

    let _ = router
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
    let _ = router
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/projects/{project_id}/execution-config"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&SetProjectExecutionConfigRequest {
                        integration_branch: "main".into(),
                        max_parallel_tasks: 1,
                        review_retry_limit: 1,
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    // Subscribe BEFORE triggering — capture only Plan Run events.
    let mut stream = Box::pin(bus.subscribe(&ProjectId(project_id.clone()), None));

    let resp = router
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
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Collect the three Plan Run lifecycle events.
    let mut started = None;
    let mut phase_completed = None;
    let mut completed = None;
    for _ in 0..3 {
        let next = timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("stream did not yield a Plan Run event in time")
            .expect("stream closed prematurely");
        match next.event {
            ProjectEvent::PlanRunStarted(plan_run) => started = Some(plan_run),
            ProjectEvent::PlanRunPhaseCompleted {
                plan_run_id,
                phase_output,
            } => {
                phase_completed = Some((plan_run_id, phase_output));
            }
            ProjectEvent::PlanRunCompleted(plan_run) => completed = Some(plan_run),
            other => panic!("unexpected event: {other:?}"),
        }
    }
    let started = started.expect("PlanRunStarted missing");
    let (phase_plan_run_id, phase_output) = phase_completed.expect("PlanRunPhaseCompleted missing");
    let completed = completed.expect("PlanRunCompleted missing");

    assert_eq!(started.state, agentic_afk_contracts::PlanRunState::Running);
    assert_eq!(phase_plan_run_id, started.id);
    assert_eq!(phase_output.phase, "planning");
    assert_eq!(phase_output.outcome, "succeeded_empty");
    assert_eq!(completed.id, started.id);
    assert_eq!(
        completed.state,
        agentic_afk_contracts::PlanRunState::Finished
    );
    assert!(completed.finished_at.is_some());
}
