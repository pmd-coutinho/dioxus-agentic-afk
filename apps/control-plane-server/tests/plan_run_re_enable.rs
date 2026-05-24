//! Issue #55 / ADR-0038: Source-Issue-keyed re-enable with Lifecycle
//! `Ready` write-back. Integration tests at the HTTP boundary, end-to-end
//! through a Plan Run that blocks an Issue Assignment via the bounded
//! Review Loop and then re-enables via the new
//! `POST /api/projects/{id}/source-issues/{sid}/re-enable` endpoint.

use agentic_afk_contracts::{
    CreateProjectRequest, EnableIssueSourceRequest, IssueSource, PlanningSnapshotResponse,
    ProjectResponse, ReEnableSourceIssueResponse, SetProjectExecutionConfigRequest,
    SourceIssueSnapshot,
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
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

/// Lifecycle writer that fails only on writes matching one Source
/// Issue id + Lifecycle Status. Used to fault-inject the re-enable
/// `Ready` write while still letting the Plan Run's Claim/Blocked
/// writes succeed so the assignment reaches `blocked` for the
/// re-enable surface to act on.
struct FaultyOnReadyWriter {
    target_source_id: String,
    calls: Mutex<Vec<(String, agentic_afk_control_plane_server::LifecycleStatus)>>,
}

impl FaultyOnReadyWriter {
    fn new(target_source_id: impl Into<String>) -> Self {
        Self {
            target_source_id: target_source_id.into(),
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl agentic_afk_control_plane_server::IssueLifecycleWriter for FaultyOnReadyWriter {
    fn write(
        &self,
        source_id: &str,
        status: agentic_afk_control_plane_server::LifecycleStatus,
    ) -> Result<(), agentic_afk_control_plane_server::PlanRunPhaseError> {
        self.calls
            .lock()
            .unwrap()
            .push((source_id.to_string(), status));
        if source_id == self.target_source_id
            && matches!(
                status,
                agentic_afk_control_plane_server::LifecycleStatus::Ready
            )
        {
            return Err(
                agentic_afk_control_plane_server::PlanRunPhaseError::LifecycleWrite(
                    "gh: rate-limited".into(),
                ),
            );
        }
        Ok(())
    }
}

fn temp_dir(label: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "agentic-afk-reenable-source-{label}-{}-{nonce}",
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
        raw_text: format!("brief for {source_id}"),
    }
}

const IMPL_OK: &str = r#"<impl>{"outcome":"ready_for_review","summary":"shipped","commits":[],"verification":[],"gaps":[]}</impl>"#;
const REVIEW_REJECTED: &str = r#"<review>{"outcome":"rejected","findings":["bad"],"summary":"needs more","verification":[],"gaps":[]}</review>"#;

struct Fixture {
    router: axum::Router,
    db: persistence::Db,
    project: ProjectResponse,
    project_dir: PathBuf,
}

async fn build_fixture(lifecycle: Arc<dyn IssueLifecycleWriter>) -> Fixture {
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let impl_runner: Arc<dyn ImplementationPhaseRunner> =
        Arc::new(FakeImplementationPhaseRunner::with_stdout(IMPL_OK));
    let review_runner: Arc<dyn ReviewPhaseRunner> =
        Arc::new(FakeReviewPhaseRunner::with_stdout(REVIEW_REJECTED));
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
        lifecycle,
        impl_runner,
        review_runner,
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
        project_dir: dir,
    }
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

async fn planning_snapshot(router: &axum::Router, pid: &str) -> PlanningSnapshotResponse {
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/projects/{pid}/planning-snapshot"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    read_json(resp).await
}

#[tokio::test]
async fn re_enable_flips_active_source_issue_to_eligible_in_next_planning_snapshot() {
    // ADR-0038 acceptance: a Plan Run that blocks Source Issue `42`
    // first leaves it bucketed as `active` (Lifecycle `Blocked` →
    // active bucket per planning-snapshot normalize). After the
    // Source-Issue-keyed re-enable, a fresh Planning Snapshot must
    // bucket the Source Issue as `eligible` again.
    let lifecycle = Arc::new(FakeLifecycleWriter::new());
    let lifecycle_for_router: Arc<dyn IssueLifecycleWriter> = lifecycle.clone();
    let fixture = build_fixture(lifecycle_for_router).await;
    let pid = fixture.project.id.0.clone();

    // Drive the Plan Run; the bounded Review Loop exhausts and blocks
    // Source Issue `42`. The Plan Run writes Lifecycle `Blocked`
    // upstream, which the fake writer records.
    let _ = start_plan_run(&fixture.router, &pid).await;
    let runs = persistence::list_recent_plan_runs(&fixture.db, &pid, 5)
        .await
        .unwrap();
    let assignment = &runs[0].assignments[0];
    assert_eq!(assignment.status, "blocked");
    assert_eq!(assignment.source_id, "42");

    // Simulate the next Issue Source sync re-reading upstream after the
    // Plan Run wrote Lifecycle `Blocked` upstream: the refreshed local
    // Planning Snapshot mirrors that `blocked` row, which the planner
    // normalize step then buckets as `active`. This re-creates the
    // ADR-0038 problem state: a Source Issue stuck in `active` until
    // the operator re-enables it.
    let source = IssueSource {
        kind: "github".into(),
        locator: "owner/repo".into(),
    };
    let blocked_upstream = SourceIssueSnapshot {
        source_id: "42".into(),
        title: "Issue 42".into(),
        readiness: "ready".into(),
        lifecycle_status: "blocked".into(),
        parent_issue: None,
        issue_dependencies: vec![],
        source_order: 0,
        raw_text: "brief for 42".into(),
    };
    persistence::replace_planning_snapshot(
        &fixture.db,
        &pid,
        &source,
        &[blocked_upstream],
        "unix:2",
    )
    .await
    .unwrap();

    // Pre-condition: Planning Snapshot now buckets `42` as `active`
    // because Lifecycle `Blocked` on a ready issue lands in the `active`
    // bucket per ADR-0036 normalization.
    let before = planning_snapshot(&fixture.router, &pid).await;
    assert!(
        before.active.iter().any(|i| i.source_id == "42"),
        "before re-enable: `42` is in the `active` bucket: {before:?}"
    );
    assert!(
        !before.eligible.iter().any(|i| i.source_id == "42"),
        "before re-enable: `42` is NOT yet in the `eligible` bucket"
    );

    // Re-enable via the new Source-Issue-keyed endpoint.
    let resp = fixture
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/projects/{pid}/source-issues/42/re-enable"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body = read_text(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let outcome: ReEnableSourceIssueResponse = serde_json::from_str(&body).unwrap();
    assert!(outcome.local_cleared, "local clear succeeded: {outcome:?}");
    assert!(
        outcome.writeback.ok,
        "writeback succeeded against the fake: {outcome:?}"
    );

    // Lifecycle writer was called with `Ready` for Source Issue `42`.
    let calls = lifecycle.calls();
    assert!(
        calls.iter().any(|(sid, status)| sid == "42"
            && matches!(
                status,
                agentic_afk_control_plane_server::LifecycleStatus::Ready
            )),
        "Lifecycle Ready was written for Source Issue 42: {calls:?}"
    );

    // Post-condition: Planning Snapshot now buckets `42` as `eligible`.
    let after = planning_snapshot(&fixture.router, &pid).await;
    assert!(
        after.eligible.iter().any(|i| i.source_id == "42"),
        "after re-enable: `42` flips to the `eligible` bucket: {after:?}"
    );
    assert!(
        !after.active.iter().any(|i| i.source_id == "42"),
        "after re-enable: `42` left the `active` bucket"
    );

    drop(fixture.project_dir);
}

#[tokio::test]
async fn re_enable_with_writeback_fault_injection_clears_locally_and_records_activity() {
    // ADR-0035 / ADR-0038: when the Lifecycle write-back fails, the
    // local clear still happens, the response carries
    // `writeback.ok == false`, and a Project Activity entry is
    // recorded so the operator sees the failure in the Dashboard.
    let lifecycle: Arc<dyn IssueLifecycleWriter> = Arc::new(FaultyOnReadyWriter::new("42"));
    let fixture = build_fixture(lifecycle).await;
    let pid = fixture.project.id.0.clone();

    // Drive the Plan Run to a blocked Issue Assignment. The Plan Run
    // itself fails its `Blocked` write-back (the fake lifecycle writer
    // always errors); that does not abort the Plan Run per ADR-0035 so
    // the assignment still reaches `blocked` for the re-enable surface
    // to act on.
    let _ = start_plan_run(&fixture.router, &pid).await;
    let runs = persistence::list_recent_plan_runs(&fixture.db, &pid, 5)
        .await
        .unwrap();
    assert_eq!(runs[0].assignments[0].status, "blocked");

    // Activity entries before the re-enable: drain so we can assert
    // the new entry is the one created by the re-enable use case.
    let before_activity = persistence::list_project_activity(&fixture.db, &pid, 100)
        .await
        .unwrap();
    let before_count = before_activity.len();

    let resp = fixture
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/projects/{pid}/source-issues/42/re-enable"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // Local clear succeeded => 200 OK even though write-back failed.
    let status = resp.status();
    let body = read_text(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let outcome: ReEnableSourceIssueResponse = serde_json::from_str(&body).unwrap();
    assert!(outcome.local_cleared, "local clear still succeeded");
    assert!(!outcome.writeback.ok, "writeback.ok is false: {outcome:?}");
    let error = outcome
        .writeback
        .error
        .as_deref()
        .expect("writeback error message present");
    assert!(
        error.contains("rate-limited"),
        "writeback error surfaces upstream message: {error}"
    );

    // Activity entry recorded for the writeback failure. Best-effort
    // activity recording goes through a `tokio::spawn` in the server
    // adapter, so we poll briefly for it to flush.
    let mut found = false;
    for _ in 0..20 {
        let activity = persistence::list_project_activity(&fixture.db, &pid, 100)
            .await
            .unwrap();
        if activity.len() > before_count
            && activity
                .iter()
                .any(|entry| entry.kind == "lifecycle_writeback_failed")
        {
            found = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        found,
        "lifecycle_writeback_failed Activity was recorded after re-enable"
    );

    drop(fixture.project_dir);
}
