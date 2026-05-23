//! Issue #57 / ADR-0037 / ADR-0039: Push failure recovery test cluster.
//!
//! This file is the canonical home for the operator-action recovery
//! scenarios on a `merge_staged` Issue Assignment:
//!
//! - Merge Phase `merge_staged` transition before the Integration Branch push.
//! - Retry Push success: `merge_staged` -> `merged` + Lifecycle `Completed`
//!   write-back + worktree cleanup.
//! - Retry Push non-fast-forward: `merge_staged` -> `blocked`
//!   (`BlockReason::PushNonFastForward`).
//! - Abandon Staged: `merge_staged` -> `blocked`
//!   (`BlockReason::AbandonedStaged`).
//! - Cleanup gating: staged assignments retain their worktree until they
//!   reach a terminal status.
//! - Plan Run terminal status preservation: a failed Plan Run stays
//!   `failed` even after a successful Retry Push (ADR-0037).
//!
//! Detailed unit-style coverage of these transitions, including the
//! `ProgrammablePusher` fault-injection harness, lives in
//! `plan_run_merge.rs` (see the `// --- Issue #53 / ADR-0037: Retry Push
//! integration tests ---` section). This file adds a higher-level
//! end-to-end smoke that drives a staged assignment through the public
//! HTTP API from a real push failure to a successful Retry Push, asserting
//! the operator-observable wire contract in one round-trip — the integration
//! tier policy (ADR-0039) is HTTP + Fakes, so the smoke uses the same
//! router and `Fake*` seams as the rest of the `plan_run_*` cluster.

use agentic_afk_contracts::{
    CreateProjectRequest, EnableIssueSourceRequest, IssueSource, ProjectResponse,
    RetryPushResponse, SetProjectExecutionConfigRequest, SourceIssueSnapshot,
};
use agentic_afk_control_plane_server::{
    AssignmentWorktreeCleaner, AssignmentWorktreeProvisioner, ControlPlaneConfig,
    FakeAssignmentWorktreeCleaner, FakeImplementationPhaseRunner, FakeLifecycleWriter,
    FakeMergePhaseRunner, FakePlanningPhaseRunner, FakeReviewPhaseRunner, FakeWorktreeProvisioner,
    ImplementationPhaseRunner, IntegrationBranchPusher, IssueLifecycleWriter, MergePhaseRunner,
    RefreshedBaseline, ReviewPhaseRunner, StaticIntegrationBranchRefresher,
    router_with_plan_run_merge_deps,
};
use agentic_afk_orchestrator::PlanRunPhaseError;
use agentic_afk_persistence as persistence;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde::de::DeserializeOwned;
use std::path::{Path as StdPath, PathBuf};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

fn temp_dir(label: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "agentic-afk-push-failure-{label}-{}-{nonce}",
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

fn config() -> ControlPlaneConfig {
    ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "target/dx/agentic-afk-dashboard/release/web/public".into(),
        database_url: "sqlite::memory:".into(),
        gh_binary_path: "gh".into(),
        worktrunk_binary_path: "wt".into(),
        codex_binary_path: "codex".into(),
    }
}

fn issue(source_id: &str) -> SourceIssueSnapshot {
    SourceIssueSnapshot {
        source_id: source_id.into(),
        title: format!("Issue {source_id}"),
        readiness: "ready".into(),
        lifecycle_status: "ready".into(),
        parent_issue: None,
        issue_dependencies: vec![],
        source_order: 0,
        raw_text: format!("brief for {source_id}"),
    }
}

const IMPL_OK: &str = r#"<impl>{"outcome":"ready_for_review","summary":"shipped","commits":["abc"],"verification":["cargo test"],"gaps":[]}</impl>"#;
const REVIEW_APPROVED: &str = r#"<review>{"outcome":"approved","findings":[],"summary":"lgtm","verification":["cargo test"],"gaps":[]}</review>"#;
const MERGE_OK: &str = r#"<merge>{"outcome":"merged","summary":"integrated cleanly","merged_source_ids":["42"],"verification":["cargo test --workspace"],"gaps":[]}</merge>"#;

/// Pusher whose responses are scripted as a `Vec<Result<...>>`. Re-played
/// in order; the final entry repeats so unexpected extra pushes do not
/// panic. Mirrors the harness in `plan_run_merge.rs` so the push-failure
/// cluster has an independent local copy for clarity.
struct ScriptedPusher {
    responses: Mutex<Vec<Result<(), PlanRunPhaseError>>>,
    calls: Mutex<Vec<String>>,
}

