use agentic_afk_contracts::{
    AutoReplanState, CreateProjectRequest, PauseReason, ProjectEvent, ProjectId, ProjectResponse,
};
use agentic_afk_control_plane_server::{ControlPlaneConfig, event_bus::EventBus, router_with_bus};
use agentic_afk_persistence as persistence;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures_util::StreamExt;
use http_body_util::BodyExt;
use serde::de::DeserializeOwned;
use std::path::PathBuf;
use tower::ServiceExt;

fn temp_path(name: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "agentic-afk-auto-replan-{name}-{}-{nonce}",
        std::process::id()
    ))
}

fn config(database_url: &str) -> ControlPlaneConfig {
    ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "target/dx/agentic-afk-dashboard/release/web/public".into(),
        database_url: database_url.into(),
        gh_binary_path: "gh".into(),
        worktrunk_binary_path: "wt".into(),
        codex_binary_path: "codex".into(),
    }
}

async fn test_router() -> (axum::Router, persistence::Db, EventBus) {
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let bus = EventBus::new();
    let app = router_with_bus(config("sqlite::memory:"), db.clone(), bus.clone());
    (app, db, bus)
}

async fn test_router_with_test_endpoints() -> (axum::Router, persistence::Db, EventBus) {
    // Test-only routes are selected at router construction time.
    unsafe {
        std::env::set_var("AGENTIC_AFK_TEST_ENDPOINTS", "1");
    }
    test_router().await
}

async fn read_json<T: DeserializeOwned>(response: axum::response::Response) -> T {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn read_text(response: axum::response::Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).into_owned()
}

async fn create_project(app: &axum::Router) -> ProjectResponse {
    let dir = temp_path("project");
    std::fs::create_dir_all(&dir).unwrap();
    let response = app
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
    assert_eq!(response.status(), StatusCode::CREATED);
    read_json(response).await
}

async fn trust_project(app: &axum::Router, project_id: &str) {
    let response = app
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
    assert_eq!(response.status(), StatusCode::OK);
}

