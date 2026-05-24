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
        docker_binary_path: "docker".into(),
        codex_auth_path: "/dev/null".into(),
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
        Arc::new(FakeWorktreeProvisioner::new(
            std::env::temp_dir().join("agentic-afk-merge-wt"),
        )) as Arc<dyn AssignmentWorktreeProvisioner>,
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
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "{}",
        read_text(resp).await
    );

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

    // Phase Outputs ordered: planning, implementation, review, merge, push.
    // ADR-0038 / issue #53: the Integration Branch push records its own
    // `push`-typed Phase Output on the Plan Run (assignment_id = None).
    let phases: Vec<&str> = run.phase_outputs.iter().map(|p| p.phase.as_str()).collect();
    assert_eq!(
        phases,
        vec!["planning", "implementation", "review", "merge", "push"],
        "phase order should be planning -> implementation -> review -> merge -> push"
    );
    let push_output = run
        .phase_outputs
        .iter()
        .find(|p| p.phase == "push")
        .expect("push Phase Output recorded on Plan Run");
    assert_eq!(push_output.outcome, "succeeded");
    assert!(push_output.assignment_id.is_none());
    assert_eq!(push_output.body_json["fast_forward"].as_bool(), Some(true));

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
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "{}",
        read_text(resp).await
    );

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
        .as_ref()
        .expect("block_reason recorded for blocked Merge Phase");
    assert_eq!(
        reason.kind,
        agentic_afk_contracts::BlockReason::MergePhaseFailed,
        "blocked Merge Phase must record the typed MergePhaseFailed kind"
    );
    let detail = reason
        .detail
        .as_deref()
        .expect("typed Merge Phase block reason carries cause-specific detail");
    assert!(
        detail.contains("merge conflict") || detail.contains("conflict"),
        "block_reason detail should describe the merge conflict: {detail}"
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
async fn merge_phase_failure_api_response_carries_typed_merge_phase_failed_kind() {
    // Issue #52 / ADR-0038: the Issue Assignment API response must carry
    // the typed `BlockReason` kind (merge_phase_failed) plus the preserved
    // freeform detail. The Dashboard's typed-badge rendering depends on
    // the wire taxonomy, so this asserts the JSON shape directly.
    let fixture = build_fixture(IMPL_OK, REVIEW_APPROVED, MERGE_BLOCKED, None).await;
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
        body.contains("\"kind\":\"merge_phase_failed\""),
        "snapshot must expose typed block_reason.kind for a failed Merge Phase: {body}"
    );
    // The freeform detail text from the existing Merge Phase block path is
    // preserved under the typed kind.
    assert!(
        body.contains("unresolvable merge conflict"),
        "preserved freeform detail must remain visible on the API response: {body}"
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
        fn run(
            &self,
            _prompt: &str,
            _context: &agentic_afk_orchestrator::plan_run::AssignmentContext<'_>,
        ) -> Result<String, PlanRunPhaseError> {
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
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "{}",
        read_text(resp).await
    );

    assert_eq!(pusher.call_count(), 0, "no push on merge runner failure");
    // Worktree IS cleaned at Plan Run finish (issue #46) for blocked
    // assignments.
    assert_eq!(
        cleaner.call_count(),
        1,
        "blocked worktree is cleaned at Plan Run finish"
    );

    // Plan Run is failed, assignment is blocked.
    let runs = persistence::list_recent_plan_runs(&db, &pid, 10)
        .await
        .unwrap();
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

#[tokio::test]
async fn integration_push_failure_stages_assignment_and_fails_plan_run() {
    // Issue #51 / ADR-0037: the Merge Phase transitions a verified
    // assignment to `merge_staged` BEFORE the Integration Branch push.
    // When the push fails, the assignment stays at `merge_staged`
    // (dormant, awaiting operator recovery), the Plan Run finishes
    // `failed`, the worktree is NOT cleaned up (cleanup gates on terminal
    // status), and the Source Issue Lifecycle is NOT advanced to
    // `Completed`. The HTTP response is the final Plan Run shape (201
    // CREATED), mirroring the blocked-merge path.
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let pusher = Arc::new(FakeIntegrationBranchPusher::failing("upstream rejected"));
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
        Arc::new(FakeWorktreeProvisioner::new(std::env::temp_dir())),
        lifecycle.clone(),
        Arc::new(FakeImplementationPhaseRunner::with_stdout(IMPL_OK)),
        Arc::new(FakeReviewPhaseRunner::with_stdout(REVIEW_APPROVED)),
        Arc::new(FakeMergePhaseRunner::with_stdout(MERGE_OK)),
        pusher.clone(),
        cleaner.clone(),
    );
    let dir = temp_dir("push-fail");
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
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "push failure now finishes the Plan Run gracefully: {}",
        read_text(resp).await
    );

    assert_eq!(pusher.call_count(), 1, "push attempted exactly once");

    let runs = persistence::list_recent_plan_runs(&db, &pid, 10)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    assert_eq!(run.state, "failed", "push failure fails the Plan Run");

    // ADR-0037: the assignment stays at `merge_staged` after a push
    // failure (dormant for Max Parallel Tasks, awaiting Retry Push /
    // Abandon Staged).
    let assignment = &run.assignments[0];
    assert_eq!(
        assignment.status, "merge_staged",
        "push failure must leave the assignment at merge_staged"
    );

    // Cleanup is gated on terminal Assignment Status (`merged` or
    // `blocked`). A staged assignment retains its worktree and issue
    // branch for operator recovery.
    assert_eq!(
        cleaner.call_count(),
        0,
        "staged assignment must keep its worktree until it reaches a terminal status"
    );

    // ADR-0038 / issue #53: the push failure is recorded as a single
    // Plan-Run-scoped `push` Phase Output (not duplicated as per-
    // assignment merge/failed rows). The developer can still see WHY
    // the run failed via the push Phase Output on the Plan Run.
    let push_outputs: Vec<_> = run
        .phase_outputs
        .iter()
        .filter(|p| p.phase == "push")
        .collect();
    assert!(
        push_outputs.iter().any(|p| p.outcome == "failed"),
        "push failure must surface as a failed push Phase Output on the Plan Run: {:?}",
        push_outputs
    );
    assert!(
        push_outputs[0].assignment_id.is_none(),
        "push Phase Output is Plan-Run-scoped, not per-assignment"
    );

    // Source Issue Lifecycle MUST NOT advance to `Completed` because the
    // upstream never received the integrated tree.
    let completed_writes: Vec<_> = lifecycle
        .calls()
        .into_iter()
        .filter(|(_, status)| {
            matches!(status, agentic_afk_orchestrator::LifecycleStatus::Completed)
        })
        .collect();
    assert!(
        completed_writes.is_empty(),
        "push failure must not write Lifecycle Completed: {completed_writes:?}"
    );

    drop(dir);
}

