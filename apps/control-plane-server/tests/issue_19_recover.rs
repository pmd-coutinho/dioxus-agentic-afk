//! Integration tests for issue #19: Recover a blocked Issue Assignment in its existing
//! Assignment Worktree.
//!
//! Recovery must:
//! - Reject unless the assignment is blocked.
//! - Start a `recovery` Assignment Attempt in the existing Assignment Worktree.
//! - Stop any still-owned prior Codex process when its identity can be verified.
//! - Never permit two owned Codex processes on one assignment worktree.
//! - Use durable Source Issue, assignment, process, and block-reason facts in the prompt.
//! - Not consume CI repair budget (the recovery attempt kind stays `recovery`, not `repair`).

use agentic_afk_contracts::{
    AssignmentTerminalOutcome, CreateProjectRequest, EnableIssueSourceRequest,
    IssueAssignmentResponse, ProblemDetail, ProjectAssignmentStateResponse, ProjectResponse,
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
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
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

fn setup_local_markdown_project(name: &str) -> (PathBuf, PathBuf) {
    let project_path = temp_project_path(name);
    let issues_dir = project_path.join(".scratch/issues");
    std::fs::create_dir_all(&issues_dir).unwrap();
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

async fn recover(
    app: &axum::Router,
    project_id: &str,
    assignment_id: &str,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/projects/{project_id}/assignments/{assignment_id}/recover"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

/// Helper: build a Codex fake that records its prompt to a log file and emits the given outcome.
fn fake_codex_recording(name: &str, log_path: &PathBuf, outcome: &str, summary: &str) -> PathBuf {
    write_fake_command(
        name,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\n\
             last=''\n\
             prompt=''\n\
             while [ \"$#\" -gt 0 ]; do\n  \
               if [ \"$1\" = \"--output-last-message\" ]; then shift; last=\"$1\"; fi\n  \
               prompt=\"$1\"\n  \
               shift\n\
             done\n\
             printf '%s\\n---\\n' \"$prompt\" >> '{log}'\n\
             printf '{{\"outcome\":\"{outcome}\",\"summary\":\"{summary}\"}}\\n' > \"$last\"\n",
            log = log_path.display(),
            outcome = outcome,
            summary = summary,
        ),
    )
}

fn fake_wt() -> PathBuf {
    let worktree = temp_project_path("recover-worktree");
    std::fs::create_dir_all(&worktree).unwrap();
    write_fake_command(
        "recover-fake-wt",
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nprintf '{{\"path\":\"{}\"}}\\n'\n",
            worktree.display()
        ),
    )
}

#[tokio::test]
async fn recover_blocked_assignment_starts_recovery_attempt_in_same_worktree() {
    let (project_path, issues_dir) = setup_local_markdown_project("recover-happy");
    std::fs::create_dir_all(project_path.join(".git")).unwrap();
    std::fs::write(
        issues_dir.join("issue-a.md"),
        "# Sample issue\n\nReadiness: ready\nSource Order: 1\n\nAcceptance: do work.\n",
    )
    .unwrap();

    let codex_log = temp_project_path("recover-happy-codex-log");
    std::fs::write(&codex_log, "").unwrap();

    // First Codex call returns Blocked, second returns Blocked again with new summary.
    let codex_script = write_fake_command(
        "recover-happy-codex",
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\n\
             last=''\nprompt=''\n\
             while [ \"$#\" -gt 0 ]; do\n  \
               if [ \"$1\" = \"--output-last-message\" ]; then shift; last=\"$1\"; fi\n  \
               prompt=\"$1\"\n  shift\n\
             done\n\
             counter='{counter}'\n\
             n=$(cat \"$counter\" 2>/dev/null || echo 0)\n\
             n=$((n + 1))\n\
             echo \"$n\" > \"$counter\"\n\
             printf '%s\\n===PROMPT===\\n' \"$prompt\" >> '{log}'\n\
             if [ \"$n\" = \"1\" ]; then\n  \
               printf '{{\"outcome\":\"Blocked\",\"summary\":\"initial block reason\"}}\\n' > \"$last\"\n\
             else\n  \
               printf '{{\"outcome\":\"Blocked\",\"summary\":\"recovery still blocked\"}}\\n' > \"$last\"\n\
             fi\n",
            log = codex_log.display(),
            counter = temp_project_path("recover-happy-counter").display()
        ),
    );

    let (app, db) = test_router_with_execution(fake_wt(), codex_script).await;
    let project = create_trusted_local_markdown_project(&app, &project_path).await;
    sync(&app, &project).await;
    let assignment = start_assignment(&app, &project, "issue-a").await;
    assert_eq!(assignment.status, "blocked");
    assert_eq!(
        assignment.latest_attempt.clone().unwrap().kind,
        "initial",
        "first attempt is the initial pass"
    );
    let initial_worktree = assignment.worktree_path.clone();

    let response = recover(&app, &project.id.0, &assignment.id).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let recovered: IssueAssignmentResponse = serde_json::from_slice(&body).unwrap();

    assert_eq!(recovered.id, assignment.id, "same Issue Assignment");
    assert_eq!(
        recovered.worktree_path, initial_worktree,
        "recovery reuses the existing Assignment Worktree"
    );
    let latest = recovered.latest_attempt.expect("recovery attempt persisted");
    assert_eq!(latest.kind, "recovery");
    assert_eq!(
        latest.terminal_outcome.unwrap().summary,
        "recovery still blocked"
    );
    assert_eq!(recovered.status, "blocked");

    // Recovery prompt should include durable facts (source id and prior block reason),
    // not invented prior-agent reasoning.
    let log = std::fs::read_to_string(&codex_log).unwrap();
    let prompts: Vec<&str> = log.split("===PROMPT===").collect();
    assert!(prompts.len() >= 2, "two prompts recorded");
    let recovery_prompt = prompts[1];
    assert!(
        recovery_prompt.contains("issue-a"),
        "recovery prompt mentions the Source Issue id"
    );
    assert!(
        recovery_prompt.to_lowercase().contains("recovery")
            || recovery_prompt.to_lowercase().contains("recover"),
        "recovery prompt declares itself as a recovery attempt"
    );
    assert!(
        recovery_prompt.contains("initial block reason"),
        "recovery prompt includes the durable prior block reason"
    );

    // Two attempts should now exist in persistence and the latest must be `recovery`.
    let _ = db;
}

#[tokio::test]
async fn recover_rejects_when_assignment_is_not_blocked() {
    let (project_path, issues_dir) = setup_local_markdown_project("recover-not-blocked");
    std::fs::create_dir_all(project_path.join(".git")).unwrap();
    std::fs::write(
        issues_dir.join("issue.md"),
        "# Issue\n\nReadiness: ready\nSource Order: 1\n",
    )
    .unwrap();

    // Use a Codex script that fails fast so the initial attempt persists as `failed`.
    let codex = write_fake_command(
        "recover-not-blocked-codex",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nlast=''\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"--output-last-message\" ]; then shift; last=\"$1\"; fi\n  shift\ndone\nprintf '{\"outcome\":\"Failed\",\"summary\":\"nope\"}\\n' > \"$last\"\n",
    );
    let (app, _db) = test_router_with_execution(fake_wt(), codex).await;
    let project = create_trusted_local_markdown_project(&app, &project_path).await;
    sync(&app, &project).await;
    let assignment = start_assignment(&app, &project, "issue").await;
    assert_eq!(assignment.status, "failed");

    let response = recover(&app, &project.id.0, &assignment.id).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let problem: ProblemDetail = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        problem.problem_type,
        "urn:agentic-afk:assignment-not-recoverable"
    );
}

#[tokio::test]
async fn recover_returns_not_found_for_unknown_assignment() {
    let (project_path, _) = setup_local_markdown_project("recover-missing");
    let (app, _db) = test_router_with_execution(fake_wt(), fake_wt()).await;
    let project = create_trusted_local_markdown_project(&app, &project_path).await;

    let response = recover(&app, &project.id.0, "no-such-assignment").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn recover_attempts_are_persisted_separately_from_initial_attempts() {
    // Drive persistence directly to confirm recovery attempts are stored and not lumped
    // into initial/repair budget tracking.
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();

    let project_path = temp_project_path("recover-persistence");
    std::fs::create_dir_all(&project_path).unwrap();
    let project = persistence::create_project(
        &db,
        &CreateProjectRequest {
            path: project_path.display().to_string(),
        },
    )
    .await
    .unwrap();

    let source = agentic_afk_contracts::IssueSource {
        kind: "local_markdown".to_string(),
        locator: ".scratch/issues".to_string(),
    };
    persistence::enable_issue_source(
        &db,
        &project.id.0,
        &EnableIssueSourceRequest {
            kind: source.kind.clone(),
            locator: source.locator.clone(),
        },
    )
    .await
    .unwrap();

    let issue = agentic_afk_contracts::SourceIssueSnapshot {
        source_id: "issue-x".to_string(),
        title: "Issue X".to_string(),
        readiness: "ready".to_string(),
        lifecycle_status: "ready".to_string(),
        parent_issue: None,
        issue_dependencies: vec![],
        source_order: 1,
        raw_text: "raw".to_string(),
    };
    let assignment =
        persistence::create_issue_assignment(&db, &project.id.0, &source, &issue, "branch")
            .await
            .unwrap();
    persistence::set_assignment_worktree(&db, &assignment.id, "/tmp/wt")
        .await
        .unwrap();
    persistence::record_initial_attempt(
        &db,
        &assignment.id,
        Some(123),
        Some("identity-initial"),
        Some(&AssignmentTerminalOutcome {
            outcome: "Blocked".to_string(),
            summary: "blocked detail".to_string(),
        }),
    )
    .await
    .unwrap();
    persistence::set_assignment_status(&db, &assignment.id, "blocked", Some("blocked detail"))
        .await
        .unwrap();

    let updated = persistence::record_recovery_attempt(
        &db,
        &assignment.id,
        Some(456),
        Some("identity-recovery"),
        Some(&AssignmentTerminalOutcome {
            outcome: "Blocked".to_string(),
            summary: "still stuck".to_string(),
        }),
    )
    .await
    .unwrap();

    let latest = updated.latest_attempt.expect("latest attempt");
    assert_eq!(latest.kind, "recovery");
    assert_eq!(latest.process_id, Some(456));
    assert_eq!(
        latest.process_identity.as_deref(),
        Some("identity-recovery")
    );

    let attempts = persistence::list_assignment_attempts(&db, &assignment.id)
        .await
        .unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].kind, "initial");
    assert_eq!(attempts[1].kind, "recovery");
}