async fn post(app: &axum::Router, uri: String) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn post_json(
    app: &axum::Router,
    uri: String,
    body: serde_json::Value,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn arm_requires_off_and_trusted_project() {
    let (app, db, _bus) = test_router().await;
    let project = create_project(&app).await;

    let untrusted = post(
        &app,
        format!("/api/projects/{}/auto-replan/arm", project.id.0),
    )
    .await;
    assert_eq!(untrusted.status(), StatusCode::FORBIDDEN);
    assert!(read_text(untrusted).await.contains("project-untrusted"));

    trust_project(&app, &project.id.0).await;
    let armed = post(
        &app,
        format!("/api/projects/{}/auto-replan/arm", project.id.0),
    )
    .await;
    assert_eq!(armed.status(), StatusCode::OK);
    let armed: ProjectResponse = read_json(armed).await;
    assert_eq!(armed.auto_replan_state, AutoReplanState::Armed);

    let conflict = post(
        &app,
        format!("/api/projects/{}/auto-replan/arm", project.id.0),
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    persistence::AutoReplanStateStore::new(&db)
        .set(
            &project.id.0,
            AutoReplanState::Paused,
            Some(PauseReason::EmptyBacklog),
        )
        .await
        .unwrap();
    let paused_conflict = post(
        &app,
        format!("/api/projects/{}/auto-replan/arm", project.id.0),
    )
    .await;
    assert_eq!(paused_conflict.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn disarm_is_idempotent_from_every_state() {
    let (app, db, _bus) = test_router().await;
    let project = create_project(&app).await;
    trust_project(&app, &project.id.0).await;
    let store = persistence::AutoReplanStateStore::new(&db);

    for (state, reason) in [
        (AutoReplanState::Off, None),
        (AutoReplanState::Armed, None),
        (
            AutoReplanState::Paused,
            Some(PauseReason::AssignmentBlocked),
        ),
    ] {
        store.set(&project.id.0, state, reason).await.unwrap();
        let response = post(
            &app,
            format!("/api/projects/{}/auto-replan/disarm", project.id.0),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let project: ProjectResponse = read_json(response).await;
        assert_eq!(project.auto_replan_state, AutoReplanState::Off);
        assert_eq!(project.auto_replan_pause_reason, None);
    }
}

#[tokio::test]
async fn resume_requires_paused_state() {
    let (app, db, _bus) = test_router().await;
    let project = create_project(&app).await;
    trust_project(&app, &project.id.0).await;

    for state in [AutoReplanState::Off, AutoReplanState::Armed] {
        persistence::AutoReplanStateStore::new(&db)
            .set(&project.id.0, state, None)
            .await
            .unwrap();
        let response = post(
            &app,
            format!("/api/projects/{}/auto-replan/resume", project.id.0),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    persistence::AutoReplanStateStore::new(&db)
        .set(
            &project.id.0,
            AutoReplanState::Paused,
            Some(PauseReason::PlanningFailed),
        )
        .await
        .unwrap();
    let response = post(
        &app,
        format!("/api/projects/{}/auto-replan/resume", project.id.0),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let project: ProjectResponse = read_json(response).await;
    assert_eq!(project.auto_replan_state, AutoReplanState::Armed);
    assert_eq!(project.auto_replan_pause_reason, None);
}

#[tokio::test]
async fn successful_transition_records_activity_and_emits_sse_delta() {
    let (app, _db, bus) = test_router().await;
    let project = create_project(&app).await;
    trust_project(&app, &project.id.0).await;
    let last_seen = bus.latest_sequence(&ProjectId(project.id.0.clone()));
    let mut stream = Box::pin(bus.subscribe(&ProjectId(project.id.0.clone()), Some(last_seen)));

    let response = post(
        &app,
        format!("/api/projects/{}/auto-replan/arm", project.id.0),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let activity = stream.next().await.unwrap();
    match activity.event {
        ProjectEvent::Activity(entry) => assert_eq!(entry.kind, "AutoReplanArmed"),
        other => panic!("expected Activity, got {other:?}"),
    }
    let state = stream.next().await.unwrap();
    match state.event {
        ProjectEvent::AutoReplanStateChanged { state, reason } => {
            assert_eq!(state, AutoReplanState::Armed);
            assert_eq!(reason, None);
        }
        other => panic!("expected AutoReplanStateChanged, got {other:?}"),
    }
}

#[tokio::test]
async fn test_pause_seam_records_paused_activity_and_state_delta() {
    let (app, db, bus) = test_router_with_test_endpoints().await;
    let project = create_project(&app).await;
    let mut stream = Box::pin(bus.subscribe(&ProjectId(project.id.0.clone()), Some(0)));

    let response = post_json(
        &app,
        format!("/api/_test/projects/{}/auto-replan/pause", project.id.0),
        serde_json::json!({ "reason": "push_non_fast_forward" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let activity = stream.next().await.unwrap();
    match activity.event {
        ProjectEvent::Activity(entry) => {
            assert_eq!(entry.kind, "AutoReplanPaused");
            assert_eq!(entry.detail.as_deref(), Some("push_non_fast_forward"));
        }
        other => panic!("expected Activity, got {other:?}"),
    }
    let state = stream.next().await.unwrap();
    match state.event {
        ProjectEvent::AutoReplanStateChanged { state, reason } => {
            assert_eq!(state, AutoReplanState::Paused);
            assert_eq!(reason, Some(PauseReason::PushNonFastForward));
        }
        other => panic!("expected AutoReplanStateChanged, got {other:?}"),
    }

    let activities = persistence::list_project_activity(&db, &project.id.0, 10)
        .await
        .unwrap();
    assert!(activities.iter().any(|entry| {
        entry.kind == "AutoReplanPaused" && entry.detail.as_deref() == Some("push_non_fast_forward")
    }));
}

#[tokio::test]
async fn paused_state_survives_database_reconnect_with_reason() {
    let db_path = temp_path("restart.sqlite");
    let database_url = format!("sqlite://{}", db_path.to_string_lossy());
    let db = persistence::connect(&database_url).await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let dir = temp_path("restart-project");
    std::fs::create_dir_all(&dir).unwrap();
    let project = persistence::create_project(
        &db,
        &CreateProjectRequest {
            path: dir.to_string_lossy().into_owned(),
        },
    )
    .await
    .unwrap();
    persistence::AutoReplanStateStore::new(&db)
        .set(
            &project.id.0,
            AutoReplanState::Paused,
            Some(PauseReason::SyncFailed),
        )
        .await
        .unwrap();
    drop(db);

    let reopened = persistence::connect(&database_url).await.unwrap();
    persistence::migrate(&reopened).await.unwrap();
    let project = persistence::get_project(&reopened, &project.id.0)
        .await
        .unwrap();
    assert_eq!(project.auto_replan_state, AutoReplanState::Paused);
    assert_eq!(
        project.auto_replan_pause_reason,
        Some(PauseReason::SyncFailed)
    );
}
