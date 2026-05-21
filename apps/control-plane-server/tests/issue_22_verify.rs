//! Integration tests for issue #22: Verify GitHub Change Proposals and complete Human Merge cleanup.
//!
//! Exercises the refresh-proposal-state endpoint, required-check inspection,
//! Verified Change Proposal slot release, Human Merge detection, completion
//! write-back, and accepted Assignment Worktree + branch cleanup.

use agentic_afk_contracts::{
    CreateProjectRequest, EnableIssueSourceRequest, IssueAssignmentResponse, ProblemDetail,
    ProjectAssignmentStateResponse, ProjectResponse,
};
use agentic_afk_control_plane_server::{ControlPlaneConfig, router};
use agentic_afk_persistence as persistence;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use tower::ServiceExt;

fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "agentic-afk-verify-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn write_fake_command(name: &str, body: &str) -> PathBuf {
    let path = temp_path(name);
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
    worktrunk: PathBuf,
    codex: PathBuf,
    gh: PathBuf,
) -> (axum::Router, persistence::Db) {
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "apps/dashboard/dist".into(),
        database_url: "sqlite::memory:".into(),
        gh_binary_path: gh,
        worktrunk_binary_path: worktrunk,
        codex_binary_path: codex,
    };
    (router(config, db.clone()), db)
}

fn run_git(path: &Path, args: &[&str]) {
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
}

fn setup_git_project_with_remote(name: &str) -> (PathBuf, PathBuf) {
    let project_path = temp_path(name);
    let remote_path = temp_path(&format!("{name}-remote"));
    std::fs::create_dir_all(&project_path).unwrap();
    std::fs::create_dir_all(&remote_path).unwrap();
    run_git(&project_path, &["init", "-b", "main"]);
    run_git(&project_path, &["config", "user.name", "Agentic AFK Test"]);
    run_git(
        &project_path,
        &["config", "user.email", "agentic-afk@example.invalid"],
    );
    run_git(&project_path, &["config", "commit.gpgsign", "false"]);
    run_git(&project_path, &["config", "tag.gpgsign", "false"]);
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

async fn create_trusted_github_project(app: &axum::Router, path: &Path) -> ProjectResponse {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/projects")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&CreateProjectRequest {
                        path: path.display().to_string(),
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
    serde_json::from_slice(
        &trust_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes(),
    )
    .unwrap()
}

/// Build a fake `gh` script that switches behavior based on environment files.
///
/// `checks_state_path` should contain one of: "pending", "passing", "failing".
/// `merge_state_path` should contain one of: "open", "merged".
fn write_proposal_gh(
    name: &str,
    log_path: &Path,
    checks_state_path: &Path,
    merge_state_path: &Path,
) -> PathBuf {
    let body = format!(
        r#"#!/bin/sh
printf '%s\n' "$*" >> '{log}'
if [ "$1" = "auth" ]; then exit 0; fi
if [ "$1" = "label" ]; then exit 0; fi
if [ "$1" = "issue" ] && [ "$2" = "list" ]; then
  printf '[{{"number":21,"title":"Proposal","body":"do work","labels":[{{"name":"ready-for-agent"}}]}}]\n'
  exit 0
fi
if [ "$1" = "issue" ] && [ "$2" = "edit" ]; then exit 0; fi
if [ "$1" = "issue" ] && [ "$2" = "comment" ]; then exit 0; fi
if [ "$1" = "pr" ] && [ "$2" = "create" ]; then
  printf 'https://github.com/owner/repo/pull/42\n'
  exit 0
fi
if [ "$1" = "pr" ] && [ "$2" = "checks" ]; then
  state=$(cat '{checks}' 2>/dev/null || printf 'pending')
  case "$state" in
    passing) printf '[{{"name":"ci","state":"SUCCESS","bucket":"pass"}}]\n'; exit 0 ;;
    failing) printf '[{{"name":"ci","state":"FAILURE","bucket":"fail"}}]\n'; exit 1 ;;
    *) printf '[{{"name":"ci","state":"IN_PROGRESS","bucket":"pending"}}]\n'; exit 8 ;;
  esac