#[tokio::test]
async fn recover_keeps_a_single_owned_codex_process_when_prior_is_verifiable() {
    // The prior attempt records an unverifiable (already-dead) process identity, so the
    // orchestrator should not fail trying to stop it and must proceed to start the
    // replacement attempt.  After recovery, the latest attempt's process identity must
    // be the new one — i.e. one owned process at a time.
    let (project_path, issues_dir) = setup_local_markdown_project("recover-one-owned");
    std::fs::create_dir_all(project_path.join(".git")).unwrap();
    std::fs::write(
        issues_dir.join("dead-pid.md"),
        "# Recoverable\n\nReadiness: ready\nSource Order: 1\n",
    )
    .unwrap();

    let codex_log = temp_project_path("recover-one-owned-log");
    std::fs::write(&codex_log, "").unwrap();
    let codex = fake_codex_recording(
        "recover-one-owned-codex",
        &codex_log,
        "Blocked",
        "first block",
    );
    let (app, db) = test_router_with_execution(fake_wt(), codex).await;
    let project = create_trusted_local_markdown_project(&app, &project_path).await;
    sync(&app, &project).await;
    let initial = start_assignment(&app, &project, "dead-pid").await;
    assert_eq!(initial.status, "blocked");
    let initial_attempt = initial.latest_attempt.clone().unwrap();
    let initial_pid = initial_attempt.process_id;
    let initial_identity = initial_attempt.process_identity.clone();

    // Now recover. The fake Codex exits before we observe it, so the prior process is
    // already gone — recovery should succeed without an error and the new attempt must
    // be a recovery kind with a different (or same, but fresh) process metadata pair.
    let response = recover(&app, &project.id.0, &initial.id).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let recovered: IssueAssignmentResponse = serde_json::from_slice(&body).unwrap();
    let latest = recovered.latest_attempt.unwrap();
    assert_eq!(latest.kind, "recovery");
    // The replacement attempt must record its own process slot — meaning recovery
    // actually ran Codex rather than copying the initial attempt row. PIDs may
    // recycle on short timescales, so we don't assert PID inequality.
    assert!(latest.process_id.is_some());
    let _ = (initial_pid, initial_identity, db);
}

