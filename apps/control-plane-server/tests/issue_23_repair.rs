//! Integration tests for GitHub Change Proposal Repair Loop (issue #23).
//!
//! Each test exercises the HTTP API of the Local Control Plane through
//! fake `gh`, `wt` (Worktrunk), and `codex` binaries so the Repair Loop
//! behavior can be observed without live external services.

use agentic_afk_contracts::{
    CreateProjectRequest, EnableIssueSourceRequest, FailedCheckFact, IssueAssignmentResponse,
    IssueSource, ProblemDetail, RepairAssignmentRequest, SourceIssueSnapshot,
};
use agentic_afk_control_plane_server::{ControlPlaneConfig, router};
use agentic_afk_persistence as persistence;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

static UNIQUE: AtomicUsize = AtomicUsize::new(0);

fn unique_path(name: &str) -> PathBuf {
    let n = UNIQUE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "agentic-afk-i23-{name}-{}-{n}",
        std::process::id()
    ))
}

fn write_fake_command(name: &str, body: &str) -> PathBuf {
    let path = unique_path(name);
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        let mut perm = std::fs::metadata(&path).unwrap().permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&path, perm).unwrap();
    }
    path
}

struct Harness {
    app: axum::Router,
    db: persistence::Db,
}

async fn setup_harness(codex_outcome_json: &str) -> Harness {
    let fake_gh = write_fake_command(
        "fake-gh",
        "#!/bin/sh\nif [ \"$1\" = \"auth\" ]; then exit 0; fi\nexit 0\n",
    );
    let fake_wt = write_fake_command(
        "fake-wt",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nexit 0\n",
    );
    let fake_codex = write_fake_command(
        "fake-codex",
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nlast=''\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"--output-last-message\" ]; then shift; last=\"$1\"; fi\n  shift\ndone\nprintf '%s' '{codex_outcome_json}' > \"$last\"\n",
            codex_outcome_json = codex_outcome_json
        ),
    );
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "apps/dashboard/dist".into(),
        database_url: "sqlite::memory:".into(),
        gh_binary_path: fake_gh,
        worktrunk_binary_path: fake_wt,
        codex_binary_path: fake_codex,
    };
    Harness {
        app: router(config, db.clone()),
        db,
    }
}

/// Create a Project + GitHub issue source row, then seed one Issue Assignment
/// in `running` status with a pending Change Proposal. Returns assignment id
/// and project id.
async fn seed_assignment_with_proposal(
    harness: &Harness,
    project_path: &Path,
    worktree_path: &Path,
) -> (String, String) {
    std::fs::create_dir_all(project_path).unwrap();
    std::fs::create_dir_all(worktree_path).unwrap();
    let project = persistence::create_project(
        &harness.db,
        &CreateProjectRequest {
            path: project_path.display().to_string(),
        },
    )
    .await
    .unwrap();
    persistence::trust_project(&harness.db, &project.id.0)
        .await
        .unwrap();
    persistence::enable_issue_source(
        &harness.db,
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
        title: "Failing checks".to_string(),
        readiness: "ready".to_string(),
        lifecycle_status: "running".to_string(),
        parent_issue: None,
        issue_dependencies: vec![],
        source_order: 1,
        raw_text: "# Failing checks\n\nFix the CI.".to_string(),
    };
    let assignment = persistence::create_issue_assignment(
        &harness.db,
        &project.id.0,
        &source,
        &issue,
        "agentic-afk/github-21",
    )
    .await
    .unwrap();
    let assignment = persistence::set_assignment_worktree(
        &harness.db,
        &assignment.id,
        &worktree_path.display().to_string(),
    )
    .await
    .unwrap();
    let assignment =
        persistence::set_assignment_status(&harness.db, &assignment.id, "running", None)
            .await
            .unwrap();
    persistence::set_assignment_change_proposal(
        &harness.db,
        &assignment.id,
        "failed",
        "https://github.com/owner/repo/pull/42",
    )
    .await
    .unwrap();
    (project.id.0, assignment.id)
}