fi
if [ "$1" = "pr" ] && [ "$2" = "view" ]; then
  merged=$(cat '{merge}' 2>/dev/null || printf 'open')
  case "$merged" in
    merged) printf '{{"state":"MERGED","mergedAt":"2026-05-21T10:00:00Z","mergeCommit":{{"oid":"abc123"}}}}\n' ;;
    *) printf '{{"state":"OPEN","mergedAt":null,"mergeCommit":null}}\n' ;;
  esac
  exit 0
fi
exit 9
"#,
        log = log_path.display(),
        checks = checks_state_path.display(),
        merge = merge_state_path.display(),
    );
    write_fake_command(name, &body)
}

fn write_proposal_wt(name: &str, log_path: &Path) -> PathBuf {
    let body = format!(
        r#"#!/bin/sh
printf '%s\n' "$*" >> '{log}'
if [ "$1" = "--version" ]; then exit 0; fi
# switch --create: returns worktree path
if [ "$1" = "switch" ] && [ "$2" = "--create" ]; then
  branch="$3"
  git switch -c "$branch" >/dev/null 2>&1 || true
  printf '{{"path":"%s"}}\n' "$PWD"
  exit 0
fi
# remove: just succeed
if [ "$1" = "remove" ]; then exit 0; fi
exit 0
"#,
        log = log_path.display(),
    );
    write_fake_command(name, &body)
}

fn write_codex_ready() -> PathBuf {
    write_fake_command(
        "verify-codex-ready",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nlast=\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"--output-last-message\" ]; then shift; last=\"$1\"; fi\n  shift\ndone\nprintf '{\"outcome\":\"ReadyForProposal\",\"summary\":\"ok\"}\\n' > \"$last\"\n",
    )
}

async fn sync(app: &axum::Router, project: &ProjectResponse) {
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

async fn start_assignment(app: &axum::Router, project: &ProjectResponse) -> IssueAssignmentResponse {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/projects/{}/source-issues/21/assignment",
                    project.id.0
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

async fn refresh_proposal_state(
    app: &axum::Router,
    project: &ProjectResponse,
    assignment_id: &str,
) -> (StatusCode, Vec<u8>) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/projects/{}/assignments/{}/refresh-proposal-state",
                    project.id.0, assignment_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, bytes.to_vec())
}