#[tokio::test]
async fn happy_path_push_passes_assignment_through_merge_staged_to_merged() {
    // Issue #51 / ADR-0037: on the push-success path the assignment
    // passes through `merge_staged` momentarily and ends at `merged`, the
    // Plan Run finishes `succeeded`, and the Source Issue Lifecycle is
    // written back as `Completed`. The transient `merge_staged`
    // transition is published via the SSE delta seam.
    use agentic_afk_contracts::{ProjectEvent, ProjectId};
    use agentic_afk_control_plane_server::event_bus::EventBus;
    use agentic_afk_control_plane_server::router_with_plan_run_merge_deps_and_bus;
    use futures_util::StreamExt;
    use std::time::Duration;
    use tokio::time::timeout;

    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let pusher = Arc::new(FakeIntegrationBranchPusher::new());
    let cleaner = Arc::new(FakeAssignmentWorktreeCleaner::new());
    let lifecycle = Arc::new(FakeLifecycleWriter::new());
    let bus = EventBus::new();

    let router = router_with_plan_run_merge_deps_and_bus(
        config(),
        db.clone(),
        Arc::new(StaticIntegrationBranchRefresher::new(RefreshedBaseline {
            commit_sha: "baseline-sha".into(),
        })),
        Arc::new(FakePlanningPhaseRunner::with_stdout(
            r#"<plan>{"issues":[{"source_issue_id":"42","title":"t","branch":"agent/issue-42","selection_summary":"ok"}],"summary":"s"}</plan>"#,
        )),
        Arc::new(FakeWorktreeProvisioner::new(std::env::temp_dir())),
        lifecycle.clone(),
        Arc::new(FakeImplementationPhaseRunner::with_stdout(IMPL_OK)),
        Arc::new(FakeReviewPhaseRunner::with_stdout(REVIEW_APPROVED)),
        Arc::new(FakeMergePhaseRunner::with_stdout(MERGE_OK)),
        pusher.clone(),
        cleaner.clone(),
        bus.clone(),
    );

    let dir = temp_dir("push-ok-staged");
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

    // Subscribe BEFORE triggering — capture every Plan Run delta.
    let mut subscriber = Box::pin(bus.subscribe(&ProjectId(pid.clone()), None));

    let resp = start(&router, &pid).await;
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "{}",
        read_text(resp).await
    );

    // Plan Run is `succeeded` and the assignment ended at `merged`.
    let runs = persistence::list_recent_plan_runs(&db, &pid, 10)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    assert_eq!(run.state, "succeeded");
    assert_eq!(run.assignments[0].status, "merged");

    // Push happened exactly once and cleanup ran for the merged
    // assignment.
    assert_eq!(pusher.call_count(), 1);
    assert_eq!(cleaner.call_count(), 1);

    // The Source Issue Lifecycle was advanced to `Completed` after the
    // verified push.
    let completed_writes: Vec<_> = lifecycle
        .calls()
        .into_iter()
        .filter(|(_, status)| {
            matches!(status, agentic_afk_orchestrator::LifecycleStatus::Completed)
        })
        .collect();
    assert_eq!(
        completed_writes.len(),
        1,
        "push success must write Lifecycle Completed once"
    );

    // SSE delta seam: the transient `merge_staged` transition is
    // published. Drain the recorded events and look for the merge_staged
    // status on the assignment.
    let mut saw_merge_staged = false;
    let mut saw_merged = false;
    while let Ok(Some(event)) = timeout(Duration::from_millis(250), subscriber.next()).await {
        if let ProjectEvent::AssignmentStatusChanged(assignment) = event.event {
            if assignment.status == "merge_staged" {
                saw_merge_staged = true;
            }
            if assignment.status == "merged" {
                saw_merged = true;
            }
        }
    }
    assert!(
        saw_merge_staged,
        "happy-path push must publish a merge_staged transition over SSE"
    );
    assert!(
        saw_merged,
        "happy-path push must publish the final merged transition over SSE"
    );

    drop(dir);
}

