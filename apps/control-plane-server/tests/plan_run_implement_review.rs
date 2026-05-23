//! Issue #43: implementation + approving review for one claimed Issue
//! Assignment inside its Plan Run.

use agentic_afk_contracts::{
    CreateProjectRequest, EnableIssueSourceRequest, IssueSource, PlanRunResponse, ProjectResponse,
    SetProjectExecutionConfigRequest, SourceIssueSnapshot,
};
use agentic_afk_control_plane_server::{
    AssignmentWorktreeProvisioner, ControlPlaneConfig, FakeImplementationPhaseRunner,
    FakeLifecycleWriter, FakePlanningPhaseRunner, FakeReviewPhaseRunner, FakeWorktreeProvisioner,
    ImplementationPhaseRunner, IssueLifecycleWriter, PlanRunPhaseError, PlanningPhaseRunner,
    RefreshedBaseline, ReviewPhaseRunner, StaticIntegrationBranchRefresher,
    router_with_plan_run_all_deps,
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
        "agentic-afk-impl-review-{label}-{}-{nonce}",
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
    impl_stdout: &str,
    review_stdout: &str,
    project_instructions: Option<&str>,
) -> Fixture {
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();

    let impl_runner = Arc::new(FakeImplementationPhaseRunner::with_stdout(impl_stdout));
    let review_runner = Arc::new(FakeReviewPhaseRunner::with_stdout(review_stdout));

    let router = router_with_plan_run_all_deps(
        config(),
        db.clone(),
        Arc::new(StaticIntegrationBranchRefresher::new(RefreshedBaseline {
            commit_sha: "baseline-sha".into(),
        })),
        Arc::new(FakePlanningPhaseRunner::with_stdout(
            r#"<plan>{"issues":[{"source_issue_id":"42","title":"t","branch":"agent/issue-42","selection_summary":"ok"}],"summary":"s"}</plan>"#,
        )),
        Arc::new(FakeWorktreeProvisioner::new(std::env::temp_dir())) as Arc<dyn AssignmentWorktreeProvisioner>,
        Arc::new(FakeLifecycleWriter::new()) as Arc<dyn IssueLifecycleWriter>,
        impl_runner.clone() as Arc<dyn ImplementationPhaseRunner>,
        review_runner.clone() as Arc<dyn ReviewPhaseRunner>,
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

#[tokio::test]
async fn approving_review_takes_assignment_to_reviewed() {
    let fixture = build_fixture(
        r#"<impl>{"outcome":"ready_for_review","summary":"shipped","commits":["abc"],"verification":["cargo test"],"gaps":[]}</impl>"#,
        r#"<review>{"outcome":"approved","findings":[],"summary":"lgtm","verification":["cargo test"],"gaps":[]}</review>"#,
        None,
    )
    .await;
    let pid = fixture.project.id.0.clone();
    let resp = start(&fixture.router, &pid).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let run: PlanRunResponse = read_json(resp).await;
    assert_eq!(run.assignments.len(), 1);
    let assignment = &run.assignments[0];
    // With #45 the approving review now drives an accepting Merge Phase
    // through the default merge fake, so the assignment reaches `merged`.
    assert_eq!(assignment.status, "merged");

    // Implementation + review + merge phase outputs persisted on the assignment in order.
    assert_eq!(assignment.phase_outputs.len(), 3);
    assert_eq!(assignment.phase_outputs[0].phase, "implementation");
    assert_eq!(assignment.phase_outputs[0].outcome, "ready_for_review");
    assert_eq!(
        assignment.phase_outputs[0].assignment_id.as_deref(),
        Some(assignment.id.as_str())
    );
    assert_eq!(assignment.phase_outputs[1].phase, "review");
    assert_eq!(assignment.phase_outputs[1].outcome, "approved");
    assert_eq!(
        assignment.phase_outputs[1].body_json["summary"]
            .as_str()
            .unwrap(),
        "lgtm"
    );
    assert_eq!(assignment.phase_outputs[2].phase, "merge");
    assert_eq!(assignment.phase_outputs[2].outcome, "merged");

    // Plan Run's phase_outputs list includes planning + impl + review + merge + push.
    // ADR-0038 / issue #53: the Integration Branch push records its own
    // `push`-typed Phase Output scoped to the Plan Run.
    let phases: Vec<&str> = run.phase_outputs.iter().map(|p| p.phase.as_str()).collect();
    assert_eq!(
        phases,
        vec!["planning", "implementation", "review", "merge", "push"]
    );
}

#[tokio::test]
async fn implementation_prompt_carries_project_instructions_and_source_brief() {
    let fixture = build_fixture(
        r#"<impl>{"outcome":"ready_for_review","summary":"s","commits":[],"verification":[],"gaps":[]}</impl>"#,
        r#"<review>{"outcome":"approved","findings":[],"summary":"lgtm","verification":[],"gaps":[]}</review>"#,
        Some("# Project Instructions\nNever bypass hooks."),
    )
    .await;
    let pid = fixture.project.id.0.clone();
    let _ = start(&fixture.router, &pid).await;
    let prompt = fixture
        .impl_runner
        .last_prompt()
        .expect("implementation runner called");
    assert!(prompt.contains("Never bypass hooks."), "{prompt}");
    assert!(
        prompt.contains("issue brief body for 42"),
        "implementation prompt must include the raw Source Issue brief: {prompt}"
    );
    assert!(prompt.contains("Plan Run Baseline: baseline-sha"), "{prompt}");
    assert!(prompt.contains("Issue Branch: agent/issue-42"), "{prompt}");
    // Ensure no proposal-era outcome words slip in by mistake.
    assert!(
        !prompt.contains("ReadyForProposal"),
        "implementation prompt must not assume proposal-era outcomes: {prompt}"
    );
    drop(fixture.project_dir);
}

#[tokio::test]
async fn review_prompt_includes_implementation_output_and_project_instructions() {
    let fixture = build_fixture(
        r#"<impl>{"outcome":"ready_for_review","summary":"impl summary text","commits":[],"verification":[],"gaps":[]}</impl>"#,
        r#"<review>{"outcome":"approved","findings":[],"summary":"lgtm","verification":[],"gaps":[]}</review>"#,
        Some("Rule: respect repo conventions."),
    )
    .await;
    let pid = fixture.project.id.0.clone();
    let _ = start(&fixture.router, &pid).await;
    let prompt = fixture
        .review_runner
        .last_prompt()
        .expect("review runner called");
    assert!(prompt.contains("Rule: respect repo conventions."), "{prompt}");
    assert!(prompt.contains("impl summary text"), "{prompt}");
    assert!(prompt.contains("Source Issue: 42"), "{prompt}");
}

#[tokio::test]
async fn unparseable_implementation_output_blocks_the_assignment() {
    let fixture = build_fixture(
        "not a structured output",
        r#"<review>{"outcome":"approved","findings":[],"summary":"lgtm","verification":[],"gaps":[]}</review>"#,
        None,
    )
    .await;
    let pid = fixture.project.id.0.clone();
    let resp = start(&fixture.router, &pid).await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = read_text(resp).await;
    assert!(
        body.contains("urn:agentic-afk:implementation-output-unparseable"),
        "unexpected body: {body}"
    );
    let runs = persistence::list_recent_plan_runs(&fixture.db, &pid, 10)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].state, "failed");
    assert_eq!(runs[0].assignments.len(), 1);
    assert_eq!(runs[0].assignments[0].status, "blocked");
}