impl ScriptedPusher {
    fn new(responses: Vec<Result<(), PlanRunPhaseError>>) -> Self {
        assert!(!responses.is_empty(), "ScriptedPusher needs at least one response");
        Self {
            responses: Mutex::new(responses),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

impl IntegrationBranchPusher for ScriptedPusher {
    fn push(
        &self,
        _project_path: &StdPath,
        integration_branch: &str,
    ) -> Result<(), PlanRunPhaseError> {
        self.calls
            .lock()
            .unwrap()
            .push(integration_branch.to_string());
        let mut queue = self.responses.lock().unwrap();
        if queue.len() == 1 {
            queue[0].clone()
        } else {
            queue.remove(0)
        }
    }
}

async fn build_project(
    router: &axum::Router,
    db: &persistence::Db,
    project_dir: &StdPath,
) -> String {
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
    let pid = project.id.0.clone();
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/projects/{pid}/trust"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    persistence::enable_issue_source(
        db,
        &pid,
        &EnableIssueSourceRequest {
            kind: "github".into(),
            locator: "owner/repo".into(),
        },
    )
    .await
    .unwrap();
    persistence::replace_planning_snapshot(
        db,
        &pid,
        &IssueSource {
            kind: "github".into(),
            locator: "owner/repo".into(),
        },
        &[issue("42")],
        "unix:1",
    )
    .await
    .unwrap();
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/projects/{pid}/execution-config"))
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
    pid
}

async fn start_plan_run(router: &axum::Router, pid: &str) -> axum::response::Response {
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/projects/{pid}/plan-runs"))
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn push_failure_then_retry_push_succeeds_round_trip_smoke() {
    // ADR-0037 / ADR-0039 acceptance: a Plan Run whose Integration Branch
    // push fails on the first attempt leaves the Issue Assignment at
    // `merge_staged` with a failed `push` Phase Output recorded on the
    // Plan Run. The operator's Retry Push then succeeds: the assignment
    // flips to `merged`, Lifecycle `Completed` is written back, the
    // worktree is cleaned up, and the Plan Run terminal status stays
    // `failed` (per ADR-0037 — the original failure is preserved in
    // history). This is the end-to-end happy recovery smoke; the unit-
    // level transition matrix lives in `plan_run_merge.rs`.

    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();

    let pusher = Arc::new(ScriptedPusher::new(vec![
        Err(PlanRunPhaseError::IntegrationPush(
            "ssh: connection reset by peer".into(),
        )),
        Ok(()),
    ]));
    let cleaner = Arc::new(FakeAssignmentWorktreeCleaner::new());
    let lifecycle = Arc::new(FakeLifecycleWriter::new());

    let router = router_with_plan_run_merge_deps(
        config(),
        db.clone(),
        Arc::new(StaticIntegrationBranchRefresher::new(RefreshedBaseline {
            commit_sha: "baseline-sha".into(),
        })),
        Arc::new(FakePlanningPhaseRunner::with_stdout(
            r#"<plan>{"issues":[{"source_issue_id":"42","title":"t","branch":"agent/issue-42","selection_summary":"ok"}],"summary":"s"}</plan>"#,
        )),
        Arc::new(FakeWorktreeProvisioner::new(std::env::temp_dir()))
            as Arc<dyn AssignmentWorktreeProvisioner>,
        lifecycle.clone() as Arc<dyn IssueLifecycleWriter>,
        Arc::new(FakeImplementationPhaseRunner::with_stdout(IMPL_OK))
            as Arc<dyn ImplementationPhaseRunner>,
        Arc::new(FakeReviewPhaseRunner::with_stdout(REVIEW_APPROVED))
            as Arc<dyn ReviewPhaseRunner>,
        Arc::new(FakeMergePhaseRunner::with_stdout(MERGE_OK)) as Arc<dyn MergePhaseRunner>,
        pusher.clone() as Arc<dyn IntegrationBranchPusher>,
        cleaner.clone() as Arc<dyn AssignmentWorktreeCleaner>,
    );

    let project_dir = temp_dir("smoke");
    std::fs::create_dir_all(&project_dir).unwrap();
    let pid = build_project(&router, &db, &project_dir).await;

    // First Plan Run: push fails, assignment stages.
    let resp = start_plan_run(&router, &pid).await;
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "first plan run finished gracefully: {}",
        read_text(resp).await
    );
    let runs = persistence::list_recent_plan_runs(&db, &pid, 5).await.unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].state, "failed", "push failure fails the Plan Run");
    let assignment_id = runs[0].assignments[0].id.clone();
    assert_eq!(
        runs[0].assignments[0].status, "merge_staged",
        "assignment must stage on push failure"
    );
    assert_eq!(cleaner.call_count(), 0, "staged worktree retained");

    // Retry Push via the operator-facing route.
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/projects/{pid}/assignments/{assignment_id}/retry-push"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let outcome: RetryPushResponse = read_json(resp).await;
    assert_eq!(outcome.status, "merged");
    assert!(outcome.block_reason.is_none());

    // Pusher invoked twice (first failure + successful retry).
    assert_eq!(pusher.call_count(), 2);

