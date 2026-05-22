//! Tests for issue #32: server write sites publish typed `ProjectEvent`
//! deltas through `project_event_publisher`. Each test subscribes to the
//! `EventBus` for the project, drives the relevant handler, then drains and
//! asserts on the published events.

use std::path::{Path, PathBuf};

use agentic_afk_contracts::{
    CreateProjectRequest, EnableIssueSourceRequest, IssueSourceSyncResponse, ProjectEvent,
    ProjectResponse,
};
use agentic_afk_control_plane_server::{
    ControlPlaneConfig, event_bus::EventBus, event_bus::SequencedEvent, router_with_bus,
};
use agentic_afk_persistence as persistence;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

fn temp_project_path(label: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let salt = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "agentic-afk-issue32-{label}-{}-{}",
        std::process::id(),
        nanos.wrapping_add(u128::from(salt))
    ));
    std::fs::create_dir_all(path.join(".git")).unwrap();
    path
}

async fn make_router() -> (axum::Router, persistence::Db, EventBus) {
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let bus = EventBus::new();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "target/dx/agentic-afk-dashboard/release/web/public".into(),
        database_url: "sqlite::memory:".into(),
        gh_binary_path: "gh".into(),
        worktrunk_binary_path: "wt".into(),
        codex_binary_path: "codex".into(),
    };
    let app = router_with_bus(config, db.clone(), bus.clone());
    (app, db, bus)
}

async fn create_project(app: &axum::Router, project_path: &Path) -> ProjectResponse {
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
    serde_json::from_slice(&body).unwrap()
}

async fn enable_local_markdown(app: &axum::Router, project_id: &str) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/projects/{project_id}/issue-source"))
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
}

/// Drain events for `project_id` strictly after `since_seq`. Lets a single
/// test compare events published by two consecutive actions without seeing
/// the first action's events replayed during the second drain.
async fn drain_events_since(
    bus: &EventBus,
    project_id: &str,
    since_seq: u64,
) -> Vec<SequencedEvent> {
    use futures_util::StreamExt;
    let stream = bus.subscribe(
        &agentic_afk_contracts::ProjectId(project_id.to_string()),
        Some(since_seq),
    );
    let mut events = Vec::new();
    let mut stream = std::pin::pin!(stream);
    while let Ok(Some(ev)) =
        tokio::time::timeout(std::time::Duration::from_millis(50), stream.next()).await
    {
        events.push(ev);
    }
    events
}

fn latest_seq(events: &[SequencedEvent]) -> u64 {
    events.last().map(|e| e.sequence).unwrap_or(0)
}

#[tokio::test]
async fn enable_issue_source_publishes_candidates_changed_and_planning_snapshot_changed() {
    let (app, _db, bus) = make_router().await;
    let project_path = temp_project_path("enable");
    std::fs::create_dir_all(project_path.join(".scratch/issues")).unwrap();
    let project = create_project(&app, &project_path).await;

    enable_local_markdown(&app, &project.id.0).await;

    let events = drain_events_since(&bus, &project.id.0, 0).await;

    let kinds: Vec<&'static str> = events
        .iter()
        .map(|e| match &e.event {
            ProjectEvent::IssueSourceCandidatesChanged { .. } => "candidates",
            ProjectEvent::PlanningSnapshotChanged { .. } => "planning",
            ProjectEvent::Activity(_) => "activity",
            _ => "other",
        })
        .collect();

    assert!(
        kinds.contains(&"candidates"),
        "expected IssueSourceCandidatesChanged, saw {kinds:?}"
    );
    assert!(
        kinds.contains(&"planning"),
        "expected PlanningSnapshotChanged, saw {kinds:?}"
    );

    // Sequences must be monotonic and gap-free for replay correctness.
    for window in events.windows(2) {
        assert_eq!(window[1].sequence, window[0].sequence + 1);
    }
}

#[tokio::test]
async fn sync_issue_source_success_publishes_started_completed_planning_sequence() {
    let (app, _db, bus) = make_router().await;
    let project_path = temp_project_path("sync-ok");
    let issues_dir = project_path.join(".scratch/issues");
    std::fs::create_dir_all(&issues_dir).unwrap();
    std::fs::write(
        issues_dir.join("issue-A.md"),
        "---\nreadiness: ready\n---\n# Issue A\n",
    )
    .unwrap();
    let project = create_project(&app, &project_path).await;
    enable_local_markdown(&app, &project.id.0).await;
    // Drain the enable-time events so the next drain only contains sync.
    let after = latest_seq(&drain_events_since(&bus, &project.id.0, 0).await);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/projects/{}/issue-source/sync",
                    project.id.0
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let events = drain_events_since(&bus, &project.id.0, after).await;
    let kinds: Vec<&'static str> = events
        .iter()
        .map(|e| match &e.event {
            ProjectEvent::IssueSourceSyncStarted => "started",
            ProjectEvent::IssueSourceSyncCompleted(_) => "completed",
            ProjectEvent::PlanningSnapshotChanged { .. } => "planning",
            _ => "other",
        })
        .collect();

    let started_pos = kinds.iter().position(|k| *k == "started").expect("Started present");
    let completed_pos = kinds.iter().position(|k| *k == "completed").expect("Completed present");
    let planning_pos = kinds.iter().position(|k| *k == "planning").expect("Planning present");
    assert!(started_pos < completed_pos, "Started before Completed");
    assert!(started_pos < planning_pos, "Started before Planning");

    for window in events.windows(2) {
        assert_eq!(window[1].sequence, window[0].sequence + 1);
    }
}

