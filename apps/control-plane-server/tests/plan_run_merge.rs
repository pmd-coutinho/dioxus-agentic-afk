//! Issue #45: first accepting Merge Phase for one reviewed Issue
//! Assignment. Covers the end-to-end success path (implement -> review
//! -> merge -> push -> complete -> cleanup -> Plan Run succeeded), the
//! blocked-merge path, the Integration Branch push boundary, and the
//! API snapshot exposure of merge state.

use agentic_afk_contracts::{
    CreateProjectRequest, EnableIssueSourceRequest, IssueSource, ProjectResponse,
    SetProjectExecutionConfigRequest, SourceIssueSnapshot,
};
use agentic_afk_control_plane_server::{
    AssignmentWorktreeCleaner, AssignmentWorktreeProvisioner, ControlPlaneConfig,
    FakeAssignmentWorktreeCleaner, FakeImplementationPhaseRunner, FakeIntegrationBranchPusher,
    FakeLifecycleWriter, FakeMergePhaseRunner, FakePlanningPhaseRunner, FakeReviewPhaseRunner,
    FakeWorktreeProvisioner, ImplementationPhaseRunner, IntegrationBranchPusher,
    IssueLifecycleWriter, MergePhaseRunner, RefreshedBaseline, ReviewPhaseRunner,
    StaticIntegrationBranchRefresher, router_with_plan_run_merge_deps,
};
use agentic_afk_persistence as persistence;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde::de::DeserializeOwned;
use std::path::PathBuf;
use std::sync::Arc;
use tower::ServiceExt;

fn temp_dir(label: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "agentic-afk-merge-{label}-{}-{nonce}",
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
        raw_text: format!("issue brief body for {source_id}"),
    }
}

const IMPL_OK: &str = r#"<impl>{"outcome":"ready_for_review","summary":"shipped","commits":["abc"],"verification":["cargo test"],"gaps":[]}</impl>"#;
const REVIEW_APPROVED: &str = r#"<review>{"outcome":"approved","findings":[],"summary":"lgtm","verification":["cargo test"],"gaps":[]}</review>"#;
const MERGE_OK: &str = r#"<merge>{"outcome":"merged","summary":"integrated cleanly","merged_source_ids":["42"],"verification":["cargo test --workspace"],"gaps":[]}</merge>"#;
const MERGE_BLOCKED: &str = r#"<merge>{"outcome":"blocked","summary":"conflict in module foo","merged_source_ids":[],"verification":["cargo test"],"gaps":["unresolved conflict in src/foo.rs"],"block_reason":"unresolvable merge conflict requires human review"}</merge>"#;

struct Fixture {
    router: axum::Router,
    db: persistence::Db,
    project: ProjectResponse,
    merge_runner: Arc<FakeMergePhaseRunner>,
    pusher: Arc<FakeIntegrationBranchPusher>,
    cleaner: Arc<FakeAssignmentWorktreeCleaner>,
    project_dir: PathBuf,
}