    // Assignment now terminal: cleanup ran, lifecycle Completed written.
    let after = persistence::get_assignment(&db, &assignment_id).await.unwrap();
    assert_eq!(after.status, "merged");
    assert_eq!(cleaner.call_count(), 1, "worktree cleaned at terminal status");
    let completed: Vec<_> = lifecycle
        .calls()
        .into_iter()
        .filter(|(_, status)| {
            matches!(
                status,
                agentic_afk_orchestrator::LifecycleStatus::Completed
            )
        })
        .collect();
    assert_eq!(
        completed.len(),
        1,
        "successful Retry Push writes Lifecycle Completed once"
    );

    // Plan Run terminal status preserved (ADR-0037).
    let runs = persistence::list_recent_plan_runs(&db, &pid, 5).await.unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(
        runs[0].state, "failed",
        "Plan Run stays failed even after successful Retry Push"
    );

    // Two `push` Phase Outputs on the Plan Run: first failed, then succeeded.
    let push_outputs: Vec<_> = runs[0]
        .phase_outputs
        .iter()
        .filter(|p| p.phase == "push")
        .collect();
    assert_eq!(push_outputs.len(), 2);
    assert_eq!(push_outputs[0].outcome, "failed");
    assert_eq!(push_outputs[1].outcome, "succeeded");

    drop(project_dir);
}

#[tokio::test]
async fn abandon_staged_round_trip_smoke() {
    // ADR-0037 acceptance: Abandon Staged transitions a `merge_staged`
    // assignment to `blocked` with `BlockReason::AbandonedStaged`, runs
    // cleanup once (terminal status reached), and does not invoke the
    // pusher. Plan Run terminal status remains `failed`.
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();

    let pusher = Arc::new(ScriptedPusher::new(vec![Err(
        PlanRunPhaseError::IntegrationPush("ssh: connection reset by peer".into()),
    )]));
    let cleaner = Arc::new(FakeAssignmentWorktreeCleaner::new());
    let lifecycle = Arc::new(FakeLifecycleWriter::new());

    let router = router_with_plan_run_merge_deps(
        config(),
        db.clone(),
        Arc::new(StaticIntegrationBranchRefresher::new(RefreshedBaseline {
            commit_sha: "baseline-sha".into(),
        })),
        Arc::new(FakePlanningPhaseRunner::with_stdout(
            r#"<plan>{"issues":[{"source_issue_id":"42","title":"t","branch":"agent/issue-42","selection_summary":"ok"}],"summary":"s"}</plan>"#,
        )),
        Arc::new(FakeWorktreeProvisioner::new(std::env::temp_dir()))
            as Arc<dyn AssignmentWorktreeProvisioner>,
        lifecycle.clone() as Arc<dyn IssueLifecycleWriter>,
        Arc::new(FakeImplementationPhaseRunner::with_stdout(IMPL_OK))
            as Arc<dyn ImplementationPhaseRunner>,
        Arc::new(FakeReviewPhaseRunner::with_stdout(REVIEW_APPROVED))
            as Arc<dyn ReviewPhaseRunner>,
        Arc::new(FakeMergePhaseRunner::with_stdout(MERGE_OK)) as Arc<dyn MergePhaseRunner>,
        pusher.clone() as Arc<dyn IntegrationBranchPusher>,
        cleaner.clone() as Arc<dyn AssignmentWorktreeCleaner>,
    );

    let project_dir = temp_dir("abandon");
    std::fs::create_dir_all(&project_dir).unwrap();
    let pid = build_project(&router, &db, &project_dir).await;

    let _ = start_plan_run(&router, &pid).await;
    let runs = persistence::list_recent_plan_runs(&db, &pid, 5).await.unwrap();
    let assignment_id = runs[0].assignments[0].id.clone();
    assert_eq!(runs[0].assignments[0].status, "merge_staged");
    let pushes_before = pusher.call_count();

    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/projects/{pid}/assignments/{assignment_id}/abandon-staged"
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "note": "operator declined staged work" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let outcome: agentic_afk_contracts::AbandonStagedResponse = read_json(resp).await;
    assert_eq!(outcome.status, "blocked");
    let reason = outcome.block_reason.expect("abandon must carry block_reason");
    assert_eq!(
        reason.kind,
        agentic_afk_contracts::BlockReason::AbandonedStaged
    );
    assert_eq!(
        reason.detail.as_deref(),
        Some("operator declined staged work")
    );

    // No new push attempts; cleanup ran once for the now-terminal assignment.
    assert_eq!(pusher.call_count(), pushes_before);
    assert_eq!(cleaner.call_count(), 1);

    // No Lifecycle Completed write-back: staged work was abandoned.
    let completed: Vec<_> = lifecycle
        .calls()
        .into_iter()
        .filter(|(_, status)| {
            matches!(
                status,
                agentic_afk_orchestrator::LifecycleStatus::Completed
            )
        })
        .collect();
    assert!(completed.is_empty());

    // Plan Run terminal status remains failed.
    let runs = persistence::list_recent_plan_runs(&db, &pid, 5).await.unwrap();
    assert_eq!(runs[0].state, "failed");

    drop(project_dir);
}
