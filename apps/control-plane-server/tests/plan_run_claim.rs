//! Issue #42: Planning Phase selects one eligible Ready Issue and claims it
//! as a nested Issue Assignment from the Plan Run baseline.

use agentic_afk_contracts::{
    CreateProjectRequest, EnableIssueSourceRequest, IssueSource, PlanRunResponse, ProjectResponse,
    ProjectSnapshotResponse, SetProjectExecutionConfigRequest, SourceIssueSnapshot,
};
use agentic_afk_control_plane_server::{
    AssignmentWorktreeProvisioner, ControlPlaneConfig, FakeLifecycleWriter,
    FakePlanningPhaseRunner, FakeWorktreeProvisioner, IssueLifecycleWriter, PlanRunPhaseError,
    PlanningPhaseRunner, RefreshedBaseline, StaticIntegrationBranchRefresher,
    router_with_plan_run_full_deps,
};
use agentic_afk_persistence as persistence;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde::de::DeserializeOwned;
use std::path::PathBuf;
use std::sync::Arc;
use tower::ServiceExt;

fn temp_project_path(name: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "agentic-afk-claim-{name}-{}-{nonce}",
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

fn issue(source_id: &str, lifecycle: &str, deps: Vec<&str>) -> SourceIssueSnapshot {
    SourceIssueSnapshot {
        source_id: source_id.into(),
        title: format!("Issue {source_id}"),
        readiness: "ready".into(),
        lifecycle_status: lifecycle.into(),
        parent_issue: None,
        issue_dependencies: deps.into_iter().map(String::from).collect(),
        source_order: 0,
        raw_text: format!("body of {source_id}"),
    }
}

struct Fixture {
    router: axum::Router,
    db: persistence::Db,
    project: ProjectResponse,
    worktree: Arc<FakeWorktreeProvisioner>,
    lifecycle: Arc<FakeLifecycleWriter>,
}

async fn build_fixture(
    planner_stdout: &str,
    eligible: Vec<SourceIssueSnapshot>,
) -> Fixture {
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();

    let worktree = Arc::new(FakeWorktreeProvisioner::new(
        std::env::temp_dir().join("agentic-afk-claim-test"),
    ));
    let lifecycle = Arc::new(FakeLifecycleWriter::new());

    let router = router_with_plan_run_full_deps(
        config(),
        db.clone(),
        Arc::new(StaticIntegrationBranchRefresher::new(RefreshedBaseline {
            commit_sha: "baseline-sha".into(),
        })),
        Arc::new(FakePlanningPhaseRunner::with_stdout(planner_stdout)),
        worktree.clone() as Arc<dyn AssignmentWorktreeProvisioner>,
        lifecycle.clone() as Arc<dyn IssueLifecycleWriter>,
    );

    // Create + trust project, enable Issue Source, set execution config.
    let dir = temp_project_path("p");
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

    let _ = router
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

    // Seed planning snapshot with the given eligible issues.
    let source = IssueSource {
        kind: "github".into(),
        locator: "owner/repo".into(),
    };
    persistence::replace_planning_snapshot(&db, &pid, &source, &eligible, "unix:1")
        .await
        .unwrap();

    let _ = router
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

    // Re-fetch the project so the test sees the enabled Issue Source.
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
        worktree,
        lifecycle,
    }
}