async fn post_repair(
    app: &axum::Router,
    project_id: &str,
    assignment_id: &str,
    request: &RepairAssignmentRequest,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/projects/{project_id}/assignments/{assignment_id}/repair"
                ))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn repair_rejects_assignment_without_change_proposal() {
    let harness = setup_harness("{\"outcome\":\"Blocked\",\"summary\":\"x\"}").await;
    let project_path = unique_path("no-proposal-project");
    let worktree_path = unique_path("no-proposal-worktree");
    std::fs::create_dir_all(&project_path).unwrap();
    std::fs::create_dir_all(&worktree_path).unwrap();
    let project = persistence::create_project(
        &harness.db,
        &CreateProjectRequest {
            path: project_path.display().to_string(),
        },
    )
    .await
    .unwrap();
    persistence::enable_issue_source(
        &harness.db,
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
        title: "No proposal".to_string(),
        readiness: "ready".to_string(),
        lifecycle_status: "running".to_string(),
        parent_issue: None,
        issue_dependencies: vec![],
        source_order: 1,
        raw_text: "body".to_string(),
    };
    let assignment = persistence::create_issue_assignment(
        &harness.db,
        &project.id.0,
        &source,
        &issue,
        "agentic-afk/github-21",
    )
    .await
    .unwrap();

    let response = post_repair(
        &harness.app,
        &project.id.0,
        &assignment.id,
        &RepairAssignmentRequest::default(),
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let problem: ProblemDetail = serde_json::from_slice(&body).unwrap();
    assert_eq!(problem.problem_type, "urn:agentic-afk:no-change-proposal");
}

#[tokio::test]
async fn repair_records_repair_attempt_with_failed_check_facts_in_prompt() {
    let prompt_log = unique_path("repair-prompt-log");
    let fake_gh = write_fake_command(
        "repair-prompt-gh",
        "#!/bin/sh\nexit 0\n",
    );
    let fake_wt = write_fake_command(
        "repair-prompt-wt",
        "#!/bin/sh\nexit 0\n",
    );
    let fake_codex = write_fake_command(
        "repair-prompt-codex",
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nlast=''\nprompt=''\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"--output-last-message\" ]; then shift; last=\"$1\"; elif [ \"$1\" = \"exec\" ] || [ \"$1\" = \"--dangerously-bypass-approvals-and-sandbox\" ] || [ \"$1\" = \"--output-schema\" ]; then :; else prompt=\"$1\"; fi\n  shift\ndone\nprintf '%s' \"$prompt\" > '{}'\nprintf '{{\"outcome\":\"Blocked\",\"summary\":\"still failing\"}}' > \"$last\"\n",
            prompt_log.display()
        ),
    );
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "apps/dashboard/dist".into(),
        database_url: "sqlite::memory:".into(),
        gh_binary_path: fake_gh,
        worktrunk_binary_path: fake_wt,
        codex_binary_path: fake_codex,
    };
    let harness = Harness {
        app: router(config, db.clone()),
        db,
    };
    let project_path = unique_path("repair-prompt-project");
    let worktree_path = unique_path("repair-prompt-worktree");
    let (project_id, assignment_id) =
        seed_assignment_with_proposal(&harness, &project_path, &worktree_path).await;

    let request = RepairAssignmentRequest {
        failed_checks: vec![FailedCheckFact {
            name: "ci/lint".to_string(),
            url: Some("https://github.com/owner/repo/actions/runs/1".to_string()),
            summary: Some("clippy".to_string()),
        }],
        verified_worktree_facts: Some("tests passed locally".to_string()),
    };
    let response = post_repair(&harness.app, &project_id, &assignment_id, &request).await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let assignment: IssueAssignmentResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(assignment.status, "blocked");
    let attempt = assignment.latest_attempt.unwrap();
    assert_eq!(attempt.kind, "repair");
    assert_eq!(attempt.terminal_outcome.unwrap().outcome, "Blocked");
    let budget = assignment.repair_budget.unwrap();
    assert_eq!(budget.attempt_count, 1);
    assert!(budget.window_started_at.is_some());

    let prompt = std::fs::read_to_string(&prompt_log).unwrap();
    assert!(prompt.contains("Change Proposal: https://github.com/owner/repo/pull/42"));
    assert!(prompt.contains("Assignment branch: agentic-afk/github-21"));
    assert!(prompt.contains("ci/lint"));
    assert!(prompt.contains("clippy"));
    assert!(prompt.contains("tests passed locally"));
    assert!(prompt.contains("# Failing checks"));
}

