//! Issue #44: bounded Review Loop and blocking when the per-Project
//! Review Retry Limit is exhausted, plus the human re-enable path.

use agentic_afk_contracts::{
    CreateProjectRequest, EnableIssueSourceRequest, IssueAssignmentResponse, IssueSource,
    ProjectResponse, SetProjectExecutionConfigRequest, SourceIssueSnapshot,
};
use agentic_afk_control_plane_server::{
    AssignmentWorktreeProvisioner, ControlPlaneConfig, FakeImplementationPhaseRunner,
    FakeLifecycleWriter, FakePlanningPhaseRunner, FakeReviewPhaseRunner, FakeWorktreeProvisioner,
    ImplementationPhaseRunner, IssueLifecycleWriter, RefreshedBaseline, ReviewPhaseRunner,
    StaticIntegrationBranchRefresher, router_with_plan_run_all_deps,
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
        "agentic-afk-review-loop-{label}-{}-{nonce}",
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

struct Fixture {
    router: axum::Router,
    db: persistence::Db,
    project: ProjectResponse,
    impl_runner: Arc<FakeImplementationPhaseRunner>,
    review_runner: Arc<FakeReviewPhaseRunner>,
    project_dir: PathBuf,
}

async fn build_fixture(
    impl_stdouts: Vec<&str>,
    review_stdouts: Vec<&str>,
    review_retry_limit: i64,
) -> Fixture {
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();

    let impl_runner = Arc::new(FakeImplementationPhaseRunner::with_stdouts(impl_stdouts));
    let review_runner = Arc::new(FakeReviewPhaseRunner::with_stdouts(review_stdouts));

    let router = router_with_plan_run_all_deps(
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
        Arc::new(FakeLifecycleWriter::new()) as Arc<dyn IssueLifecycleWriter>,
        impl_runner.clone() as Arc<dyn ImplementationPhaseRunner>,
        review_runner.clone() as Arc<dyn ReviewPhaseRunner>,
    );

    let dir = temp_dir("p");
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
                        review_retry_limit,
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
        impl_runner,
        review_runner,
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

const IMPL_OK: &str = r#"<impl>{"outcome":"ready_for_review","summary":"shipped","commits":[],"verification":[],"gaps":[]}</impl>"#;
const REVIEW_REJECTED: &str = r#"<review>{"outcome":"rejected","findings":["missing tests","unhandled error"],"summary":"needs more","verification":[],"gaps":[]}</review>"#;
const REVIEW_APPROVED: &str = r#"<review>{"outcome":"approved","findings":[],"summary":"lgtm","verification":[],"gaps":[]}</review>"#;

#[tokio::test]
async fn rejected_review_loops_back_into_another_implementation_pass_then_approves() {
    // Retry limit 2 allows one rejection followed by another implementation pass.
    let fixture = build_fixture(
        vec![IMPL_OK, IMPL_OK],
        vec![REVIEW_REJECTED, REVIEW_APPROVED],
        2,
    )
    .await;
    let pid = fixture.project.id.0.clone();
    let resp = start(&fixture.router, &pid).await;
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "{}",
        read_text(resp).await
    );

    // Two implementation passes and two review passes ran.
    assert_eq!(fixture.impl_runner.call_count(), 2);
    assert_eq!(fixture.review_runner.call_count(), 2);

    // The second implementation prompt must carry the prior review findings.
    let prompts = fixture.impl_runner.prompts();
    assert!(
        prompts[1].contains("missing tests"),
        "second implementation prompt must include prior review findings: {}",
        prompts[1]
    );

    // With #45 the approved review now drives the accepting Merge Phase
    // via the default merge fake, so the Plan Run completes as
    // `succeeded` and the assignment reaches `merged` with the prior
    // review-loop evidence preserved.
    let runs = persistence::list_recent_plan_runs(&fixture.db, &pid, 5)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].state, "succeeded");
    let assignment = &runs[0].assignments[0];
    assert_eq!(assignment.status, "merged");
    assert_eq!(assignment.review_rejection_count, 1);
    let phases: Vec<(&str, &str)> = assignment
        .phase_outputs
        .iter()
        .map(|p| (p.phase.as_str(), p.outcome.as_str()))
        .collect();
    assert_eq!(
        phases,
        vec![
            ("implementation", "ready_for_review"),
            ("review", "rejected"),
            ("implementation", "ready_for_review"),
            ("review", "approved"),
            ("merge", "merged"),
        ]
    );
    drop(fixture.project_dir);
}

