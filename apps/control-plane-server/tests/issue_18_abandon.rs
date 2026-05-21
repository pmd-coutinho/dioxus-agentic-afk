//! Tests for issue #18: Abandon a local markdown Issue Assignment.

use agentic_afk_contracts::{
    CreateProjectRequest, EnableIssueSourceRequest, IssueAssignmentResponse, ProblemDetail,
    ProjectAssignmentStateResponse, ProjectResponse,
};
use agentic_afk_control_plane_server::{ControlPlaneConfig, router};
use agentic_afk_persistence::{self as persistence};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use tower::ServiceExt;

fn temp_project_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "agentic-afk-{name}-{}-{}",
        std::process::id(),
        unique_nonce()
    ))
}

fn unique_nonce() -> u128 {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let salt = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    nanos.wrapping_add(u128::from(salt))
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

fn setup_local_markdown_project(name: &str) -> (PathBuf, PathBuf) {
    let project_path = temp_project_path(name);
    let issues_dir = project_path.join(".scratch/issues");
    std::fs::create_dir_all(&issues_dir).unwrap();
    (project_path, issues_dir)
}

async fn test_router_with_execution(
    worktrunk_binary_path: PathBuf,
    codex_binary_path: PathBuf,
) -> (axum::Router, persistence::Db) {
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "target/dx/agentic-afk-dashboard/release/web/public".into(),
        database_url: "sqlite::memory:".into(),
        gh_binary_path: "gh".into(),
        worktrunk_binary_path,
        codex_binary_path,
    };
    (router(config, db.clone()), db)
}