async fn post_plan_run(router: &axum::Router, pid: &str) -> axum::response::Response {
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
async fn planning_phase_claims_one_eligible_issue() {
    let fixture = build_fixture(
        r#"<plan>{"issues":[{"source_issue_id":"42","title":"Plan and claim","branch":"agent/issue-42","selection_summary":"baseline ready"}],"summary":"one ready"}</plan>"#,
        vec![issue("42", "ready", vec![])],
    )
    .await;
    let pid = fixture.project.id.0.clone();

    let resp = post_plan_run(&fixture.router, &pid).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let run: PlanRunResponse = read_json(resp).await;
    // With #45 the default deps also drive the accepting Merge Phase, so
    // the Plan Run finishes as `succeeded` and the assignment is `merged`.
    assert_eq!(run.state, "succeeded", "Plan Run completes after merge");
    assert_eq!(run.assignments.len(), 1);
    let assignment = &run.assignments[0];
    assert_eq!(assignment.source_id, "42");
    assert_eq!(assignment.branch, "agent/issue-42");
    assert_eq!(assignment.status, "merged");
    assert_eq!(assignment.plan_run_id.as_deref(), Some(run.id.as_str()));
    assert_eq!(
        assignment.selection_summary.as_deref(),
        Some("baseline ready")
    );
    assert!(
        !assignment.worktree_path.is_empty(),
        "worktree path stamped on claim"
    );

    // Worktree provisioner saw the Plan Run baseline commit, not a refreshed one.
    let calls = fixture.worktree.calls();
    assert_eq!(calls.len(), 1);
    let (project_path, baseline, branch) = &calls[0];
    assert_eq!(project_path, std::path::Path::new(&fixture.project.path));
    assert_eq!(baseline, "baseline-sha");
    assert_eq!(branch, "agent/issue-42");

    // Lifecycle writer saw the Claimed write before any implementation pass.
    let calls = fixture.lifecycle.calls();
    assert_eq!(
        calls.first().cloned(),
        Some((
            "42".to_string(),
            agentic_afk_orchestrator::LifecycleStatus::Claimed
        )),
        "Claimed write is the first Lifecycle write for the assignment, got {calls:?}",
    );
}

#[tokio::test]
async fn planner_selection_outside_eligible_set_fails_plan_run() {
    let fixture = build_fixture(
        r#"<plan>{"issues":[{"source_issue_id":"999","title":"ghost","branch":"agent/issue-999","selection_summary":"not in snapshot"}],"summary":"oops"}</plan>"#,
        vec![issue("42", "ready", vec![])],
    )
    .await;
    let pid = fixture.project.id.0.clone();

    let resp = post_plan_run(&fixture.router, &pid).await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = read_text(resp).await;
    assert!(
        body.contains("urn:agentic-afk:planning-selection-ineligible"),
        "unexpected body: {body}"
    );

    // No worktree provisioned, no lifecycle write.
    assert!(fixture.worktree.calls().is_empty());
    assert!(fixture.lifecycle.calls().is_empty());

    // Plan Run finished as failed with a recorded failed planning phase output.
    let runs = persistence::list_recent_plan_runs(&fixture.db, &pid, 10)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].state, "failed");
    assert_eq!(runs[0].phase_outputs[0].outcome, "failed");
    assert!(runs[0].assignments.is_empty());
}

#[tokio::test]
async fn lifecycle_write_failure_releases_provisional_assignment() {
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let worktree = Arc::new(FakeWorktreeProvisioner::new(
        std::env::temp_dir().join("agentic-afk-claim-test"),
    ));
    let lifecycle = Arc::new(FakeLifecycleWriter::failing("gh down"));
    let router = router_with_plan_run_full_deps(
        config(),
        db.clone(),
        Arc::new(StaticIntegrationBranchRefresher::new(RefreshedBaseline {
            commit_sha: "baseline-sha".into(),
        })),
        Arc::new(FakePlanningPhaseRunner::with_stdout(
            r#"<plan>{"issues":[{"source_issue_id":"42","title":"x","branch":"agent/issue-42","selection_summary":"ok"}],"summary":"s"}</plan>"#,
        )),
        worktree.clone() as Arc<dyn AssignmentWorktreeProvisioner>,
        lifecycle.clone() as Arc<dyn IssueLifecycleWriter>,
    );
    let dir = temp_project_path("lc");
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
    persistence::replace_planning_snapshot(&db, &pid, &source, &[issue("42", "ready", vec![])], "unix:1")
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

    let resp = post_plan_run(&router, &pid).await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = read_text(resp).await;
    assert!(
        body.contains("urn:agentic-afk:issue-source-lifecycle-failed"),
        "unexpected body: {body}"
    );

    // Provisional assignment was released, so the Plan Run shows no
    // assignments and is recorded as failed.
    let runs = persistence::list_recent_plan_runs(&db, &pid, 10)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].state, "failed");
    assert!(runs[0].assignments.is_empty());
}