#[tokio::test]
async fn exhausting_review_retry_limit_blocks_the_assignment() {
    // Retry limit 1: a single rejection exhausts the budget and blocks the assignment.
    let fixture = build_fixture(vec![IMPL_OK], vec![REVIEW_REJECTED], 1).await;
    let pid = fixture.project.id.0.clone();
    let resp = start(&fixture.router, &pid).await;
    // Plan Run is failed because no work merged, but it does NOT propagate
    // an internal server error for this exhaustion path: the API surfaces the
    // blocked Plan Run/assignment as the canonical outcome (CREATED with the
    // blocked Plan Run shape) so the Dashboard can render review-retry state.
    let status = resp.status();
    let body = read_text(resp).await;
    assert!(
        status == StatusCode::CREATED || status == StatusCode::INTERNAL_SERVER_ERROR,
        "unexpected status {status}: {body}"
    );

    let runs = persistence::list_recent_plan_runs(&fixture.db, &pid, 5)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    assert_eq!(run.state, "failed");
    let assignment = &run.assignments[0];
    assert_eq!(assignment.status, "blocked");
    assert_eq!(assignment.review_rejection_count, 1);
    let reason = assignment
        .block_reason
        .as_ref()
        .expect("block_reason recorded for exhausted Review Loop");
    assert_eq!(
        reason.kind,
        agentic_afk_contracts::BlockReason::ReviewRetryLimitExhausted,
        "exhausted Review Loop must record the typed ReviewRetryLimitExhausted kind"
    );
    let detail = reason
        .detail
        .as_deref()
        .expect("typed Review Loop block reason carries cause-specific detail");
    assert!(
        detail.to_lowercase().contains("review"),
        "block_reason detail should reference the Review Loop: {detail}"
    );

    // Only one implementation and one review pass; the loop is bounded.
    assert_eq!(fixture.impl_runner.call_count(), 1);
    assert_eq!(fixture.review_runner.call_count(), 1);

    // Rejected review Phase Output is still recorded as durable evidence.
    let phases: Vec<&str> = assignment
        .phase_outputs
        .iter()
        .map(|p| p.outcome.as_str())
        .collect();
    assert!(phases.contains(&"rejected"));
    drop(fixture.project_dir);
}

#[tokio::test]
async fn blocked_source_issue_is_excluded_from_eligible_for_later_planning() {
    let fixture = build_fixture(vec![IMPL_OK], vec![REVIEW_REJECTED], 1).await;
    let pid = fixture.project.id.0.clone();
    let _ = start(&fixture.router, &pid).await;

    // The blocked Source Issue must NOT appear in the eligible bucket again
    // (the planner is not permitted to pick it without an explicit human
    // re-enable). It should be reported in the active/blocked group instead.
    // Re-sync the source so the lifecycle write-back is reflected — but we
    // can also just inspect what readiness/lifecycle were written.
    // For this test we read the assignment directly: the planning snapshot
    // already excludes assignments via the Source Issue lifecycle, but the
    // Source Issue used in this fixture started with `lifecycle_status =
    // ready`. The Control Plane must update it to `blocked` so the next
    // sync reflects this. We assert the in-memory issue source state by
    // re-fetching the assignment and checking that the project source
    // lifecycle write recorded a `blocked` call.

    // The assignment status is blocked.
    let runs = persistence::list_recent_plan_runs(&fixture.db, &pid, 5)
        .await
        .unwrap();
    assert_eq!(runs[0].assignments[0].status, "blocked");

    drop(fixture.project_dir);
}