async fn build_fixture(
    impl_stdout: &str,
    review_stdout: &str,
    merge_stdout: &str,
    project_instructions: Option<&str>,
) -> Fixture {
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();

    let impl_runner = Arc::new(FakeImplementationPhaseRunner::with_stdout(impl_stdout));
    let review_runner = Arc::new(FakeReviewPhaseRunner::with_stdout(review_stdout));
    let merge_runner = Arc::new(FakeMergePhaseRunner::with_stdout(merge_stdout));
    let pusher = Arc::new(FakeIntegrationBranchPusher::new());
    let cleaner = Arc::new(FakeAssignmentWorktreeCleaner::new());

    let router = router_with_plan_run_merge_deps(
        config(),
        db.clone(),
        Arc::new(StaticIntegrationBranchRefresher::new(RefreshedBaseline {
            commit_sha: "baseline-sha".into(),
        })),
        Arc::new(FakePlanningPhaseRunner::with_stdout(
            r#"<plan>{"issues":[{"source_issue_id":"42","title":"t","branch":"agent/issue-42","selection_summary":"baseline ready"}],"summary":"s"}</plan>"#,
        )),
        Arc::new(FakeWorktreeProvisioner::new(std::env::temp_dir().join("agentic-afk-merge-wt")))
            as Arc<dyn AssignmentWorktreeProvisioner>,
        Arc::new(FakeLifecycleWriter::new()) as Arc<dyn IssueLifecycleWriter>,
        impl_runner.clone() as Arc<dyn ImplementationPhaseRunner>,
        review_runner.clone() as Arc<dyn ReviewPhaseRunner>,
        merge_runner.clone() as Arc<dyn MergePhaseRunner>,
        pusher.clone() as Arc<dyn IntegrationBranchPusher>,
        cleaner.clone() as Arc<dyn AssignmentWorktreeCleaner>,
    );

    let dir = temp_dir("p");
    std::fs::create_dir_all(&dir).unwrap();
    if let Some(text) = project_instructions {
        std::fs::write(dir.join("AGENTS.md"), text).unwrap();
    }

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
        &db,
        &pid,
        &EnableIssueSourceRequest {
            kind: "github".into(),
            locator: "owner/repo".into(),
        },
    )
    .await
    .unwrap();
    let source = IssueSource {
        kind: "github".into(),
        locator: "owner/repo".into(),
    };
    persistence::replace_planning_snapshot(&db, &pid, &source, &[issue("42")], "unix:1")
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
    let project: ProjectResponse = read_json(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/projects/{pid}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    Fixture {
        router,
        db,
        project,
        merge_runner,
        pusher,
        cleaner,
        project_dir: dir,
    }
}

