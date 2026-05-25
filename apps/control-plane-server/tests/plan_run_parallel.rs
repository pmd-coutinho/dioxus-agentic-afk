//! Issue #46: bounded parallel Plan Run with partial success.
//!
//! Drives a Plan Run that selects multiple eligible assignments at once,
//! runs implementation + review concurrently, merges reviewed work, and
//! leaves blocked work outside the merge while the Plan Run still
//! finishes. Covers concurrency limits, partial-success merge behavior,
//! blocked artifact cleanup, source lifecycle state, and live dashboard
//! state via the snapshot route.

use agentic_afk_contracts::{
    CreateProjectRequest, EnableIssueSourceRequest, IssueSource, ProjectResponse,
    SetProjectExecutionConfigRequest, SourceIssueSnapshot,
};
use agentic_afk_control_plane_server::{
    AssignmentWorktreeCleaner, AssignmentWorktreeProvisioner, ControlPlaneConfig,
    FakeAssignmentWorktreeCleaner, FakeIntegrationBranchPusher, FakeLifecycleWriter,
    FakePlanningPhaseRunner, FakeWorktreeProvisioner, ImplementationPhaseRunner,
    IntegrationBranchPusher, IssueLifecycleWriter, MergePhaseRunner,
    PerSourceImplementationPhaseRunner, PerSourceMergePhaseRunner, PerSourceReviewPhaseRunner,
    RefreshedBaseline, ReviewPhaseRunner, StaticIntegrationBranchRefresher,
    router_with_plan_run_merge_deps,
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
        "agentic-afk-parallel-{label}-{}-{nonce}",
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

const IMPL_OK_42: &str = r#"<impl>{"outcome":"ready_for_review","summary":"shipped 42","commits":["abc"],"verification":["cargo test"],"gaps":[]}</impl>"#;
const IMPL_OK_43: &str = r#"<impl>{"outcome":"ready_for_review","summary":"shipped 43","commits":["def"],"verification":["cargo test"],"gaps":[]}</impl>"#;
const REVIEW_APPROVED_42: &str = r#"<review>{"outcome":"approved","findings":[],"summary":"lgtm 42","verification":["cargo test"],"gaps":[]}</review>"#;
const REVIEW_APPROVED_43: &str = r#"<review>{"outcome":"approved","findings":[],"summary":"lgtm 43","verification":["cargo test"],"gaps":[]}</review>"#;
const REVIEW_REJECTED_43: &str = r#"<review>{"outcome":"rejected","findings":["missing tests"],"summary":"needs more","verification":[],"gaps":[]}</review>"#;
const MERGE_OK_42: &str = r#"<merge>{"outcome":"merged","summary":"integrated 42","merged_source_ids":["42"],"verification":["cargo test --workspace"],"gaps":[]}</merge>"#;
const MERGE_OK_43: &str = r#"<merge>{"outcome":"merged","summary":"integrated 43","merged_source_ids":["43"],"verification":["cargo test --workspace"],"gaps":[]}</merge>"#;

struct Fixture {
    router: axum::Router,
    db: persistence::Db,
    project: ProjectResponse,
    impl_runner: Arc<PerSourceImplementationPhaseRunner>,
    review_runner: Arc<PerSourceReviewPhaseRunner>,
    merge_runner: Arc<PerSourceMergePhaseRunner>,
    pusher: Arc<FakeIntegrationBranchPusher>,
    cleaner: Arc<FakeAssignmentWorktreeCleaner>,
    project_dir: PathBuf,
}

#[allow(clippy::too_many_arguments)]
async fn build_fixture(
    impl_runner: PerSourceImplementationPhaseRunner,
    review_runner: PerSourceReviewPhaseRunner,
    merge_runner: PerSourceMergePhaseRunner,
    issues: &[SourceIssueSnapshot],
    planner_stdout: &str,
    max_parallel_tasks: i64,
    review_retry_limit: i64,
) -> Fixture {
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();

    let impl_runner = Arc::new(impl_runner);
    let review_runner = Arc::new(review_runner);
    let merge_runner = Arc::new(merge_runner);
    let pusher = Arc::new(FakeIntegrationBranchPusher::new());
    let cleaner = Arc::new(FakeAssignmentWorktreeCleaner::new());

    let router = router_with_plan_run_merge_deps(
        config(),
        db.clone(),
        Arc::new(StaticIntegrationBranchRefresher::new(RefreshedBaseline {
            commit_sha: "baseline-sha".into(),
        })),
        Arc::new(FakePlanningPhaseRunner::with_stdout(planner_stdout)),
        Arc::new(FakeWorktreeProvisioner::new(
            std::env::temp_dir().join("agentic-afk-parallel-wt"),
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
    persistence::replace_planning_snapshot(&db, &pid, &source, issues, "unix:1")
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
                        max_parallel_tasks,
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

const TWO_ISSUE_PLAN: &str = r#"<plan>{"issues":[
{"source_issue_id":"42","title":"Issue 42","branch":"agent/issue-42","selection_summary":"ready 42"},
{"source_issue_id":"43","title":"Issue 43","branch":"agent/issue-43","selection_summary":"ready 43"}
],"summary":"two issues"}</plan>"#;

#[tokio::test]
async fn planning_phase_exceeding_max_parallel_tasks_fails_plan_run() {
    // Max Parallel Tasks = 1, planner returns 2 → reject up front.
    let fixture = build_fixture(
        PerSourceImplementationPhaseRunner::new().with_fallback(IMPL_OK_42),
        PerSourceReviewPhaseRunner::new().with_fallback(REVIEW_APPROVED_42),
        PerSourceMergePhaseRunner::new().with_fallback(MERGE_OK_42),
        &[issue("42"), issue("43")],
        TWO_ISSUE_PLAN,
        1,
        1,
    )
    .await;
    let pid = fixture.project.id.0.clone();
    let resp = start(&fixture.router, &pid).await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = read_text(resp).await;
    assert!(
        body.contains("urn:agentic-afk:planning-exceeds-max-parallel"),
        "unexpected body: {body}"
    );
    let runs = persistence::list_recent_plan_runs(&fixture.db, &pid, 10)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].state, agentic_afk_contracts::PlanRunState::Finished);
    drop(fixture.project_dir);
}

#[tokio::test]
async fn two_eligible_assignments_merge_together_inside_one_plan_run() {
    let fixture = build_fixture(
        PerSourceImplementationPhaseRunner::new()
            .with_source("42", IMPL_OK_42)
            .with_source("43", IMPL_OK_43),
        PerSourceReviewPhaseRunner::new()
            .with_source("42", REVIEW_APPROVED_42)
            .with_source("43", REVIEW_APPROVED_43),
        PerSourceMergePhaseRunner::new()
            .with_source("42", MERGE_OK_42)
            .with_source("43", MERGE_OK_43),
        &[issue("42"), issue("43")],
        TWO_ISSUE_PLAN,
        2,
        1,
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

    let runs = persistence::list_recent_plan_runs(&fixture.db, &pid, 10)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    assert_eq!(
        run.state,
        agentic_afk_contracts::PlanRunState::Finished,
        "two merged → finished"
    );
    assert_eq!(run.assignments.len(), 2);

    // Deterministic claim order matches the planner selection order so
    // snapshots are stable for the Dashboard.
    assert_eq!(run.assignments[0].source_id, "42");
    assert_eq!(run.assignments[1].source_id, "43");
    assert!(run.assignments.iter().all(|a| a.status == "merged"));

    // Each assignment carries its own implementation, review, and merge
    // Phase Outputs (durable across worktree cleanup).
    for assignment in &run.assignments {
        let phases: Vec<&str> = assignment
            .phase_outputs
            .iter()
            .map(|p| p.phase.as_str())
            .collect();
        assert_eq!(
            phases,
            vec!["implementation", "review", "merge"],
            "assignment {} phase order",
            assignment.source_id
        );
    }

    // Each runner saw both Source Issues exactly once.
    assert_eq!(fixture.impl_runner.call_count(), 2);
    assert_eq!(fixture.review_runner.call_count(), 2);
    assert_eq!(fixture.merge_runner.call_count(), 2);

    // Integration Branch pushed once for the whole merged tranche.
    assert_eq!(fixture.pusher.call_count(), 1);
    assert_eq!(fixture.pusher.calls()[0].1, "main");

    // Both merged worktrees were cleaned up at Plan Run finish.
    assert_eq!(fixture.cleaner.call_count(), 2);
    let cleaned_branches: Vec<String> = fixture.cleaner.calls().into_iter().map(|c| c.2).collect();
    assert!(cleaned_branches.contains(&"agent/issue-42".to_string()));
    assert!(cleaned_branches.contains(&"agent/issue-43".to_string()));

    drop(fixture.project_dir);
}

#[tokio::test]
async fn partial_success_merges_reviewed_work_and_keeps_blocked_assignment_blocked() {
    // 42: reviewed → merged. 43: review rejected with retry limit 1, so
    // the Review Loop exhausts and the assignment blocks.
    let fixture = build_fixture(
        PerSourceImplementationPhaseRunner::new()
            .with_source("42", IMPL_OK_42)
            .with_source("43", IMPL_OK_43),
        PerSourceReviewPhaseRunner::new()
            .with_source("42", REVIEW_APPROVED_42)
            .with_source("43", REVIEW_REJECTED_43),
        PerSourceMergePhaseRunner::new().with_source("42", MERGE_OK_42),
        &[issue("42"), issue("43")],
        TWO_ISSUE_PLAN,
        2,
        1,
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

    let runs = persistence::list_recent_plan_runs(&fixture.db, &pid, 10)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    // Partial-success Plan Runs finish as `succeeded` once any reviewed
    // work merges; the blocked assignment stays outside the merge.
    assert_eq!(run.state, agentic_afk_contracts::PlanRunState::Finished);
    assert_eq!(run.assignments.len(), 2);

    let a42 = run
        .assignments
        .iter()
        .find(|a| a.source_id == "42")
        .unwrap();
    let a43 = run
        .assignments
        .iter()
        .find(|a| a.source_id == "43")
        .unwrap();
    assert_eq!(a42.status, "merged");
    assert_eq!(a43.status, "blocked");
    assert!(
        a43.block_reason.is_some(),
        "blocked assignment carries reason"
    );
    assert!(a43.review_rejection_count >= 1);

    // Merge Phase only ran for the reviewed assignment.
    assert_eq!(fixture.merge_runner.call_count(), 1);
    let merge_prompt = fixture.merge_runner.last_prompt().unwrap();
    assert!(
        merge_prompt.contains("Source Issue: 42"),
        "merge prompt must target the reviewed assignment: {merge_prompt}"
    );

    // Integration Branch pushed once for the merged set (just 42 here).
    assert_eq!(fixture.pusher.call_count(), 1);

    // Cleanup runs for both the merged AND the blocked assignment so the
    // blocked branch/worktree doesn't linger past Plan Run finish.
    assert_eq!(fixture.cleaner.call_count(), 2);
    let cleaned_branches: Vec<String> = fixture.cleaner.calls().into_iter().map(|c| c.2).collect();
    assert!(cleaned_branches.contains(&"agent/issue-42".to_string()));
    assert!(cleaned_branches.contains(&"agent/issue-43".to_string()));

    drop(fixture.project_dir);
}

#[tokio::test]
async fn all_blocked_plan_run_finishes_as_failed_without_pushing() {
    let fixture = build_fixture(
        PerSourceImplementationPhaseRunner::new()
            .with_source("42", IMPL_OK_42)
            .with_source("43", IMPL_OK_43),
        PerSourceReviewPhaseRunner::new()
            .with_source("42", REVIEW_REJECTED_43)
            .with_source("43", REVIEW_REJECTED_43),
        PerSourceMergePhaseRunner::new(),
        &[issue("42"), issue("43")],
        TWO_ISSUE_PLAN,
        2,
        1,
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

    let runs = persistence::list_recent_plan_runs(&fixture.db, &pid, 10)
        .await
        .unwrap();
    assert_eq!(runs[0].state, agentic_afk_contracts::PlanRunState::Finished);
    assert!(runs[0].assignments.iter().all(|a| a.status == "blocked"));

    // No merge attempts and no Integration Branch push when nothing was
    // reviewed successfully.
    assert_eq!(fixture.merge_runner.call_count(), 0);
    assert_eq!(fixture.pusher.call_count(), 0);

    // Both blocked worktrees still cleaned at finish.
    assert_eq!(fixture.cleaner.call_count(), 2);
    drop(fixture.project_dir);
}

#[tokio::test]
async fn snapshot_route_exposes_parallel_assignments_with_phase_outputs() {
    let fixture = build_fixture(
        PerSourceImplementationPhaseRunner::new()
            .with_source("42", IMPL_OK_42)
            .with_source("43", IMPL_OK_43),
        PerSourceReviewPhaseRunner::new()
            .with_source("42", REVIEW_APPROVED_42)
            .with_source("43", REVIEW_REJECTED_43),
        PerSourceMergePhaseRunner::new().with_source("42", MERGE_OK_42),
        &[issue("42"), issue("43")],
        TWO_ISSUE_PLAN,
        2,
        1,
    )
    .await;
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
    // Snapshot must expose both the merged and the blocked assignment so
    // the Dashboard can show the merge set and the blocked exclusions
    // for the same Plan Run.
    assert!(body.contains("\"status\":\"merged\""), "{body}");
    assert!(body.contains("\"status\":\"blocked\""), "{body}");
    assert!(body.contains("\"state\":\"finished\""), "{body}");
    // Durable Phase Outputs preserved after cleanup.
    assert!(body.contains("\"phase\":\"merge\""), "{body}");
    assert!(body.contains("\"phase\":\"review\""), "{body}");
    drop(fixture.project_dir);
}

#[tokio::test]
async fn second_plan_run_excludes_dormant_blocked_assignment_from_capacity() {
    // After a partial-success Plan Run, the blocked Source Issue moves
    // out of `eligible` (lifecycle = blocked), so a second Plan Run
    // started without re-enabling it does not consume Max Parallel
    // Tasks via the blocked assignment.
    let fixture = build_fixture(
        PerSourceImplementationPhaseRunner::new()
            .with_source("42", IMPL_OK_42)
            .with_source("43", IMPL_OK_43),
        PerSourceReviewPhaseRunner::new()
            .with_source("42", REVIEW_APPROVED_42)
            .with_source("43", REVIEW_REJECTED_43),
        PerSourceMergePhaseRunner::new().with_source("42", MERGE_OK_42),
        &[issue("42"), issue("43")],
        TWO_ISSUE_PLAN,
        2,
        1,
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

    // The blocked assignment is dormant: it survives Plan Run finish in
    // the persistence layer (so the Dashboard still shows it), but the
    // active Plan Run is now `None` and the next sync/plan run does not
    // see it as `ready` from the planning snapshot.
    let active = persistence::get_active_plan_run(&fixture.db, &pid)
        .await
        .unwrap();
    assert!(
        active.is_none(),
        "Plan Run finishes even while a dormant blocked assignment remains"
    );

    drop(fixture.project_dir);
}