async fn assignment_state(
    app: &axum::Router,
    project: &ProjectResponse,
) -> ProjectAssignmentStateResponse {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/projects/{}/assignment-state", project.id.0))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

fn setup_proposal(
    name: &str,
) -> (
    PathBuf, // project path
    PathBuf, // remote path
    PathBuf, // gh log
    PathBuf, // wt log
    PathBuf, // checks state file
    PathBuf, // merge state file
    PathBuf, // gh path
    PathBuf, // wt path
    PathBuf, // codex path
) {
    let (project_path, remote_path) = setup_git_project_with_remote(name);
    let gh_log = temp_path(&format!("{name}-gh-log"));
    let wt_log = temp_path(&format!("{name}-wt-log"));
    let checks_state = temp_path(&format!("{name}-checks-state"));
    let merge_state = temp_path(&format!("{name}-merge-state"));
    std::fs::write(&checks_state, "pending").unwrap();
    std::fs::write(&merge_state, "open").unwrap();
    let gh = write_proposal_gh(
        &format!("{name}-fake-gh"),
        &gh_log,
        &checks_state,
        &merge_state,
    );
    let wt = write_proposal_wt(&format!("{name}-fake-wt"), &wt_log);
    let codex = write_codex_ready();
    (
        project_path,
        remote_path,
        gh_log,
        wt_log,
        checks_state,
        merge_state,
        gh,
        wt,
        codex,
    )
}

#[tokio::test]
async fn pending_checks_keep_assignment_in_proposal_pending() {
    let (project_path, _remote, _gh_log, _wt_log, _checks, _merge, gh, wt, codex) =
        setup_proposal("pending");
    let (app, _db) = test_router(wt, codex, gh).await;
    let project = create_trusted_github_project(&app, &project_path).await;
    sync(&app, &project).await;
    let assignment = start_assignment(&app, &project).await;
    assert_eq!(assignment.status, "proposal_pending");

    let (status, body) = refresh_proposal_state(&app, &project, &assignment.id).await;
    assert_eq!(status, StatusCode::OK);
    let refreshed: IssueAssignmentResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(refreshed.status, "proposal_pending");
    let proposal = refreshed.change_proposal.unwrap();
    assert_eq!(proposal.status, "pending");
}

#[tokio::test]
async fn failing_checks_transition_assignment_to_blocked_with_detail() {
    let (project_path, _remote, _gh_log, _wt_log, checks, _merge, gh, wt, codex) =
        setup_proposal("failing");
    let (app, _db) = test_router(wt, codex, gh).await;
    let project = create_trusted_github_project(&app, &project_path).await;
    sync(&app, &project).await;
    let assignment = start_assignment(&app, &project).await;
    assert_eq!(assignment.status, "proposal_pending");

    std::fs::write(&checks, "failing").unwrap();
    let (status, body) = refresh_proposal_state(&app, &project, &assignment.id).await;
    assert_eq!(status, StatusCode::OK);
    let refreshed: IssueAssignmentResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(refreshed.status, "blocked");
    assert!(
        refreshed
            .status_detail
            .as_deref()
            .unwrap_or("")
            .to_lowercase()
            .contains("check"),
        "expected check detail, got {:?}",
        refreshed.status_detail
    );
}

#[tokio::test]
async fn passing_checks_mark_assignment_proposal_verified_and_release_slot() {
    let (project_path, _remote, _gh_log, _wt_log, checks, _merge, gh, wt, codex) =
        setup_proposal("passing");
    let (app, _db) = test_router(wt, codex, gh).await;
    let project = create_trusted_github_project(&app, &project_path).await;
    sync(&app, &project).await;
    let assignment = start_assignment(&app, &project).await;
    assert_eq!(assignment.status, "proposal_pending");

    std::fs::write(&checks, "passing").unwrap();
    let (status, body) = refresh_proposal_state(&app, &project, &assignment.id).await;
    assert_eq!(status, StatusCode::OK);
    let refreshed: IssueAssignmentResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(refreshed.status, "proposal_verified");
    let proposal = refreshed.change_proposal.unwrap();
    assert_eq!(proposal.status, "verified");

    // execution slot is released: a new assignment can be started.
    // But the verified assignment must still be visible in assignment-state.
    let state = assignment_state(&app, &project).await;
    assert_eq!(
        state.active_assignment.as_ref().map(|a| a.status.clone()),
        Some("proposal_verified".to_string())
    );
}

#[tokio::test]
async fn human_merge_writes_completed_and_cleans_up_assignment_worktree() {
    let (project_path, _remote, gh_log, wt_log, checks, merge, gh, wt, codex) =
        setup_proposal("merge");
    let (app, _db) = test_router(wt, codex, gh).await;
    let project = create_trusted_github_project(&app, &project_path).await;
    sync(&app, &project).await;
    let assignment = start_assignment(&app, &project).await;
    assert_eq!(assignment.status, "proposal_pending");

    // First make checks pass, then merge.
    std::fs::write(&checks, "passing").unwrap();
    std::fs::write(&merge, "merged").unwrap();

    let (status, body) = refresh_proposal_state(&app, &project, &assignment.id).await;
    assert_eq!(status, StatusCode::OK);
    let refreshed: IssueAssignmentResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(refreshed.status, "completed");
    assert_eq!(
        refreshed.change_proposal.as_ref().map(|p| p.status.clone()),
        Some("merged".to_string())
    );

    let gh_log_contents = std::fs::read_to_string(&gh_log).unwrap();
    assert!(
        gh_log_contents.contains("--add-label agentic-afk:completed"),
        "expected completed label write-back; log:\n{gh_log_contents}"
    );

    let wt_log_contents = std::fs::read_to_string(&wt_log).unwrap();
    assert!(
        wt_log_contents.contains("remove"),
        "expected worktree removal; log:\n{wt_log_contents}"
    );
}

#[tokio::test]
async fn verified_assignment_releases_execution_slot_for_a_new_eligible_start() {
    // After verification, the slot is released. We can directly observe this
    // by inserting a fresh assignment row into the database via persistence:
    // the project-level unique-active index must no longer block it.
    let (project_path, _remote, _gh_log, _wt_log, checks, _merge, gh, wt, codex) =
        setup_proposal("verified-releases-slot");
    let (app, db) = test_router(wt, codex, gh).await;
    let project = create_trusted_github_project(&app, &project_path).await;
    sync(&app, &project).await;
    let assignment = start_assignment(&app, &project).await;
    assert_eq!(assignment.status, "proposal_pending");

    std::fs::write(&checks, "passing").unwrap();
    let (status, _) = refresh_proposal_state(&app, &project, &assignment.id).await;
    assert_eq!(status, StatusCode::OK);

    // Directly attempt a second persistence-level claim for a different
    // source issue: the slot should be open because the verified assignment
    // no longer occupies the active-assignment unique index.
    use agentic_afk_contracts::{IssueSource, SourceIssueSnapshot};
    let source = IssueSource {
        kind: "github".to_string(),
        locator: "owner/repo".to_string(),
    };
    let issue = SourceIssueSnapshot {
        source_id: "99".to_string(),
        title: "Another".to_string(),
        readiness: "ready".to_string(),
        lifecycle_status: "ready".to_string(),
        parent_issue: None,
        issue_dependencies: vec![],
        source_order: 99,
        raw_text: String::new(),
    };
    let result = persistence::create_issue_assignment(
        &db,
        &project.id.0,
        &source,
        &issue,
        "agentic-afk/github-99",
    )
    .await;
    assert!(
        result.is_ok(),
        "verified assignment must release the execution slot, got: {:?}",
        result.err()
    );
}

#[tokio::test]
async fn refresh_proposal_state_rejects_unknown_assignment() {
    let (project_path, _remote, _gh_log, _wt_log, _checks, _merge, gh, wt, codex) =
        setup_proposal("unknown");
    let (app, _db) = test_router(wt, codex, gh).await;
    let project = create_trusted_github_project(&app, &project_path).await;
    let (status, body) =
        refresh_proposal_state(&app, &project, "nonexistent-assignment-id").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let problem: ProblemDetail = serde_json::from_slice(&body).unwrap();
    assert_eq!(problem.status, 404);
}

#[tokio::test]
async fn refresh_proposal_state_rejects_assignment_without_change_proposal() {
    let (project_path, _remote_path) = setup_git_project_with_remote("no-proposal");
    let gh_log = temp_path("no-proposal-gh-log");
    let checks_state = temp_path("no-proposal-checks-state");
    let merge_state = temp_path("no-proposal-merge-state");
    std::fs::write(&checks_state, "pending").unwrap();
    std::fs::write(&merge_state, "open").unwrap();
    let gh = write_proposal_gh("no-proposal-fake-gh", &gh_log, &checks_state, &merge_state);
    // Codex returns Blocked, so no proposal is created.
    let codex = write_fake_command(
        "no-proposal-fake-codex",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nlast=\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"--output-last-message\" ]; then shift; last=\"$1\"; fi\n  shift\ndone\nprintf '{\"outcome\":\"Blocked\",\"summary\":\"need input\"}\\n' > \"$last\"\n",
    );
    let wt_log = temp_path("no-proposal-wt-log");
    let wt = write_proposal_wt("no-proposal-fake-wt", &wt_log);
    let (app, _db) = test_router(wt, codex, gh).await;
    let project = create_trusted_github_project(&app, &project_path).await;
    sync(&app, &project).await;
    let assignment = start_assignment(&app, &project).await;
    assert_eq!(assignment.status, "blocked");
    assert!(assignment.change_proposal.is_none());

    let (status, body) = refresh_proposal_state(&app, &project, &assignment.id).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let problem: ProblemDetail = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        problem.problem_type,
        "urn:agentic-afk:assignment-has-no-change-proposal"
    );
}

#[tokio::test]
async fn refresh_proposal_state_is_idempotent_on_completed_assignment() {
    let (project_path, _remote, _gh_log, _wt_log, checks, merge, gh, wt, codex) =
        setup_proposal("idempotent");
    let (app, _db) = test_router(wt, codex, gh).await;
    let project = create_trusted_github_project(&app, &project_path).await;
    sync(&app, &project).await;
    let assignment = start_assignment(&app, &project).await;
    std::fs::write(&checks, "passing").unwrap();
    std::fs::write(&merge, "merged").unwrap();

    let (status, body) = refresh_proposal_state(&app, &project, &assignment.id).await;
    assert_eq!(status, StatusCode::OK);
    let first: IssueAssignmentResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(first.status, "completed");

    let (status, body) = refresh_proposal_state(&app, &project, &assignment.id).await;
    assert_eq!(status, StatusCode::OK);
    let second: IssueAssignmentResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(second.status, "completed");
}