// --- Issue #53 / ADR-0037: Retry Push integration tests ---

/// Pusher that returns canned `Result<(), PlanRunPhaseError>` values in
/// order. Used by the Retry Push tests to stage an assignment with a
/// failing first push, then drive a second push with a different
/// outcome (success / non-fast-forward / transient again). Falls back
/// to repeating the last response after the queue drains so cleanup
/// pushes (none in these tests) cannot panic.
struct ProgrammablePusher {
    responses: Mutex<Vec<Result<(), PlanRunPhaseError>>>,
    calls: Mutex<Vec<String>>,
}

impl ProgrammablePusher {
    fn new(responses: Vec<Result<(), PlanRunPhaseError>>) -> Self {
        assert!(!responses.is_empty());
        Self {
            responses: Mutex::new(responses),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

impl IntegrationBranchPusher for ProgrammablePusher {
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

struct StagedFixture {
    router: axum::Router,
    db: persistence::Db,
    project_id: String,
    assignment_id: String,
    pusher: Arc<ProgrammablePusher>,
    cleaner: Arc<FakeAssignmentWorktreeCleaner>,
    lifecycle: Arc<FakeLifecycleWriter>,
    project_dir: PathBuf,
}

/// Build a fixture that has already started one Plan Run whose first
/// push failed with `first_push_err`, leaving the assignment at
/// `merge_staged`. The `subsequent_responses` are returned on later push
/// calls (e.g. by Retry Push).
async fn build_staged_fixture(
    first_push_err: PlanRunPhaseError,
    subsequent_responses: Vec<Result<(), PlanRunPhaseError>>,
) -> StagedFixture {
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();

    let mut responses: Vec<Result<(), PlanRunPhaseError>> = vec![Err(first_push_err)];
    responses.extend(subsequent_responses);
    let pusher = Arc::new(ProgrammablePusher::new(responses));
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
        Arc::new(FakeWorktreeProvisioner::new(std::env::temp_dir())),
        lifecycle.clone() as Arc<dyn IssueLifecycleWriter>,
        Arc::new(FakeImplementationPhaseRunner::with_stdout(IMPL_OK)),
        Arc::new(FakeReviewPhaseRunner::with_stdout(REVIEW_APPROVED)),
        Arc::new(FakeMergePhaseRunner::with_stdout(MERGE_OK)),
        pusher.clone() as Arc<dyn IntegrationBranchPusher>,
        cleaner.clone() as Arc<dyn AssignmentWorktreeCleaner>,
    );

    let dir = temp_dir("retry-push");
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
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "first plan run should stage the assignment: {}",
        read_text(resp).await
    );
    let runs = persistence::list_recent_plan_runs(&db, &pid, 10)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1, "fixture should have exactly one plan run");
    assert_eq!(runs[0].state, "failed", "fixture push must have failed");
    let assignment = runs[0]
        .assignments
        .first()
        .expect("fixture plan run has one assignment")
        .clone();
    assert_eq!(
        assignment.status, "merge_staged",
        "fixture must leave assignment merge_staged"
    );
    let assignment_id = assignment.id.clone();

    StagedFixture {
        router,
        db,
        project_id: pid,
        assignment_id,
        pusher,
        cleaner,
        lifecycle,
        project_dir: dir,
    }
}

async fn call_retry_push(
    router: &axum::Router,
    project_id: &str,
    assignment_id: &str,
) -> axum::response::Response {
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/projects/{project_id}/assignments/{assignment_id}/retry-push"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn retry_push_success_advances_staged_to_merged_and_plan_run_stays_failed() {
    // Issue #53 / ADR-0037: a staged assignment whose retry push succeeds
    // transitions `merge_staged` → `merged`, writes Lifecycle `Completed`
    // best-effort to the Issue Source, cleans up the worktree, and leaves
    // the originating Plan Run terminal status at `failed` (the original
    // Merge Phase outcome is preserved per ADR-0037).
    let fixture = build_staged_fixture(
        PlanRunPhaseError::IntegrationPush("ssh: temporary failure".into()),
        vec![Ok(())],
    )
    .await;

    let resp = call_retry_push(&fixture.router, &fixture.project_id, &fixture.assignment_id).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let response: agentic_afk_contracts::RetryPushResponse = read_json(resp).await;
    assert_eq!(response.status, "merged");
    assert!(response.block_reason.is_none());

    assert_eq!(fixture.pusher.call_count(), 2, "first push + retry");

    // Assignment is now merged.
    let assignment = persistence::get_assignment(&fixture.db, &fixture.assignment_id)
        .await
        .unwrap();
    assert_eq!(assignment.status, "merged");
    assert!(assignment.block_reason.is_none());

    // Lifecycle write-back: `Completed` recorded after the verified push.
    let completed_writes: Vec<_> = fixture
        .lifecycle
        .calls()
        .into_iter()
        .filter(|(_, status)| {
            matches!(status, agentic_afk_orchestrator::LifecycleStatus::Completed)
        })
        .collect();
    assert_eq!(
        completed_writes.len(),
        1,
        "successful retry push must write Lifecycle Completed"
    );

    // Worktree cleanup ran because the assignment reached a terminal status.
    assert_eq!(fixture.cleaner.call_count(), 1);

    // Plan Run terminal status remains `failed` (ADR-0037).
    let runs = persistence::list_recent_plan_runs(&fixture.db, &fixture.project_id, 10)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(
        runs[0].state, "failed",
        "Plan Run stays failed even after a successful Retry Push"
    );

    // Two push Phase Outputs recorded (first failure + retry success).
    let push_outputs: Vec<_> = runs[0]
        .phase_outputs
        .iter()
        .filter(|p| p.phase == "push")
        .collect();
    assert_eq!(push_outputs.len(), 2);
    assert_eq!(push_outputs[0].outcome, "failed");
    assert_eq!(push_outputs[1].outcome, "succeeded");
    assert_eq!(
        push_outputs[1].body_json["fast_forward"].as_bool(),
        Some(true)
    );

    drop(fixture.project_dir);
}

#[tokio::test]
async fn retry_push_non_fast_forward_blocks_assignment_with_push_non_fast_forward_kind() {
    // Issue #53 / ADR-0037: a staged assignment whose retry push is
    // rejected with a non-fast-forward error transitions
    // `merge_staged` → `blocked` with `BlockReason::PushNonFastForward`.
    // The Integration Branch has diverged; recovery belongs in a new
    // Plan Run with a refreshed baseline.
    let fixture = build_staged_fixture(
        PlanRunPhaseError::IntegrationPush("ssh: temporary failure".into()),
        vec![Err(PlanRunPhaseError::IntegrationPush(
            "! [rejected] main -> main (non-fast-forward)\nhint: Updates were rejected".into(),
        ))],
    )
    .await;

    let resp = call_retry_push(&fixture.router, &fixture.project_id, &fixture.assignment_id).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let response: agentic_afk_contracts::RetryPushResponse = read_json(resp).await;
    assert_eq!(response.status, "blocked");
    let reason = response
        .block_reason
        .expect("non-fast-forward retry must carry a block_reason");
    assert_eq!(
        reason.kind,
        agentic_afk_contracts::BlockReason::PushNonFastForward
    );
    assert!(
        reason
            .detail
            .as_deref()
            .is_some_and(|d| d.contains("non-fast-forward")),
        "block_reason.detail should carry push stderr: {:?}",
        reason.detail
    );

    // Assignment is durably blocked with the typed kind.
    let assignment = persistence::get_assignment(&fixture.db, &fixture.assignment_id)
        .await
        .unwrap();
    assert_eq!(assignment.status, "blocked");
    let reason = assignment
        .block_reason
        .expect("blocked assignment carries block_reason");
    assert_eq!(
        reason.kind,
        agentic_afk_contracts::BlockReason::PushNonFastForward
    );

    // Plan Run remains `failed`.
    let runs = persistence::list_recent_plan_runs(&fixture.db, &fixture.project_id, 10)
        .await
        .unwrap();
    assert_eq!(runs[0].state, "failed");

    // Push Phase Outputs: two failures (initial + non-fast-forward retry).
    let push_outputs: Vec<_> = runs[0]
        .phase_outputs
        .iter()
        .filter(|p| p.phase == "push")
        .collect();
    assert_eq!(push_outputs.len(), 2);
    assert!(push_outputs.iter().all(|p| p.outcome == "failed"));
    // The retry attempt is a non-fast-forward: typed Push body carries
    // `fast_forward=false`, the upstream stderr, and the 1-indexed attempt.
    assert_eq!(
        push_outputs[1].body_json["fast_forward"].as_bool(),
        Some(false)
    );
    assert_eq!(push_outputs[1].body_json["attempt"].as_u64(), Some(2));
    assert!(
        push_outputs[1].body_json["stderr"]
            .as_str()
            .unwrap_or("")
            .to_ascii_lowercase()
            .contains("non-fast-forward")
            || push_outputs[1].body_json["stderr"]
                .as_str()
                .unwrap_or("")
                .to_ascii_lowercase()
                .contains("fetch first"),
        "stderr should carry the non-fast-forward upstream message: {:?}",
        push_outputs[1].body_json["stderr"]
    );

    // No Lifecycle `Completed` write-back because the push never landed.
    let completed: Vec<_> = fixture
        .lifecycle
        .calls()
        .into_iter()
        .filter(|(_, status)| {
            matches!(status, agentic_afk_orchestrator::LifecycleStatus::Completed)
        })
        .collect();
    assert!(completed.is_empty());

    drop(fixture.project_dir);
}

#[tokio::test]
async fn retry_push_transient_other_failure_leaves_assignment_merge_staged() {
    // Issue #53 / ADR-0037: a staged assignment whose retry push fails
    // with a non-fast-forward-unrelated error (network, auth) stays at
    // `merge_staged` so the operator can retry again or abandon.
    let fixture = build_staged_fixture(
        PlanRunPhaseError::IntegrationPush("ssh: temporary failure".into()),
        vec![Err(PlanRunPhaseError::IntegrationPush(
            "ssh: connect to host github.com port 22: Connection timed out".into(),
        ))],
    )
    .await;

    let resp = call_retry_push(&fixture.router, &fixture.project_id, &fixture.assignment_id).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let response: agentic_afk_contracts::RetryPushResponse = read_json(resp).await;
    assert_eq!(response.status, "merge_staged");
    assert!(response.block_reason.is_none());

    // Assignment stays at `merge_staged` — dormant, awaiting another
    // operator action.
    let assignment = persistence::get_assignment(&fixture.db, &fixture.assignment_id)
        .await
        .unwrap();
    assert_eq!(assignment.status, "merge_staged");
    assert!(assignment.block_reason.is_none());

    // Worktree NOT cleaned up: cleanup gates on terminal status only.
    assert_eq!(
        fixture.cleaner.call_count(),
        0,
        "transient retry failure must keep worktree for further retries"
    );

    // No Lifecycle `Completed` write-back.
    let completed: Vec<_> = fixture
        .lifecycle
        .calls()
        .into_iter()
        .filter(|(_, status)| {
            matches!(status, agentic_afk_orchestrator::LifecycleStatus::Completed)
        })
        .collect();
    assert!(completed.is_empty());

    // Both push attempts surfaced as failed `push` Phase Outputs with
    // `kind=other` for the transient cause.
    let runs = persistence::list_recent_plan_runs(&fixture.db, &fixture.project_id, 10)
        .await
        .unwrap();
    let push_outputs: Vec<_> = runs[0]
        .phase_outputs
        .iter()
        .filter(|p| p.phase == "push")
        .collect();
    assert_eq!(push_outputs.len(), 2);
    assert!(push_outputs.iter().all(|p| p.outcome == "failed"));
    // Transient `Other` failure: typed Push body carries
    // `fast_forward=false`, the upstream stderr, and the 1-indexed attempt.
    assert_eq!(
        push_outputs[1].body_json["fast_forward"].as_bool(),
        Some(false)
    );
    assert_eq!(push_outputs[1].body_json["attempt"].as_u64(), Some(2));
    assert!(
        !push_outputs[1].body_json["stderr"]
            .as_str()
            .unwrap_or("")
            .is_empty(),
        "stderr should carry the upstream error text: {:?}",
        push_outputs[1].body_json["stderr"]
    );

    // Operator may still retry — confirm the route still accepts another
    // call (and would 422 if the assignment weren't merge_staged).
    drop(fixture.project_dir);
}

async fn call_abandon_staged(
    router: &axum::Router,
    project_id: &str,
    assignment_id: &str,
    body: Option<&serde_json::Value>,
) -> axum::response::Response {
    let mut builder = Request::builder().method("POST").uri(format!(
        "/api/projects/{project_id}/assignments/{assignment_id}/abandon-staged"
    ));
    let body = if let Some(value) = body {
        builder = builder.header("content-type", "application/json");
        Body::from(serde_json::to_string(value).unwrap())
    } else {
        Body::empty()
    };
    router
        .clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn abandon_staged_transitions_to_blocked_abandoned_staged_with_note() {
    // Issue #54 / ADR-0037: Abandon Staged routes a `merge_staged`
    // assignment to `blocked` with `BlockReason::AbandonedStaged`. No
    // push is attempted; the optional `{ note }` becomes the block
    // reason `detail`. Worktree cleanup runs (terminal status reached).
    // The originating Plan Run stays `failed` (ADR-0037).
    let fixture = build_staged_fixture(
        PlanRunPhaseError::IntegrationPush("ssh: temporary failure".into()),
        vec![Err(PlanRunPhaseError::IntegrationPush(
            "should not be called".into(),
        ))],
    )
    .await;

    let pushes_before = fixture.pusher.call_count();
    let note = "operator decided staged work should not land";
    let resp = call_abandon_staged(
        &fixture.router,
        &fixture.project_id,
        &fixture.assignment_id,
        Some(&serde_json::json!({ "note": note })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let response: agentic_afk_contracts::AbandonStagedResponse = read_json(resp).await;
    assert_eq!(response.status, "blocked");
    let reason = response
        .block_reason
        .expect("abandon-staged must carry a block_reason");
    assert_eq!(
        reason.kind,
        agentic_afk_contracts::BlockReason::AbandonedStaged
    );
    assert_eq!(reason.detail.as_deref(), Some(note));

    // Assignment durably blocked with the typed kind and freeform detail.
    let assignment = persistence::get_assignment(&fixture.db, &fixture.assignment_id)
        .await
        .unwrap();
    assert_eq!(assignment.status, "blocked");
    let persisted = assignment
        .block_reason
        .expect("blocked assignment carries block_reason");
    assert_eq!(
        persisted.kind,
        agentic_afk_contracts::BlockReason::AbandonedStaged
    );
    assert_eq!(persisted.detail.as_deref(), Some(note));

    // No new push attempts.
    assert_eq!(
        fixture.pusher.call_count(),
        pushes_before,
        "abandon-staged must not invoke the pusher"
    );

    // Worktree cleanup ran because the assignment reached a terminal status.
    assert_eq!(fixture.cleaner.call_count(), 1);

    // Plan Run terminal status remains `failed`.
    let runs = persistence::list_recent_plan_runs(&fixture.db, &fixture.project_id, 10)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].state, "failed");

    // No new push Phase Outputs were appended (only the original failure).
    let push_outputs: Vec<_> = runs[0]
        .phase_outputs
        .iter()
        .filter(|p| p.phase == "push")
        .collect();
    assert_eq!(push_outputs.len(), 1);

    // Activity entry was emitted.
    let activity = persistence::list_project_activity(&fixture.db, &fixture.project_id, 50)
        .await
        .unwrap();
    assert!(
        activity
            .iter()
            .any(|entry| entry.kind == "assignment_abandoned_staged"),
        "abandon-staged must emit an activity entry"
    );

    drop(fixture.project_dir);
}

#[tokio::test]
async fn abandon_staged_without_note_omits_detail() {
    // Issue #54: the request body is optional. Absent `note` leaves the
    // typed block reason in place with `detail = None`.
    let fixture = build_staged_fixture(
        PlanRunPhaseError::IntegrationPush("ssh: temporary failure".into()),
        vec![Err(PlanRunPhaseError::IntegrationPush(
            "should not be called".into(),
        ))],
    )
    .await;

    let resp = call_abandon_staged(
        &fixture.router,
        &fixture.project_id,
        &fixture.assignment_id,
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let response: agentic_afk_contracts::AbandonStagedResponse = read_json(resp).await;
    assert_eq!(response.status, "blocked");
    let reason = response.block_reason.expect("block_reason present");
    assert_eq!(
        reason.kind,
        agentic_afk_contracts::BlockReason::AbandonedStaged
    );
    assert!(reason.detail.is_none());

    drop(fixture.project_dir);
}

#[tokio::test]
async fn abandon_staged_rejects_assignments_not_in_merge_staged() {
    // Defensive: the route refuses to act on an assignment that is not
    // currently `merge_staged`.
    let fixture = build_fixture(IMPL_OK, REVIEW_APPROVED, MERGE_OK, None).await;
    let pid = fixture.project.id.0.clone();
    let _ = start(&fixture.router, &pid).await;
    let runs = persistence::list_recent_plan_runs(&fixture.db, &pid, 10)
        .await
        .unwrap();
    let assignment_id = runs[0].assignments[0].id.clone();
    let resp = call_abandon_staged(&fixture.router, &pid, &assignment_id, None).await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    drop(fixture.project_dir);
}

#[tokio::test]
async fn retry_push_rejects_assignments_not_in_merge_staged() {
    // Defensive: the route refuses to act on an assignment that is not
    // currently `merge_staged`. The Dashboard hides the button outside
    // that state but the API contract must be safe regardless.
    let fixture = build_fixture(IMPL_OK, REVIEW_APPROVED, MERGE_OK, None).await;
    let pid = fixture.project.id.0.clone();
    let _ = start(&fixture.router, &pid).await;
    let runs = persistence::list_recent_plan_runs(&fixture.db, &pid, 10)
        .await
        .unwrap();
    let assignment_id = runs[0].assignments[0].id.clone();
    // This assignment is now `merged`, not `merge_staged`.
    let resp = call_retry_push(&fixture.router, &pid, &assignment_id).await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    drop(fixture.project_dir);
}