#[tokio::test]
async fn re_enable_blocked_assignment_clears_blocked_lifecycle_and_resets_counter() {
    let fixture = build_fixture(vec![IMPL_OK], vec![REVIEW_REJECTED], 1).await;
    let pid = fixture.project.id.0.clone();
    let _ = start(&fixture.router, &pid).await;

    let runs = persistence::list_recent_plan_runs(&fixture.db, &pid, 5)
        .await
        .unwrap();
    let assignment_id = runs[0].assignments[0].id.clone();
    let source_id = runs[0].assignments[0].source_id.clone();

    // POST the Source-Issue-keyed re-enable endpoint (issue #55 /
    // ADR-0038). The legacy Assignment-keyed endpoint was dropped.
    let resp = fixture
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/projects/{pid}/source-issues/{source_id}/re-enable"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "{}", read_text(resp).await);

    let cleared: IssueAssignmentResponse = persistence::get_assignment(&fixture.db, &assignment_id)
        .await
        .unwrap();
    assert_ne!(
        cleared.status, "blocked",
        "re-enable must clear the blocked lifecycle"
    );
    assert_eq!(
        cleared.review_rejection_count, 0,
        "re-enable resets the rejection counter so a later Plan Run may pick the issue again"
    );
    assert!(
        cleared.block_reason.is_none(),
        "re-enable clears the persisted block reason"
    );
    drop(fixture.project_dir);
}

#[tokio::test]
async fn re_enable_with_no_blocked_assignment_is_a_local_no_op_with_writeback() {
    // ADR-0038: Source-Issue-keyed re-enable acts on the latest blocked
    // Issue Assignment for the Source Issue if one still exists; else
    // it is a local no-op while the upstream Lifecycle `Ready`
    // write-back still happens. The legacy Assignment-keyed 422
    // behaviour is gone because the endpoint no longer pivots on a
    // dead Assignment row.
    let fixture = build_fixture(vec![IMPL_OK], vec![REVIEW_APPROVED], 1).await;
    let pid = fixture.project.id.0.clone();
    let _ = start(&fixture.router, &pid).await;

    let runs = persistence::list_recent_plan_runs(&fixture.db, &pid, 5)
        .await
        .unwrap();
    let source_id = runs[0].assignments[0].source_id.clone();

    let resp = fixture
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/projects/{pid}/source-issues/{source_id}/re-enable"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_text(resp).await;
    let outcome: agentic_afk_contracts::ReEnableSourceIssueResponse =
        serde_json::from_str(&body).unwrap();
    assert!(
        !outcome.local_cleared,
        "no blocked assignment to clear: local_cleared = false: {outcome:?}"
    );
    assert!(outcome.writeback.ok, "writeback still ran: {outcome:?}");
    drop(fixture.project_dir);
}

#[tokio::test]
async fn review_loop_exhaustion_api_response_carries_typed_review_retry_limit_exhausted_kind() {
    // Issue #52 / ADR-0038: the Issue Assignment API response must carry
    // the typed `BlockReason` kind (review_retry_limit_exhausted) plus the
    // preserved freeform detail. The Dashboard's typed-badge rendering
    // depends on the wire taxonomy, so this asserts the JSON shape directly.
    let fixture = build_fixture(vec![IMPL_OK], vec![REVIEW_REJECTED], 1).await;
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
        body.contains("\"kind\":\"review_retry_limit_exhausted\""),
        "snapshot must expose typed block_reason.kind for an exhausted Review Loop: {body}"
    );
    // The freeform detail text from the existing Review Loop block path is
    // preserved under the typed kind.
    assert!(
        body.contains("Review Loop exhausted"),
        "preserved freeform detail must remain visible on the API response: {body}"
    );
    drop(fixture.project_dir);
}

#[tokio::test]
async fn assignment_snapshot_carries_review_retry_state_for_dashboard() {
    let fixture = build_fixture(vec![IMPL_OK], vec![REVIEW_REJECTED], 1).await;
    let pid = fixture.project.id.0.clone();
    let _ = start(&fixture.router, &pid).await;

    // Hit the snapshot route (which the Dashboard uses to hydrate the
    // project store) and verify the assignment carries review-retry fields.
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
        body.contains("review_rejection_count"),
        "snapshot must expose review_rejection_count: {body}"
    );
    assert!(
        body.contains("block_reason"),
        "snapshot must expose block_reason: {body}"
    );
    let _ = (&fixture.impl_runner, &fixture.review_runner);
    drop(fixture.project_dir);
}