async fn create_trusted_local_markdown_project(
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

async fn sync_local_markdown(app: &axum::Router, project: &ProjectResponse) {
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

async fn start_assignment(
    app: &axum::Router,
    project: &ProjectResponse,
    source_id: &str,
) -> IssueAssignmentResponse {
    let response = app
        .clone()
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
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).unwrap()
}

async fn abandon(
    app: &axum::Router,
    project_id: &str,
    assignment_id: &str,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/projects/{project_id}/assignments/{assignment_id}/abandon"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

/// Tracer bullet: abandoning a blocked local markdown Issue Assignment removes the
/// Assignment Worktree, marks the assignment abandoned, returns the Source Issue
/// lifecycle to ready, and records an Activity entry.
#[tokio::test]
async fn abandon_blocked_local_markdown_assignment_clears_worktree_and_returns_to_ready() {
    let (project_path, issues_dir) = setup_local_markdown_project("abandon-blocked");
    std::fs::create_dir_all(project_path.join(".git")).unwrap();
    std::fs::write(
        issues_dir.join("blocked-issue.md"),
        "# Blocked\n\nReadiness: ready\nSource Order: 1\n",
    )
    .unwrap();

    let worktree_path = temp_project_path("abandon-blocked-worktree");
    let wt_log = temp_project_path("abandon-blocked-wt-log");
    let fake_wt = write_fake_command(
        "abandon-blocked-fake-wt",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{log}'\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nif [ \"$1\" = \"switch\" ]; then\n  mkdir -p '{worktree}'\n  printf '{{\"path\":\"{worktree}\"}}\\n'\n  exit 0\nfi\nif [ \"$1\" = \"remove\" ]; then\n  rm -rf '{worktree}'\n  exit 0\nfi\nexit 9\n",
            log = wt_log.display(),
            worktree = worktree_path.display(),
        ),
    );
    let fake_codex = write_fake_command(
        "abandon-blocked-fake-codex",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nlast=\"\"\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"--output-last-message\" ]; then shift; last=\"$1\"; fi\n  shift\ndone\nprintf '{\"outcome\":\"Blocked\",\"summary\":\"need input\"}\\n' > \"$last\"\n",
    );
    let (app, db) = test_router_with_execution(fake_wt, fake_codex).await;
    let project = create_trusted_local_markdown_project(&app, &project_path).await;
    sync_local_markdown(&app, &project).await;
    let assignment = start_assignment(&app, &project, "blocked-issue").await;
    assert_eq!(assignment.status, "blocked");
    assert!(std::path::Path::new(&assignment.worktree_path).exists());

    let response = abandon(&app, &project.id.0, &assignment.id).await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let abandoned: IssueAssignmentResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(abandoned.id, assignment.id);
    assert_eq!(abandoned.status, "abandoned");
    assert!(!std::path::Path::new(&assignment.worktree_path).exists());

    let log = std::fs::read_to_string(&wt_log).unwrap();
    assert!(
        log.contains("remove"),
        "Worktrunk should be asked to remove the worktree, got log: {log}"
    );
    assert!(log.contains(&assignment.branch));

    // Issue file lifecycle status returned to ready.
    let issue_text = std::fs::read_to_string(issues_dir.join("blocked-issue.md")).unwrap();
    assert!(
        issue_text.contains("Lifecycle Status: ready"),
        "expected lifecycle returned to ready, got: {issue_text}"
    );

    // No active assignment left for the Project.
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
    let body = state.into_body().collect().await.unwrap().to_bytes();
    let state: ProjectAssignmentStateResponse = serde_json::from_slice(&body).unwrap();
    assert!(state.active_assignment.is_none());

    // Activity row recorded for abandonment.
    let activity =
        agentic_afk_persistence::list_project_activity(&db, &project.id.0, 50)
            .await
            .unwrap();
    assert!(
        activity
            .iter()
            .any(|entry| entry.kind == "assignment_abandoned"
                && entry.assignment_id.as_deref() == Some(assignment.id.as_str())),
        "expected an assignment_abandoned Activity entry, got: {activity:?}"
    );
}

/// Abandoning a non-blocked active assignment is rejected so non-blocked work is
/// not silently discarded.
#[tokio::test]
async fn abandon_rejects_non_blocked_assignment() {
    let (project_path, issues_dir) = setup_local_markdown_project("abandon-running");
    std::fs::create_dir_all(project_path.join(".git")).unwrap();
    std::fs::write(
        issues_dir.join("running.md"),
        "# Running\n\nReadiness: ready\nSource Order: 1\n",
    )
    .unwrap();

    let (app, db) = test_router_with_execution(
        write_fake_command("abandon-running-noop-wt", "#!/bin/sh\nexit 0\n"),
        write_fake_command("abandon-running-noop-codex", "#!/bin/sh\nexit 0\n"),
    )
    .await;
    let project = create_trusted_local_markdown_project(&app, &project_path).await;
    sync_local_markdown(&app, &project).await;

    let issue = persistence::get_planning_snapshot(&db, &project.id.0)
        .await
        .unwrap()
        .eligible
        .into_iter()
        .find(|issue| issue.source_id == "running")
        .unwrap();
    let assignment = persistence::create_issue_assignment(
        &db,
        &project.id.0,
        project.enabled_issue_source.as_ref().unwrap(),
        &issue,
        "agentic-afk/local-markdown-running",
    )
    .await
    .unwrap();
    let assignment = persistence::set_assignment_worktree(&db, &assignment.id, "/tmp/running")
        .await
        .unwrap();
    let assignment = persistence::set_assignment_status(&db, &assignment.id, "running", None)
        .await
        .unwrap();

    let response = abandon(&app, &project.id.0, &assignment.id).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let problem: ProblemDetail = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        problem.problem_type,
        "urn:agentic-afk:assignment-not-abandonable"
    );
}

/// Abandoning an unknown assignment id returns 404.
#[tokio::test]
async fn abandon_unknown_assignment_returns_not_found() {
    let (project_path, _issues_dir) = setup_local_markdown_project("abandon-missing");
    let (app, _db) = test_router_with_execution(
        write_fake_command("abandon-missing-noop-wt", "#!/bin/sh\nexit 0\n"),
        write_fake_command("abandon-missing-noop-codex", "#!/bin/sh\nexit 0\n"),
    )
    .await;
    let project = create_trusted_local_markdown_project(&app, &project_path).await;

    let response = abandon(&app, &project.id.0, "does-not-exist").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// Abandoning an assignment that belongs to a different Project is rejected so
/// ownership is enforced through the API.
#[tokio::test]
async fn abandon_rejects_assignment_from_different_project() {
    let (project_a_path, issues_dir_a) = setup_local_markdown_project("abandon-owner-a");
    std::fs::create_dir_all(project_a_path.join(".git")).unwrap();
    std::fs::write(
        issues_dir_a.join("a-issue.md"),
        "# A\n\nReadiness: ready\nSource Order: 1\n",
    )
    .unwrap();
    let (project_b_path, _issues_dir_b) = setup_local_markdown_project("abandon-owner-b");

    let (app, db) = test_router_with_execution(
        write_fake_command("abandon-owner-noop-wt", "#!/bin/sh\nexit 0\n"),
        write_fake_command("abandon-owner-noop-codex", "#!/bin/sh\nexit 0\n"),
    )
    .await;

    let project_a = create_trusted_local_markdown_project(&app, &project_a_path).await;
    let project_b = create_trusted_local_markdown_project(&app, &project_b_path).await;
    sync_local_markdown(&app, &project_a).await;

    let issue = persistence::get_planning_snapshot(&db, &project_a.id.0)
        .await
        .unwrap()
        .eligible
        .into_iter()
        .find(|issue| issue.source_id == "a-issue")
        .unwrap();
    let assignment = persistence::create_issue_assignment(
        &db,
        &project_a.id.0,
        project_a.enabled_issue_source.as_ref().unwrap(),
        &issue,
        "agentic-afk/local-markdown-a-issue",
    )
    .await
    .unwrap();
    let assignment = persistence::set_assignment_worktree(&db, &assignment.id, "/tmp/a-issue")
        .await
        .unwrap();
    let assignment =
        persistence::set_assignment_status(&db, &assignment.id, "blocked", Some("need input"))
            .await
            .unwrap();

    // Attempt abandon under project B's URL.
    let response = abandon(&app, &project_b.id.0, &assignment.id).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// When Worktrunk cleanup fails, the API surfaces a cleanup failure and the
/// assignment is left in blocked state so the developer can retry.
#[tokio::test]
async fn abandon_cleanup_failure_preserves_blocked_assignment() {
    let (project_path, issues_dir) = setup_local_markdown_project("abandon-cleanup-fail");
    std::fs::create_dir_all(project_path.join(".git")).unwrap();
    std::fs::write(
        issues_dir.join("cleanup.md"),
        "# Cleanup\n\nReadiness: ready\nSource Order: 1\n",
    )
    .unwrap();
    let worktree_path = temp_project_path("abandon-cleanup-fail-worktree");
    let fake_wt = write_fake_command(
        "abandon-cleanup-fail-fake-wt",
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nif [ \"$1\" = \"switch\" ]; then\n  mkdir -p '{worktree}'\n  printf '{{\"path\":\"{worktree}\"}}\\n'\n  exit 0\nfi\nif [ \"$1\" = \"remove\" ]; then\n  printf 'cleanup failed\\n' >&2\n  exit 7\nfi\nexit 9\n",
            worktree = worktree_path.display(),
        ),
    );
    let fake_codex = write_fake_command(
        "abandon-cleanup-fail-fake-codex",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nlast=\"\"\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"--output-last-message\" ]; then shift; last=\"$1\"; fi\n  shift\ndone\nprintf '{\"outcome\":\"Blocked\",\"summary\":\"need input\"}\\n' > \"$last\"\n",
    );
    let (app, _db) = test_router_with_execution(fake_wt, fake_codex).await;
    let project = create_trusted_local_markdown_project(&app, &project_path).await;
    sync_local_markdown(&app, &project).await;
    let assignment = start_assignment(&app, &project, "cleanup").await;
    assert_eq!(assignment.status, "blocked");

    let response = abandon(&app, &project.id.0, &assignment.id).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let problem: ProblemDetail = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        problem.problem_type,
        "urn:agentic-afk:assignment-cleanup-failed"
    );

    // Assignment remains blocked and is still the active assignment.
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
    let body = state.into_body().collect().await.unwrap().to_bytes();
    let state: ProjectAssignmentStateResponse = serde_json::from_slice(&body).unwrap();
    let active = state.active_assignment.unwrap();
    assert_eq!(active.id, assignment.id);
    assert_eq!(active.status, "blocked");
}

/// After abandonment the Source Issue is eligible again so a fresh Issue
/// Assignment can be started for the same Source Issue.
#[tokio::test]
async fn abandoned_source_issue_can_be_started_again() {
    let (project_path, issues_dir) = setup_local_markdown_project("abandon-restart");
    std::fs::create_dir_all(project_path.join(".git")).unwrap();
    std::fs::write(
        issues_dir.join("restart-me.md"),
        "# Restart\n\nReadiness: ready\nSource Order: 1\n",
    )
    .unwrap();
    let worktree_path = temp_project_path("abandon-restart-worktree");
    let fake_wt = write_fake_command(
        "abandon-restart-fake-wt",
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nif [ \"$1\" = \"switch\" ]; then\n  mkdir -p '{worktree}'\n  printf '{{\"path\":\"{worktree}\"}}\\n'\n  exit 0\nfi\nif [ \"$1\" = \"remove\" ]; then\n  rm -rf '{worktree}'\n  exit 0\nfi\nexit 9\n",
            worktree = worktree_path.display(),
        ),
    );
    let fake_codex = write_fake_command(
        "abandon-restart-fake-codex",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nlast=\"\"\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"--output-last-message\" ]; then shift; last=\"$1\"; fi\n  shift\ndone\nprintf '{\"outcome\":\"Blocked\",\"summary\":\"need input\"}\\n' > \"$last\"\n",
    );
    let (app, _db) = test_router_with_execution(fake_wt, fake_codex).await;
    let project = create_trusted_local_markdown_project(&app, &project_path).await;
    sync_local_markdown(&app, &project).await;
    let first = start_assignment(&app, &project, "restart-me").await;
    let abandon_response = abandon(&app, &project.id.0, &first.id).await;
    assert_eq!(abandon_response.status(), StatusCode::OK);

    // Re-sync so planning reflects ready lifecycle and the issue is eligible.
    sync_local_markdown(&app, &project).await;
    let second = start_assignment(&app, &project, "restart-me").await;
    assert_ne!(second.id, first.id);
    assert_eq!(second.source_id, "restart-me");
}