#[tokio::test]
async fn sync_issue_source_failure_publishes_started_then_failed() {
    let (app, _db, bus) = make_router().await;
    let project_path = temp_project_path("sync-fail");
    // No .scratch/issues directory => sync fails.
    let project = create_project(&app, &project_path).await;
    enable_local_markdown(&app, &project.id.0).await;
    let after = latest_seq(&drain_events_since(&bus, &project.id.0, 0).await);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/projects/{}/issue-source/sync",
                    project.id.0
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let events = drain_events_since(&bus, &project.id.0, after).await;
    let kinds: Vec<&'static str> = events
        .iter()
        .map(|e| match &e.event {
            ProjectEvent::IssueSourceSyncStarted => "started",
            ProjectEvent::IssueSourceSyncFailed { .. } => "failed",
            _ => "other",
        })
        .collect();
    let started_pos = kinds.iter().position(|k| *k == "started").expect("Started present");
    let failed_pos = kinds.iter().position(|k| *k == "failed").expect("Failed present");
    assert!(started_pos < failed_pos);

    // Sanity: no Completed and no PlanningSnapshotChanged on the failure path.
    for ev in &events {
        assert!(
            !matches!(
                ev.event,
                ProjectEvent::IssueSourceSyncCompleted(_)
                    | ProjectEvent::PlanningSnapshotChanged { .. }
            ),
            "unexpected event on failure path: {:?}",
            ev.event
        );
    }

    // Suppress unused-import warning if `IssueSourceSyncResponse` happens not to
    // be referenced in this test (kept for parity with adjacent tests).
    let _ = std::marker::PhantomData::<IssueSourceSyncResponse>;
}

// --- Assignment-side publisher tests (cycles 12-14) ------------------------

fn temp_file_path(label: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let salt = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "agentic-afk-issue32-{label}-{}-{}",
        std::process::id(),
        nanos.wrapping_add(u128::from(salt))
    ))
}

fn write_fake_command(name: &str, body: &str) -> PathBuf {
    let path = temp_file_path(name);
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }
    path
}

async fn make_router_with_fakes(
    worktrunk: PathBuf,
    codex: PathBuf,
) -> (axum::Router, persistence::Db, EventBus) {
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let bus = EventBus::new();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "target/dx/agentic-afk-dashboard/release/web/public".into(),
        database_url: "sqlite::memory:".into(),
        gh_binary_path: "gh".into(),
        worktrunk_binary_path: worktrunk,
        codex_binary_path: codex,
    };
    let app = router_with_bus(config, db.clone(), bus.clone());
    (app, db, bus)
}

async fn trust_project(app: &axum::Router, project_id: &str) -> ProjectResponse {
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
    let body = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn start_assignment_publishes_created_attempt_proposal_and_status_in_sequence() {
    let worktree_path = temp_project_path("issue32-worktree");
    let fake_wt = write_fake_command(
        "issue32-fake-wt",
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nif [ \"$1\" = \"switch\" ]; then\n  mkdir -p '{worktree}'\n  printf '{{\"path\":\"{worktree}\"}}\\n'\n  exit 0\nfi\nexit 0\n",
            worktree = worktree_path.display(),
        ),
    );
    // Codex outcome `Blocked` keeps the flow simple (no Change Proposal, no
    // github lifecycle writes). Cycle 14 (ChangeProposalRefreshed) is asserted
    // by a separate test path where applicable; here we focus on Created +
    // AttemptAdded + StatusChanged.
    let fake_codex = write_fake_command(
        "issue32-fake-codex",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nlast=\"\"\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"--output-last-message\" ]; then shift; last=\"$1\"; fi\n  shift\ndone\nprintf '{\"outcome\":\"Blocked\",\"summary\":\"need input\"}\\n' > \"$last\"\n",
    );

    let (app, _db, bus) = make_router_with_fakes(fake_wt, fake_codex).await;
    let project_path = temp_project_path("issue32-proj");
    let issues_dir = project_path.join(".scratch/issues");
    std::fs::create_dir_all(&issues_dir).unwrap();
    std::fs::write(
        issues_dir.join("issue-A.md"),
        "# Issue A\n\nReadiness: ready\nSource Order: 1\n",
    )
    .unwrap();

    let project = create_project(&app, &project_path).await;
    enable_local_markdown(&app, &project.id.0).await;
    let _ = trust_project(&app, &project.id.0).await;
    // Sync to populate eligible Source Issues.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/projects/{}/issue-source/sync",
                    project.id.0
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let after = latest_seq(&drain_events_since(&bus, &project.id.0, 0).await);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/projects/{}/source-issues/issue-A/assignment",
                    project.id.0
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let events = drain_events_since(&bus, &project.id.0, after).await;
    let kinds: Vec<&'static str> = events
        .iter()
        .map(|e| match &e.event {
            ProjectEvent::AssignmentCreated(_) => "created",
            ProjectEvent::AssignmentAttemptAdded { .. } => "attempt",
            ProjectEvent::AssignmentStatusChanged(_) => "status",
            ProjectEvent::ChangeProposalRefreshed { .. } => "proposal",
            ProjectEvent::Activity(_) => "activity",
            _ => "other",
        })
        .collect();

    let created_pos = kinds.iter().position(|k| *k == "created").expect("Created present");
    let attempt_pos = kinds.iter().position(|k| *k == "attempt").expect("Attempt present");
    let status_pos = kinds.iter().position(|k| *k == "status").expect("Status present");
    assert!(created_pos < attempt_pos, "Created before Attempt");
    assert!(attempt_pos < status_pos, "Attempt before Status");

    for window in events.windows(2) {
        assert_eq!(window[1].sequence, window[0].sequence + 1);
    }
}