#[tokio::test]
async fn snapshot_exposes_claimed_assignment_inside_active_plan_run() {
    let fixture = build_fixture(
        r#"<plan>{"issues":[{"source_issue_id":"42","title":"t","branch":"agent/issue-42","selection_summary":"go"}],"summary":"s"}</plan>"#,
        vec![issue("42", "ready", vec![])],
    )
    .await;
    let pid = fixture.project.id.0.clone();

    let _ = post_plan_run(&fixture.router, &pid).await;

    let snapshot: ProjectSnapshotResponse = read_json(
        fixture
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
            .unwrap(),
    )
    .await;

    // With #45 the merge phase finishes the Plan Run; the active slot
    // clears and the run lives in recent history with the merged
    // assignment attached.
    assert!(snapshot.snapshot.active_plan_run.is_none());
    let recent = &snapshot.snapshot.recent_plan_runs;
    assert_eq!(recent.len(), 1);
    let recent_run = &recent[0];
    assert_eq!(recent_run.state, "succeeded");
    assert_eq!(recent_run.assignments.len(), 1);
    assert_eq!(recent_run.assignments[0].source_id, "42");
    assert_eq!(recent_run.assignments[0].status, "merged");

    // Planning snapshot remains visible alongside the active Plan Run.
    let planning = snapshot
        .snapshot
        .planning_snapshot
        .as_ref()
        .expect("planning snapshot present");
    assert!(
        planning
            .eligible
            .iter()
            .any(|issue| issue.source_id == "42")
    );
}

#[tokio::test]
async fn planning_prompt_lists_only_eligible_source_issues() {
    let planner = Arc::new(FakePlanningPhaseRunner::with_stdout(
        r#"<plan>{"issues":[],"summary":"none"}</plan>"#,
    ));
    let planner_dyn: Arc<dyn PlanningPhaseRunner> = planner.clone();
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let router = router_with_plan_run_full_deps(
        config(),
        db.clone(),
        Arc::new(StaticIntegrationBranchRefresher::new(RefreshedBaseline {
            commit_sha: "baseline-sha".into(),
        })),
        planner_dyn,
        Arc::new(FakeWorktreeProvisioner::new(std::env::temp_dir())),
        Arc::new(FakeLifecycleWriter::new()),
    );
    let dir = temp_project_path("prompt");
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
    persistence::replace_planning_snapshot(
        &db,
        &pid,
        &source,
        &[
            issue("42", "ready", vec![]),
            // blocked issue: dependency on a ready issue still in the snapshot.
            issue("43", "ready", vec!["42"]),
            // claimed issue: already active, excluded from eligible set.
            issue("44", "claimed", vec![]),
        ],
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

    let _ = post_plan_run(&router, &pid).await;
    let prompt = planner.last_prompt().expect("planner called");
    assert!(prompt.contains("source_id: 42"), "{prompt}");
    assert!(
        !prompt.contains("source_id: 43"),
        "blocked issue must not appear: {prompt}"
    );
    assert!(
        !prompt.contains("source_id: 44"),
        "active issue must not appear: {prompt}"
    );
}

#[tokio::test]
async fn worktree_provision_failure_releases_provisional_assignment() {
    struct FailingWorktree;
    impl AssignmentWorktreeProvisioner for FailingWorktree {
        fn provision(
            &self,
            _project_path: &std::path::Path,
            _baseline_commit: &str,
            _branch: &str,
        ) -> Result<PathBuf, PlanRunPhaseError> {
            Err(PlanRunPhaseError::WorktreeProvision(
                "worktrunk missing".into(),
            ))
        }
    }

    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let lifecycle = Arc::new(FakeLifecycleWriter::new());
    let router = router_with_plan_run_full_deps(
        config(),
        db.clone(),
        Arc::new(StaticIntegrationBranchRefresher::new(RefreshedBaseline {
            commit_sha: "baseline-sha".into(),
        })),
        Arc::new(FakePlanningPhaseRunner::with_stdout(
            r#"<plan>{"issues":[{"source_issue_id":"42","title":"t","branch":"agent/issue-42","selection_summary":"ok"}],"summary":"s"}</plan>"#,
        )),
        Arc::new(FailingWorktree),
        lifecycle.clone() as Arc<dyn IssueLifecycleWriter>,
    );
    let dir = temp_project_path("wf");
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
    persistence::replace_planning_snapshot(&db, &pid, &source, &[issue("42", "ready", vec![])], "unix:1")
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

    let resp = post_plan_run(&router, &pid).await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = read_text(resp).await;
    assert!(
        body.contains("urn:agentic-afk:assignment-worktree-failed"),
        "unexpected body: {body}"
    );

    // Lifecycle write must not have been attempted before the worktree was ready.
    assert!(lifecycle.calls().is_empty());

    let runs = persistence::list_recent_plan_runs(&db, &pid, 10)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].state, "failed");
    assert!(runs[0].assignments.is_empty());
}