#[tokio::test]
async fn repair_ready_for_proposal_marks_proposal_pending_again() {
    let harness = setup_harness("{\"outcome\":\"ReadyForProposal\",\"summary\":\"fixed\"}").await;
    let project_path = unique_path("repair-pending-project");
    let worktree_path = unique_path("repair-pending-worktree");
    let (project_id, assignment_id) =
        seed_assignment_with_proposal(&harness, &project_path, &worktree_path).await;

    let response = post_repair(
        &harness.app,
        &project_id,
        &assignment_id,
        &RepairAssignmentRequest::default(),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let assignment: IssueAssignmentResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(assignment.status, "proposal_pending");
    let proposal = assignment.change_proposal.unwrap();
    assert_eq!(proposal.status, "pending");
    assert_eq!(proposal.url, "https://github.com/owner/repo/pull/42");
}

#[tokio::test]
async fn repair_budget_blocks_assignment_with_preserved_worktree_when_attempts_exhausted() {
    let harness = setup_harness("{\"outcome\":\"Blocked\",\"summary\":\"nope\"}").await;
    let project_path = unique_path("repair-exhausted-project");
    let worktree_path = unique_path("repair-exhausted-worktree");
    let (project_id, assignment_id) =
        seed_assignment_with_proposal(&harness, &project_path, &worktree_path).await;

    // Three repair attempts is the default budget.
    for _ in 0..3 {
        let response = post_repair(
            &harness.app,
            &project_id,
            &assignment_id,
            &RepairAssignmentRequest::default(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    let response = post_repair(
        &harness.app,
        &project_id,
        &assignment_id,
        &RepairAssignmentRequest::default(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let assignment: IssueAssignmentResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(assignment.status, "blocked");
    assert!(
        assignment
            .status_detail
            .as_deref()
            .unwrap()
            .contains("repair budget exhausted")
    );
    // Assignment Worktree preserved for recovery or abandonment.
    assert_eq!(assignment.worktree_path, worktree_path.display().to_string());
    // Change Proposal still attached so the operator can keep reviewing the
    // failed proposal while choosing recovery or abandonment.
    assert!(assignment.change_proposal.is_some());
    let budget = assignment.repair_budget.unwrap();
    assert_eq!(budget.attempt_count, budget.max_attempts);
}

#[tokio::test]
async fn recovery_attempts_do_not_count_against_repair_budget() {
    let harness = setup_harness("{\"outcome\":\"Blocked\",\"summary\":\"x\"}").await;
    let project_path = unique_path("recovery-budget-project");
    let worktree_path = unique_path("recovery-budget-worktree");
    let (_project_id, assignment_id) =
        seed_assignment_with_proposal(&harness, &project_path, &worktree_path).await;

    let outcome = agentic_afk_contracts::AssignmentTerminalOutcome {
        outcome: "Blocked".to_string(),
        summary: "still blocked".to_string(),
    };
    for _ in 0..5 {
        persistence::repair::record_recovery_attempt(
            &harness.db,
            &assignment_id,
            Some(1),
            None,
            Some(&outcome),
        )
        .await
        .unwrap();
    }
    let assignment = persistence::get_assignment(&harness.db, &assignment_id)
        .await
        .unwrap();
    let budget = assignment.repair_budget.unwrap();
    assert_eq!(budget.attempt_count, 0);
    assert!(budget.window_started_at.is_none());
    assert_eq!(assignment.latest_attempt.unwrap().kind, "recovery");
}

#[tokio::test]
async fn repair_endpoint_requires_existing_assignment() {
    let harness = setup_harness("{\"outcome\":\"Blocked\",\"summary\":\"x\"}").await;
    let project_path = unique_path("repair-missing-project");
    std::fs::create_dir_all(&project_path).unwrap();
    let project = persistence::create_project(
        &harness.db,
        &CreateProjectRequest {
            path: project_path.display().to_string(),
        },
    )
    .await
    .unwrap();

    let response = post_repair(
        &harness.app,
        &project.id.0,
        "missing-assignment-id",
        &RepairAssignmentRequest::default(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
