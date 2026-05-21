//! Tests for issue #24: Expose comprehensive Issue Assignment Activity on Project detail.
//!
//! Covers the API projection of `project_activity` rows and verifies that every
//! lifecycle path enumerated by the issue (start, block, recover, abandon,
//! proposal open/verify/repair, complete, cleanup) records a truthful Activity
//! entry without leaking full Codex output.

use agentic_afk_contracts::{
    CreateProjectRequest, EnableIssueSourceRequest, IssueAssignmentResponse, IssueSource,
    ProjectActivityEntryResponse, ProjectResponse, RepairAssignmentRequest, SourceIssueSnapshot,
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

async fn test_router(
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

fn setup_local_markdown_project(name: &str) -> (PathBuf, PathBuf) {
    let project_path = temp_project_path(name);
    let issues_dir = project_path.join(".scratch/issues");
    std::fs::create_dir_all(&issues_dir).unwrap();
    std::fs::create_dir_all(project_path.join(".git")).unwrap();
    (project_path, issues_dir)
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

    app.clone()
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

async fn fetch_activity(
    app: &axum::Router,
    project_id: &str,
) -> Vec<ProjectActivityEntryResponse> {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/projects/{project_id}/activity"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).unwrap()
}

/// Worktrunk fake that creates a per-call worktree directory on `switch` and
/// removes it on `remove`.
fn fake_worktrunk(name: &str, worktree: &std::path::Path) -> PathBuf {
    write_fake_command(
        name,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nif [ \"$1\" = \"switch\" ]; then\n  mkdir -p '{wt}'\n  printf '{{\"path\":\"{wt}\"}}\\n'\n  exit 0\nfi\nif [ \"$1\" = \"remove\" ]; then\n  rm -rf '{wt}'\n  exit 0\nfi\nexit 9\n",
            wt = worktree.display(),
        ),
    )
}

fn fake_codex_blocked(name: &str) -> PathBuf {
    write_fake_command(
        name,
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nlast=\"\"\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"--output-last-message\" ]; then shift; last=\"$1\"; fi\n  shift\ndone\nprintf '{\"outcome\":\"Blocked\",\"summary\":\"need input\"}\\n' > \"$last\"\n",
    )
}

/// Tracer: starting and abandoning an assignment surfaces Activity entries
/// through the new `GET /api/projects/{id}/activity` endpoint, newest first.
#[tokio::test]
async fn activity_endpoint_returns_recorded_lifecycle_entries_newest_first() {
    let (project_path, issues_dir) = setup_local_markdown_project("activity-tracer");
    std::fs::write(
        issues_dir.join("traceable.md"),
        "# Traceable\n\nReadiness: ready\nSource Order: 1\n",
    )
    .unwrap();
    let worktree = temp_project_path("activity-tracer-wt");
    let (app, _db) = test_router(
        fake_worktrunk("activity-tracer-fake-wt", &worktree),
        fake_codex_blocked("activity-tracer-fake-codex"),
    )
    .await;
    let project = create_trusted_local_markdown_project(&app, &project_path).await;
    sync_local_markdown(&app, &project).await;
    let assignment = start_assignment(&app, &project, "traceable").await;

    // Abandon to seed the existing assignment_abandoned emit (issue #18).
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/projects/{}/assignments/{}/abandon",
                    project.id.0, assignment.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let entries = fetch_activity(&app, &project.id.0).await;
    assert!(
        entries
            .iter()
            .any(|entry| entry.kind == "assignment_abandoned"
                && entry.assignment_id.as_deref() == Some(assignment.id.as_str())),
        "expected an assignment_abandoned entry, got: {entries:?}"
    );
    // Newest-first ordering: scan recorded_at timestamps weakly monotonic
    // decreasing (within the same second equal is fine).
    for window in entries.windows(2) {
        assert!(
            window[0].recorded_at >= window[1].recorded_at,
            "entries not newest first: {entries:?}"
        );
    }
}

/// Starting an Issue Assignment records an `assignment_started` Activity entry
/// after the Assignment Worktree is claimed and the Codex process spawned, so
/// Project detail can show the start fact without inspecting durable
/// assignment state.
#[tokio::test]
async fn start_assignment_records_assignment_started_activity() {
    let (project_path, issues_dir) = setup_local_markdown_project("activity-started");
    std::fs::write(
        issues_dir.join("startable.md"),
        "# Startable\n\nReadiness: ready\nSource Order: 1\n",
    )
    .unwrap();
    let worktree = temp_project_path("activity-started-wt");
    let (app, _db) = test_router(
        fake_worktrunk("activity-started-fake-wt", &worktree),
        fake_codex_blocked("activity-started-fake-codex"),
    )
    .await;
    let project = create_trusted_local_markdown_project(&app, &project_path).await;
    sync_local_markdown(&app, &project).await;
    let assignment = start_assignment(&app, &project, "startable").await;

    let entries = fetch_activity(&app, &project.id.0).await;
    let started = entries
        .iter()
        .find(|entry| entry.kind == "assignment_started")
        .unwrap_or_else(|| {
            panic!("expected an assignment_started entry, got: {entries:?}");
        });
    assert_eq!(started.assignment_id.as_deref(), Some(assignment.id.as_str()));
    assert_eq!(started.detail.as_deref(), Some(assignment.branch.as_str()));
}

/// A Codex Blocked outcome on the initial attempt records an
/// `assignment_blocked` Activity entry alongside the start entry.
#[tokio::test]
async fn blocked_initial_outcome_records_assignment_blocked_activity() {
    let (project_path, issues_dir) = setup_local_markdown_project("activity-blocked");
    std::fs::write(
        issues_dir.join("blockable.md"),
        "# Blockable\n\nReadiness: ready\nSource Order: 1\n",
    )
    .unwrap();
    let worktree = temp_project_path("activity-blocked-wt");
    let (app, _db) = test_router(
        fake_worktrunk("activity-blocked-fake-wt", &worktree),
        fake_codex_blocked("activity-blocked-fake-codex"),
    )
    .await;
    let project = create_trusted_local_markdown_project(&app, &project_path).await;
    sync_local_markdown(&app, &project).await;
    let assignment = start_assignment(&app, &project, "blockable").await;
    assert_eq!(assignment.status, "blocked");

    let entries = fetch_activity(&app, &project.id.0).await;
    let blocked = entries
        .iter()
        .find(|entry| {
            entry.kind == "assignment_blocked"
                && entry.assignment_id.as_deref() == Some(assignment.id.as_str())
        })
        .unwrap_or_else(|| panic!("expected assignment_blocked, got: {entries:?}"));
    assert!(blocked.detail.is_some());
}

/// Recovering a blocked assignment records an `assignment_recovered` Activity
/// entry so Project detail shows recovery attempts distinctly from the original
/// start.
#[tokio::test]
async fn recover_records_assignment_recovered_activity() {
    let (project_path, issues_dir) = setup_local_markdown_project("activity-recover");
    std::fs::write(
        issues_dir.join("recoverable.md"),
        "# Recoverable\n\nReadiness: ready\nSource Order: 1\n",
    )
    .unwrap();
    let worktree = temp_project_path("activity-recover-wt");
    let (app, _db) = test_router(
        fake_worktrunk("activity-recover-fake-wt", &worktree),
        fake_codex_blocked("activity-recover-fake-codex"),
    )
    .await;
    let project = create_trusted_local_markdown_project(&app, &project_path).await;
    sync_local_markdown(&app, &project).await;
    let assignment = start_assignment(&app, &project, "recoverable").await;
    assert_eq!(assignment.status, "blocked");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/projects/{}/assignments/{}/recover",
                    project.id.0, assignment.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let entries = fetch_activity(&app, &project.id.0).await;
    assert!(
        entries.iter().any(|entry| entry.kind == "assignment_recovered"
            && entry.assignment_id.as_deref() == Some(assignment.id.as_str())),
        "expected assignment_recovered, got: {entries:?}"
    );
}

/// Detail is truncated so a huge Codex summary never lands in Activity.
#[tokio::test]
async fn activity_detail_is_truncated_to_protect_against_codex_output() {
    use agentic_afk_persistence::PROJECT_ACTIVITY_DETAIL_MAX_BYTES;

    let (project_path, _issues_dir) = setup_local_markdown_project("activity-truncate");
    let (app, db) = test_router(
        write_fake_command("activity-truncate-wt", "#!/bin/sh\nexit 0\n"),
        write_fake_command("activity-truncate-codex", "#!/bin/sh\nexit 0\n"),
    )
    .await;
    let project = create_trusted_local_markdown_project(&app, &project_path).await;
    let huge_detail = "x".repeat(PROJECT_ACTIVITY_DETAIL_MAX_BYTES * 4);
    let entry = agentic_afk_persistence::record_project_activity(
        &db,
        &project.id.0,
        None,
        "test_truncation",
        Some(&huge_detail),
    )
    .await
    .unwrap();
    let stored = entry.detail.unwrap();
    assert!(stored.len() <= PROJECT_ACTIVITY_DETAIL_MAX_BYTES + 8);
    assert!(stored.ends_with('…'));
}

/// Seed a GitHub-source Project with one assignment that already has a pending
/// Change Proposal, so verify/repair handlers can be exercised without going
/// through the full Codex start flow.
async fn seed_github_assignment_with_proposal(
    db: &persistence::Db,
    project_path: &std::path::Path,
    worktree_path: &std::path::Path,
) -> (String, IssueAssignmentResponse) {
    std::fs::create_dir_all(project_path).unwrap();
    std::fs::create_dir_all(worktree_path).unwrap();
    let project = persistence::create_project(
        db,
        &CreateProjectRequest {
            path: project_path.display().to_string(),
        },
    )
    .await
    .unwrap();
    persistence::trust_project(db, &project.id.0).await.unwrap();
    persistence::enable_issue_source(
        db,
        &project.id.0,
        &EnableIssueSourceRequest {
            kind: "github".to_string(),
            locator: "owner/repo".to_string(),
        },
    )
    .await
    .unwrap();
    let source = IssueSource {
        kind: "github".to_string(),
        locator: "owner/repo".to_string(),
    };
    let issue = SourceIssueSnapshot {
        source_id: "21".to_string(),
        title: "Proposal".to_string(),
        readiness: "ready".to_string(),
        lifecycle_status: "running".to_string(),
        parent_issue: None,
        issue_dependencies: vec![],
        source_order: 1,
        raw_text: "body".to_string(),
    };
    let assignment =
        persistence::create_issue_assignment(db, &project.id.0, &source, &issue, "agentic-afk/github-21")
            .await
            .unwrap();
    let assignment = persistence::set_assignment_worktree(
        db,
        &assignment.id,
        &worktree_path.display().to_string(),
    )
    .await
    .unwrap();
    let assignment = persistence::set_assignment_status(db, &assignment.id, "running", None)
        .await
        .unwrap();
    let assignment = persistence::set_assignment_change_proposal(
        db,
        &assignment.id,
        "pending",
        "https://github.com/owner/repo/pull/42",
    )
    .await
    .unwrap();
    (project.id.0, assignment)
}

fn fake_gh_verify(name: &str, checks_state: &str, merged: bool) -> PathBuf {
    let merged_json = if merged {
        r#"{"state":"MERGED","mergedAt":"2026-05-21T10:00:00Z","mergeCommit":{"oid":"abc"}}"#
    } else {
        r#"{"state":"OPEN","mergedAt":null,"mergeCommit":null}"#
    };
    let checks_json = match checks_state {
        "passing" => r#"[{"name":"ci","state":"SUCCESS","bucket":"pass"}]"#,
        "failing" => r#"[{"name":"ci","state":"FAILURE","bucket":"fail"}]"#,
        _ => r#"[{"name":"ci","state":"IN_PROGRESS","bucket":"pending"}]"#,
    };
    let checks_exit = if checks_state == "failing" {
        1
    } else if checks_state == "passing" {
        0
    } else {
        8
    };
    let body = format!(
        r#"#!/bin/sh
if [ "$1" = "auth" ]; then exit 0; fi
if [ "$1" = "label" ]; then exit 0; fi
if [ "$1" = "issue" ]; then exit 0; fi
if [ "$1" = "pr" ] && [ "$2" = "view" ]; then printf '%s\n' '{merged_json}'; exit 0; fi
if [ "$1" = "pr" ] && [ "$2" = "checks" ]; then printf '%s\n' '{checks_json}'; exit {checks_exit}; fi
exit 0
"#,
    );
    write_fake_command(name, &body)
}

fn run_git(path: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .current_dir(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn setup_git_project_with_remote(name: &str) -> PathBuf {
    let project_path = temp_project_path(name);
    let remote_path = temp_project_path(&format!("{name}-remote"));
    std::fs::create_dir_all(&project_path).unwrap();
    std::fs::create_dir_all(&remote_path).unwrap();
    run_git(&project_path, &["init", "-b", "main"]);
    run_git(&project_path, &["config", "user.name", "Activity Test"]);
    run_git(&project_path, &["config", "user.email", "t@example.invalid"]);
    run_git(&project_path, &["config", "commit.gpgsign", "false"]);
    std::fs::write(project_path.join("README.md"), "x\n").unwrap();
    run_git(&project_path, &["add", "README.md"]);
    run_git(&project_path, &["commit", "-m", "init"]);
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
    run_git(
        &project_path,
        &[
            "config",
            &format!("url.{}.insteadOf", remote_path.to_str().unwrap()),
            "git@github.com:owner/repo.git",
        ],
    );
    run_git(&project_path, &["push", "-u", "origin", "main"]);
    project_path
}

fn proposal_gh_fake(name: &str) -> PathBuf {
    write_fake_command(
        name,
        r#"#!/bin/sh
if [ "$1" = "auth" ]; then exit 0; fi
if [ "$1" = "label" ]; then exit 0; fi
if [ "$1" = "issue" ] && [ "$2" = "list" ]; then
  printf '[{"number":21,"title":"Proposal","body":"do work","labels":[{"name":"ready-for-agent"}]}]\n'
  exit 0
fi
if [ "$1" = "issue" ]; then exit 0; fi
if [ "$1" = "pr" ] && [ "$2" = "create" ]; then printf 'https://github.com/owner/repo/pull/42\n'; exit 0; fi
if [ "$1" = "pr" ] && [ "$2" = "view" ]; then printf '{"state":"OPEN","mergedAt":null,"mergeCommit":null}\n'; exit 0; fi
if [ "$1" = "pr" ] && [ "$2" = "checks" ]; then printf '[{"name":"ci","state":"IN_PROGRESS","bucket":"pending"}]\n'; exit 8; fi
exit 0
"#,
    )
}

fn proposal_wt_fake(name: &str) -> PathBuf {
    write_fake_command(
        name,
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then exit 0; fi
if [ "$1" = "switch" ] && [ "$2" = "--create" ]; then
  branch="$3"
  git switch -c "$branch" >/dev/null 2>&1 || true
  printf '{"path":"%s"}\n' "$PWD"
  exit 0
fi
if [ "$1" = "remove" ]; then exit 0; fi
exit 0
"#,
    )
}

fn codex_ready_for_proposal_fake(name: &str) -> PathBuf {
    write_fake_command(
        name,
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nlast=\nwhile [ \"$#\" -gt 0 ]; do if [ \"$1\" = \"--output-last-message\" ]; then shift; last=\"$1\"; fi; shift; done\nprintf '{\"outcome\":\"ReadyForProposal\",\"summary\":\"ok\"}\\n' > \"$last\"\n",
    )
}

async fn create_trusted_github_project(
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
    let project: ProjectResponse =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    app.clone()
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
    let trust = app
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
    assert_eq!(trust.status(), StatusCode::OK);
    serde_json::from_slice(&trust.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

/// Starting a GitHub-backed assignment whose Codex returns `ReadyForProposal`
/// records a `change_proposal_opened` Activity entry after the pull request URL
/// is persisted.
#[tokio::test]
async fn start_assignment_ready_for_proposal_records_change_proposal_opened_activity() {
    let project_path = setup_git_project_with_remote("activity-proposal-opened");
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "target/dx/agentic-afk-dashboard/release/web/public".into(),
        database_url: "sqlite::memory:".into(),
        gh_binary_path: proposal_gh_fake("activity-proposal-opened-gh"),
        worktrunk_binary_path: proposal_wt_fake("activity-proposal-opened-wt"),
        codex_binary_path: codex_ready_for_proposal_fake("activity-proposal-opened-codex"),
    };
    let app = router(config, db);
    let project = create_trusted_github_project(&app, &project_path).await;
    sync_local_markdown(&app, &project).await; // syncs whatever source is enabled
    let assignment = start_assignment(&app, &project, "21").await;
    assert_eq!(assignment.status, "proposal_pending");

    let entries = fetch_activity(&app, &project.id.0).await;
    assert!(
        entries.iter().any(|entry| entry.kind == "change_proposal_opened"
            && entry.assignment_id.as_deref() == Some(assignment.id.as_str())),
        "expected change_proposal_opened, got: {entries:?}"
    );
}

/// Passing required checks on a verified Change Proposal record a
/// `change_proposal_verified` Activity entry.
#[tokio::test]
async fn proposal_verified_records_activity_entry() {
    let project_path = temp_project_path("activity-verified-project");
    let worktree_path = temp_project_path("activity-verified-wt");
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let (project_id, assignment) =
        seed_github_assignment_with_proposal(&db, &project_path, &worktree_path).await;
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "target/dx/agentic-afk-dashboard/release/web/public".into(),
        database_url: "sqlite::memory:".into(),
        gh_binary_path: fake_gh_verify("activity-verified-gh", "passing", false),
        worktrunk_binary_path: write_fake_command("activity-verified-wt-bin", "#!/bin/sh\nexit 0\n"),
        codex_binary_path: write_fake_command(
            "activity-verified-codex",
            "#!/bin/sh\nexit 0\n",
        ),
    };
    let app = router(config, db);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/projects/{project_id}/assignments/{}/refresh-proposal-state",
                    assignment.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let entries = fetch_activity(&app, &project_id).await;
    assert!(
        entries.iter().any(|entry| entry.kind == "change_proposal_verified"
            && entry.assignment_id.as_deref() == Some(assignment.id.as_str())),
        "expected change_proposal_verified, got: {entries:?}"
    );
}

/// A merged Change Proposal records both `assignment_completed` and
/// `assignment_cleanup` Activity entries.
#[tokio::test]
async fn human_merge_records_completed_and_cleanup_activity() {
    let project_path = temp_project_path("activity-merged-project");
    let worktree_path = temp_project_path("activity-merged-wt");
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let (project_id, assignment) =
        seed_github_assignment_with_proposal(&db, &project_path, &worktree_path).await;
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "target/dx/agentic-afk-dashboard/release/web/public".into(),
        database_url: "sqlite::memory:".into(),
        gh_binary_path: fake_gh_verify("activity-merged-gh", "passing", true),
        worktrunk_binary_path: write_fake_command("activity-merged-wt-bin", "#!/bin/sh\nexit 0\n"),
        codex_binary_path: write_fake_command(
            "activity-merged-codex",
            "#!/bin/sh\nexit 0\n",
        ),
    };
    let app = router(config, db);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/projects/{project_id}/assignments/{}/refresh-proposal-state",
                    assignment.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let entries = fetch_activity(&app, &project_id).await;
    let kinds: Vec<&str> = entries.iter().map(|e| e.kind.as_str()).collect();
    assert!(
        kinds.contains(&"assignment_completed"),
        "expected assignment_completed, got: {kinds:?}"
    );
    assert!(
        kinds.contains(&"assignment_cleanup"),
        "expected assignment_cleanup, got: {kinds:?}"
    );
}

/// A successful Repair Attempt that produces ReadyForProposal records a
/// `change_proposal_repaired` Activity entry.
#[tokio::test]
async fn successful_repair_records_change_proposal_repaired_activity() {
    let project_path = temp_project_path("activity-repair-project");
    let worktree_path = temp_project_path("activity-repair-wt");
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let (project_id, assignment) =
        seed_github_assignment_with_proposal(&db, &project_path, &worktree_path).await;
    // Mark proposal as failed first so the repair endpoint accepts the request.
    persistence::set_assignment_change_proposal(
        &db,
        &assignment.id,
        "failed",
        "https://github.com/owner/repo/pull/42",
    )
    .await
    .unwrap();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "target/dx/agentic-afk-dashboard/release/web/public".into(),
        database_url: "sqlite::memory:".into(),
        gh_binary_path: write_fake_command("activity-repair-gh", "#!/bin/sh\nexit 0\n"),
        worktrunk_binary_path: write_fake_command("activity-repair-wt-bin", "#!/bin/sh\nexit 0\n"),
        codex_binary_path: write_fake_command(
            "activity-repair-codex",
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nlast=\nwhile [ \"$#\" -gt 0 ]; do if [ \"$1\" = \"--output-last-message\" ]; then shift; last=\"$1\"; fi; shift; done\nprintf '{\"outcome\":\"ReadyForProposal\",\"summary\":\"fixed\"}\\n' > \"$last\"\n",
        ),
    };
    let app = router(config, db);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/projects/{project_id}/assignments/{}/repair",
                    assignment.id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&RepairAssignmentRequest::default()).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let entries = fetch_activity(&app, &project_id).await;
    assert!(
        entries.iter().any(|entry| entry.kind == "change_proposal_repaired"
            && entry.assignment_id.as_deref() == Some(assignment.id.as_str())),
        "expected change_proposal_repaired, got: {entries:?}"
    );
}

/// The activity endpoint rejects unknown projects with a problem+json 404.
#[tokio::test]
async fn activity_endpoint_returns_404_for_unknown_project() {
    let (app, _db) = test_router(
        write_fake_command("activity-unknown-wt", "#!/bin/sh\nexit 0\n"),
        write_fake_command("activity-unknown-codex", "#!/bin/sh\nexit 0\n"),
    )
    .await;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/projects/does-not-exist/activity")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