#[tokio::test]
async fn rejected_review_with_exhausted_retry_limit_blocks_assignment_and_fails_plan_run() {
    // The Review Retry Limit defaults to 1 in `build_fixture`, so a single
    // rejection exhausts the Review Loop (issue #44) and blocks the
    // assignment instead of returning HTTP 500.
    let fixture = build_fixture(
        r#"<impl>{"outcome":"ready_for_review","summary":"s","commits":[],"verification":[],"gaps":[]}</impl>"#,
        r#"<review>{"outcome":"rejected","findings":["missing tests"],"summary":"needs more","verification":[],"gaps":[]}</review>"#,
        None,
    )
    .await;
    let pid = fixture.project.id.0.clone();
    let resp = start(&fixture.router, &pid).await;
    // Blocked Plan Run surfaces as the canonical CREATED outcome so the
    // Dashboard can render review-retry state without a 500 round trip.
    assert_eq!(resp.status(), StatusCode::CREATED);
    let runs = persistence::list_recent_plan_runs(&fixture.db, &pid, 10)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].state, "failed");
    assert_eq!(runs[0].assignments[0].status, "blocked");
    assert_eq!(runs[0].assignments[0].review_rejection_count, 1);
    assert!(runs[0].assignments[0].block_reason.is_some());
    // Review Phase Output is still persisted as durable Review Loop evidence.
    assert!(
        runs[0]
            .assignments[0]
            .phase_outputs
            .iter()
            .any(|p| p.phase == "review" && p.outcome == "rejected")
    );
}

#[tokio::test]
async fn implementation_runner_failure_blocks_assignment_and_skips_review() {
    struct FailingImpl;
    impl ImplementationPhaseRunner for FailingImpl {
        fn run(&self, _prompt: &str) -> Result<String, PlanRunPhaseError> {
            Err(PlanRunPhaseError::Planning("codex impl broke".into()))
        }
    }
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let review_runner = Arc::new(FakeReviewPhaseRunner::with_stdout(
        r#"<review>{"outcome":"approved","findings":[],"summary":"never","verification":[],"gaps":[]}</review>"#,
    ));
    let router = router_with_plan_run_all_deps(
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
        Arc::new(FailingImpl),
        review_runner.clone(),
    );
    let dir = temp_dir("impl-fail");
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

    let resp = start(&router, &pid).await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = read_text(resp).await;
    assert!(
        body.contains("urn:agentic-afk:implementation-phase-failed"),
        "unexpected body: {body}"
    );
    // Review runner must not have been invoked.
    assert!(review_runner.last_prompt().is_none());
}