async fn start(router: &axum::Router, pid: &str) -> axum::response::Response {
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
async fn successful_merge_completes_source_issue_and_finishes_plan_run() {
    let fixture = build_fixture(IMPL_OK, REVIEW_APPROVED, MERGE_OK, None).await;
    let pid = fixture.project.id.0.clone();
    let resp = start(&fixture.router, &pid).await;
    assert_eq!(resp.status(), StatusCode::CREATED, "{}", read_text(resp).await);

    // Plan Run finishes as `succeeded` (not `succeeded_empty` — that
    // outcome is reserved for empty Planning Phases).
    let runs = persistence::list_recent_plan_runs(&fixture.db, &pid, 10)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    assert_eq!(run.state, "succeeded");
    assert!(run.finished_at.is_some());

    // The merged Issue Assignment transitions to `merged`.
    let assignment = &run.assignments[0];
    assert_eq!(assignment.status, "merged");

    // Phase Outputs ordered: planning, implementation, review, merge.
    let phases: Vec<&str> = run.phase_outputs.iter().map(|p| p.phase.as_str()).collect();
    assert_eq!(
        phases,
        vec!["planning", "implementation", "review", "merge"],
        "phase order should be planning -> implementation -> review -> merge"
    );

    // Durable merge Phase Output attributed to the assignment with
    // verification evidence preserved.
    let merge_output = assignment
        .phase_outputs
        .iter()
        .find(|p| p.phase == "merge")
        .expect("merge Phase Output recorded on assignment");
    assert_eq!(merge_output.outcome, "merged");
    assert_eq!(
        merge_output.body_json["verification"][0]
            .as_str()
            .unwrap_or_default(),
        "cargo test --workspace",
        "merge Phase Output must preserve verification evidence"
    );
    assert_eq!(
        merge_output.assignment_id.as_deref(),
        Some(assignment.id.as_str())
    );

    // Integration Branch was pushed exactly once for this Plan Run.
    assert_eq!(fixture.pusher.call_count(), 1);
    assert_eq!(fixture.pusher.calls()[0].1, "main");

    // Worktree cleanup was invoked once with the assignment's branch.
    assert_eq!(fixture.cleaner.call_count(), 1);
    assert_eq!(fixture.cleaner.calls()[0].2, "agent/issue-42");

    drop(fixture.project_dir);
}

#[tokio::test]
async fn blocked_merge_does_not_push_and_fails_plan_run_with_block_reason() {
    let fixture = build_fixture(IMPL_OK, REVIEW_APPROVED, MERGE_BLOCKED, None).await;
    let pid = fixture.project.id.0.clone();
    let resp = start(&fixture.router, &pid).await;
    assert_eq!(resp.status(), StatusCode::CREATED, "{}", read_text(resp).await);

    let runs = persistence::list_recent_plan_runs(&fixture.db, &pid, 10)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    // Failed Plan Run: a blocked Merge Phase does not produce work that
    // can be completed.
    assert_eq!(run.state, "failed");

    // The blocked Merge Phase pushes the assignment into the coarse
    // blocked lifecycle and persists the block reason for the Dashboard.
    let assignment = &run.assignments[0];
    assert_eq!(assignment.status, "blocked");
    let reason = assignment
        .block_reason
        .as_deref()
        .expect("block_reason recorded for blocked Merge Phase");
    assert!(
        reason.contains("merge conflict") || reason.contains("conflict"),
        "block_reason should describe the merge conflict: {reason}"
    );

    // Durable merge Phase Output with the blocked outcome is preserved.
    let merge_output = assignment
        .phase_outputs
        .iter()
        .find(|p| p.phase == "merge")
        .expect("merge Phase Output recorded for blocked merge");
    assert_eq!(merge_output.outcome, "blocked");

    // Push boundary: a blocked merge MUST NOT push the Integration
    // Branch. This is the critical safety property for issue #45.
    assert_eq!(
        fixture.pusher.call_count(),
        0,
        "Integration Branch must not be pushed for a blocked merge"
    );

    // Issue #46: finished Plan Runs clean both merged AND blocked
    // Assignment Worktrees so dormant blocked work does not consume
    // Max Parallel Tasks via stale worktrees. The blocked merge here
    // is the only assignment in the run, so cleanup runs once for
    // its worktree at Plan Run finish.
    assert_eq!(
        fixture.cleaner.call_count(),
        1,
        "Assignment Worktree must be cleaned up at Plan Run finish for blocked merges (issue #46)"
    );

    drop(fixture.project_dir);
}

#[tokio::test]
async fn merge_prompt_carries_project_instructions_and_reviewed_assignment_only() {
    let fixture = build_fixture(
        IMPL_OK,
        REVIEW_APPROVED,
        MERGE_OK,
        Some("# Project Instructions\nNever push without verification."),
    )
    .await;
    let pid = fixture.project.id.0.clone();
    let _ = start(&fixture.router, &pid).await;

    let prompt = fixture
        .merge_runner
        .last_prompt()
        .expect("merge runner called once");
    assert!(
        prompt.contains("Never push without verification."),
        "merge prompt must include Project Instructions: {prompt}"
    );
    assert!(
        prompt.contains("Source Issue: 42"),
        "merge prompt must identify the reviewed Source Issue: {prompt}"
    );
    assert!(
        prompt.contains("Issue Branch: agent/issue-42"),
        "merge prompt must identify the reviewed issue branch: {prompt}"
    );
    assert!(
        prompt.contains("Integration Branch: main"),
        "merge prompt must include the configured Integration Branch: {prompt}"
    );
    assert!(
        prompt.contains("Plan Run Baseline: baseline-sha"),
        "merge prompt must include the Plan Run baseline: {prompt}"
    );
    assert!(
        prompt.contains("Selection Summary: baseline ready"),
        "merge prompt must carry the planner selection summary: {prompt}"
    );
    assert!(
        prompt.contains("lgtm"),
        "merge prompt must include the review Phase Output: {prompt}"
    );

    drop(fixture.project_dir);
}

#[tokio::test]
async fn merge_phase_failure_blocks_assignment_and_fails_plan_run() {
    use agentic_afk_orchestrator::PlanRunPhaseError;
    struct FailingMerger;
    impl MergePhaseRunner for FailingMerger {
        fn run(&self, _prompt: &str) -> Result<String, PlanRunPhaseError> {
            Err(PlanRunPhaseError::Merge("codex merge crashed".into()))
        }
    }

    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let pusher = Arc::new(FakeIntegrationBranchPusher::new());
    let cleaner = Arc::new(FakeAssignmentWorktreeCleaner::new());
    let router = router_with_plan_run_merge_deps(
        config(),
        db.clone(),
        Arc::new(StaticIntegrationBranchRefresher::new(RefreshedBaseline {
            commit_sha: "baseline-sha".into(),
        })),
        Arc::new(FakePlanningPhaseRunner::with_stdout(
            r#"<plan>{"issues":[{"source_issue_id":"42","title":"t","branch":"agent/issue-42","selection_summary":"ok"}],"summary":"s"}</plan>"#,
        )),
        Arc::new(FakeWorktreeProvisioner::new(std::env::temp_dir())),
        Arc::new(FakeLifecycleWriter::new()),
        Arc::new(FakeImplementationPhaseRunner::with_stdout(IMPL_OK)),
        Arc::new(FakeReviewPhaseRunner::with_stdout(REVIEW_APPROVED)),
        Arc::new(FailingMerger),
        pusher.clone(),
        cleaner.clone(),
    );

    let dir = temp_dir("merge-fail");
    std::fs::create_dir_all(&dir).unwrap();
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
        &db,
        &pid,
        &EnableIssueSourceRequest {
            kind: "github".into(),
            locator: "owner/repo".into(),
        },
    )
    .await
    .unwrap();
    persistence::replace_planning_snapshot(
        &db,
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

    let resp = start(&router, &pid).await;
    // Issue #46: a single-assignment merge runner failure no longer
    // returns 500. Instead the assignment is blocked, no push happens,
    // the worktree is cleaned up at Plan Run finish, and the Plan Run
    // settles as `failed` (no merged work). The HTTP response carries
    // the final Plan Run shape.
    assert_eq!(resp.status(), StatusCode::CREATED, "{}", read_text(resp).await);

    assert_eq!(pusher.call_count(), 0, "no push on merge runner failure");
    // Worktree IS cleaned at Plan Run finish (issue #46) for blocked
    // assignments.
    assert_eq!(
        cleaner.call_count(),
        1,
        "blocked worktree is cleaned at Plan Run finish"
    );

    // Plan Run is failed, assignment is blocked.
    let runs = persistence::list_recent_plan_runs(&db, &pid, 10).await.unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].state, "failed");
    assert_eq!(runs[0].assignments[0].status, "blocked");

    drop(dir);
}