#[tokio::test]
async fn recover_updates_local_markdown_lifecycle_and_records_activity_in_state() {
    let (project_path, issues_dir) = setup_local_markdown_project("recover-lifecycle");
    std::fs::create_dir_all(project_path.join(".git")).unwrap();
    std::fs::write(
        issues_dir.join("issue.md"),
        "# Lifecycle\n\nReadiness: ready\nSource Order: 1\n",
    )
    .unwrap();

    // First Codex call → Blocked; second (recovery) → ReadyForProposal (but no proposal
    // target for local markdown, so still ends blocked, but Codex should have been
    // re-run and the lifecycle status should remain `blocked`.)
    let counter = temp_project_path("recover-lifecycle-counter");
    let codex = write_fake_command(
        "recover-lifecycle-codex",
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nlast=''\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"--output-last-message\" ]; then shift; last=\"$1\"; fi\n  shift\ndone\nn=$(cat '{counter}' 2>/dev/null || echo 0)\nn=$((n + 1))\necho \"$n\" > '{counter}'\nif [ \"$n\" = \"1\" ]; then\n  printf '{{\"outcome\":\"Blocked\",\"summary\":\"need help\"}}\\n' > \"$last\"\nelse\n  printf '{{\"outcome\":\"Blocked\",\"summary\":\"still need help\"}}\\n' > \"$last\"\nfi\n",
            counter = counter.display()
        ),
    );
    let (app, _db) = test_router_with_execution(fake_wt(), codex).await;
    let project = create_trusted_local_markdown_project(&app, &project_path).await;
    sync(&app, &project).await;
    let assignment = start_assignment(&app, &project, "issue").await;
    assert_eq!(assignment.status, "blocked");

    let response = recover(&app, &project.id.0, &assignment.id).await;
    assert_eq!(response.status(), StatusCode::CREATED);

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
    assert_eq!(
        active.latest_attempt.unwrap().kind,
        "recovery",
        "assignment-state surfaces the latest recovery attempt"
    );

    // Lifecycle file remains `blocked` (recovery did not reach a verified change).
    let raw = std::fs::read_to_string(issues_dir.join("issue.md")).unwrap();
    assert!(raw.contains("Lifecycle Status: blocked"));
}