#[tokio::test]
async fn snapshot_route_exposes_merged_assignment_and_merge_phase_output() {
    let fixture = build_fixture(IMPL_OK, REVIEW_APPROVED, MERGE_OK, None).await;
    let pid = fixture.project.id.0.clone();
    let _ = start(&fixture.router, &pid).await;

    let resp = fixture
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/projects/{pid}/snapshot"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_text(resp).await;
    assert!(
        body.contains("\"phase\":\"merge\""),
        "snapshot must expose the merge Phase Output: {body}"
    );
    assert!(
        body.contains("\"status\":\"merged\""),
        "snapshot must expose the merged assignment status: {body}"
    );
    // Recent Plan Run history surfaces the succeeded run for Dashboard rendering.
    assert!(
        body.contains("\"state\":\"succeeded\""),
        "snapshot must expose the succeeded Plan Run: {body}"
    );

    drop(fixture.project_dir);
}

#[tokio::test]
async fn parse_merge_output_rejects_unknown_outcomes() {
    let err = agentic_afk_orchestrator::parse_merge_output(
        r#"<merge>{"outcome":"reviewed","summary":"nope"}</merge>"#,
    )
    .unwrap_err();
    assert!(
        err.contains("merged|blocked"),
        "merge parser must reject non-merge outcomes: {err}"
    );

    // Missing tags / malformed bodies are rejected too.
    assert!(agentic_afk_orchestrator::parse_merge_output("nothing structured").is_err());
}
