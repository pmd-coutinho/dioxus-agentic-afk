//! Issue #74: per-phase launch shape for `DockerCodexRunner`.
//!
//! Assertions:
//! - Planning launches bind-mount the Project path read-only at `/work`.
//! - Implementation / Review / Merge launches bind-mount the Assignment
//!   Worktree read-write at `/work`.
//! - All launches carry the five `agentic-afk.*` labels and the
//!   auth/config/cache mounts.
//! - Each phase trait impl maps launcher errors to the correct
//!   `PlanRunPhaseError` variant.
//! - Stdout returned by the launcher is the stdout returned by the
//!   runner trait method.

use std::path::PathBuf;
use std::sync::Arc;

use agentic_afk_contracts::{
    IssueAssignmentResponse, PlanRunResponse, PlanRunState, ProjectId, ProjectResponse,
};
use agentic_afk_orchestrator::plan_run::{AssignmentContext, PlanningContext};
use agentic_afk_orchestrator::{
    DockerCodexRunner, FakeSandboxLauncher, ImplementationPhaseRunner, MergePhaseRunner,
    PlanRunPhaseError, PlanningPhaseRunner, ReviewPhaseRunner, SandboxError, SandboxMount,
    SandboxPhase,
};

fn project(path: &str) -> ProjectResponse {
    ProjectResponse {
        id: ProjectId("proj-42".to_string()),
        path: path.to_string(),
        git_summary: None,
        trusted: true,
        enabled_issue_source: None,
        auto_replan_state: agentic_afk_contracts::AutoReplanState::Off,
        auto_replan_pause_reason: None,
    }
}

fn plan_run() -> PlanRunResponse {
    PlanRunResponse {
        id: "plan-run-7".to_string(),
        project_id: ProjectId("proj-42".to_string()),
        integration_branch: "main".to_string(),
        baseline_commit: "deadbeef".to_string(),
        state: PlanRunState::Running,
        started_at: "2026-05-23T00:00:00Z".to_string(),
        finished_at: None,
        phase_outputs: Vec::new(),
        assignments: Vec::new(),
    }
}

fn assignment(id: &str, worktree: &str) -> IssueAssignmentResponse {
    IssueAssignmentResponse {
        id: id.to_string(),
        project_id: ProjectId("proj-42".to_string()),
        source_id: "issue-1".to_string(),
        source_title: "title".to_string(),
        branch: "agent/issue-1".to_string(),
        worktree_path: worktree.to_string(),
        status: "implementing".to_string(),
        status_detail: None,
        latest_attempt: None,
        plan_run_id: Some("plan-run-7".to_string()),
        selection_summary: None,
        phase_outputs: Vec::new(),
        review_rejection_count: 0,
        block_reason: None,
    }
}

fn make_runner(launcher: Arc<FakeSandboxLauncher>, phase: SandboxPhase) -> DockerCodexRunner {
    DockerCodexRunner::new(
        launcher as Arc<dyn agentic_afk_orchestrator::SandboxLauncher>,
        phase,
        "agentic-afk-runtime:000000000000".to_string(),
        PathBuf::from("/host/proj"),
        PathBuf::from("/home/dev/.codex/auth.json"),
        PathBuf::from("/home/dev/.codex/config.toml"),
        Some((1000, 1000)),
    )
}

fn bind_mount(mounts: &[SandboxMount], host: &str) -> Option<SandboxMount> {
    mounts
        .iter()
        .find(|m| matches!(m, SandboxMount::Bind { host_path, .. } if host_path.to_string_lossy() == host))
        .cloned()
}

#[test]
fn planning_launch_mounts_project_read_only_and_returns_stdout() {
    let launcher = Arc::new(FakeSandboxLauncher::with_stdout(
        r#"<plan>{"issues":[]}</plan>"#,
    ));
    let runner = make_runner(launcher.clone(), SandboxPhase::Planning);

    let project = project("/host/proj");
    let plan_run = plan_run();
    let ctx = PlanningContext {
        project: &project,
        plan_run: &plan_run,
        process_recorder: None,
    };
    let stdout =
        PlanningPhaseRunner::run(&runner, "plan now", &ctx).expect("planning launch succeeds");
    assert!(stdout.contains("<plan>"));

    let launches = launcher.launches();
    assert_eq!(launches.len(), 1);
    let launch = &launches[0];
    assert_eq!(launch.phase, SandboxPhase::Planning);
    assert!(
        launch
            .labels
            .iter()
            .any(|(key, value)| { key == "agentic-afk.plan-run-id" && value == "plan-run-7" })
    );
    assert!(
        launch
            .labels
            .iter()
            .any(|(key, value)| key == "agentic-afk.project-id" && value == "proj-42")
    );
    assert_codex_exec_uses_last_message_capture(&launch.command, "plan now", "gpt-5.5");
    let work_mount = bind_mount(&launch.mounts, "/host/proj").expect("project bind mount");
    match work_mount {
        SandboxMount::Bind {
            container_path,
            read_only,
            ..
        } => {
            assert_eq!(container_path.to_string_lossy(), "/work");
            assert!(read_only, "Planning must bind-mount the Project read-only");
        }
        _ => panic!(),
    }
}

#[test]
fn implementation_launch_mounts_assignment_worktree_read_write() {
    let launcher = Arc::new(FakeSandboxLauncher::with_stdout(
        r#"<impl>{"outcome":"ready_for_review"}</impl>"#,
    ));
    let runner = make_runner(launcher.clone(), SandboxPhase::Implementation);
    let project = project("/host/proj");
    let plan_run = plan_run();
    let assignment = assignment("asgn-1", "/host/worktrees/issue-1");
    let ctx = AssignmentContext {
        project: &project,
        plan_run: &plan_run,
        assignment: &assignment,
        process_recorder: None,
    };
    let stdout = ImplementationPhaseRunner::run(&runner, "implement now", &ctx)
        .expect("implementation launch succeeds");
    assert!(stdout.contains("<impl>"));

    let launch = &launcher.launches()[0];
    assert_eq!(launch.phase, SandboxPhase::Implementation);
    assert_codex_exec_uses_last_message_capture(&launch.command, "implement now", "gpt-5.4");
    let work_mount =
        bind_mount(&launch.mounts, "/host/worktrees/issue-1").expect("worktree bind mount");
    match work_mount {
        SandboxMount::Bind {
            container_path,
            read_only,
            ..
        } => {
            assert_eq!(container_path.to_string_lossy(), "/work");
            assert!(
                !read_only,
                "Implementation must bind-mount the Worktree read-write"
            );
        }
        _ => panic!(),
    }
}

fn assert_codex_exec_uses_last_message_capture(command: &[String], prompt: &str, model: &str) {
    assert_eq!(command.first().map(String::as_str), Some("bash"));
    assert_eq!(command.get(1).map(String::as_str), Some("-lc"));
    let script = command.get(2).expect("bash script present");
    assert!(
        script.contains("--output-last-message"),
        "command must capture only Codex final answer: {script}"
    );
    assert!(
        script.contains(&format!("--model {model}")),
        "command must pin Codex model to {model}: {script}"
    );
    assert!(
        script.contains(r#"model_reasoning_effort="medium""#),
        "command must pin Codex reasoning effort to medium: {script}"
    );
    assert!(
        script.contains(">\"$transcript\""),
        "command must redirect Codex transcript away from stdout: {script}"
    );
    assert_eq!(
        command.last().map(String::as_str),
        Some(prompt),
        "prompt is passed as a shell argument"
    );
}

#[test]
fn review_and_merge_launches_also_mount_worktree_read_write() {
    for phase in [SandboxPhase::Review, SandboxPhase::Merge] {
        let launcher = Arc::new(FakeSandboxLauncher::with_stdout("out"));
        let runner = make_runner(launcher.clone(), phase);
        let project = project("/host/proj");
        let plan_run = plan_run();
        let assignment = assignment("asgn-x", "/host/wt/x");
        let ctx = AssignmentContext {
            project: &project,
            plan_run: &plan_run,
            assignment: &assignment,
            process_recorder: None,
        };
        match phase {
            SandboxPhase::Review => {
                let _ = ReviewPhaseRunner::run(&runner, "review", &ctx);
            }
            SandboxPhase::Merge => {
                let _ = MergePhaseRunner::run(&runner, "merge", &ctx);
            }
            _ => unreachable!(),
        }
        let launch = &launcher.launches()[0];
        assert_eq!(launch.phase, phase);
        assert_codex_exec_uses_last_message_capture(&launch.command, phase.as_label(), "gpt-5.5");
        let work = bind_mount(&launch.mounts, "/host/wt/x").expect("worktree mount");
        match work {
            SandboxMount::Bind { read_only, .. } => assert!(!read_only),
            _ => panic!(),
        }
    }
}

#[test]
fn every_launch_carries_the_five_labels_and_auth_config_cache_mounts() {
    let launcher = Arc::new(FakeSandboxLauncher::with_stdout("ok"));
    let runner = make_runner(launcher.clone(), SandboxPhase::Implementation);
    let project = project("/host/proj");
    let plan_run = plan_run();
    let assignment = assignment("asgn-1", "/host/wt");
    let ctx = AssignmentContext {
        project: &project,
        plan_run: &plan_run,
        assignment: &assignment,
        process_recorder: None,
    };
    let _ = ImplementationPhaseRunner::run(&runner, "p", &ctx);

    let launch = &launcher.launches()[0];
    let label_keys: Vec<&str> = launch.labels.iter().map(|(k, _)| k.as_str()).collect();
    for required in [
        "agentic-afk.plan-run-id",
        "agentic-afk.project-id",
        "agentic-afk.phase",
        "agentic-afk.issue-assignment-id",
        "agentic-afk.assignment-attempt-id",
    ] {
        assert!(
            label_keys.contains(&required),
            "launch missing label {required}"
        );
    }

    // Auth, config, and mise cache mounts present alongside the work mount.
    assert!(
        bind_mount(&launch.mounts, "/home/dev/.codex/auth.json").is_some(),
        "auth bind mount missing"
    );
    assert!(
        bind_mount(&launch.mounts, "/home/dev/.codex/config.toml").is_some(),
        "config bind mount missing"
    );
    assert!(
        launch.mounts.iter().any(
            |m| matches!(m, SandboxMount::Volume { name, .. } if name == "agentic-afk-mise-cache")
        ),
        "mise cache volume mount missing"
    );

    // HOME env always points at the per-container codex home.
    assert!(
        launch
            .env
            .iter()
            .any(|(k, v)| k == "HOME" && v == "/tmp/codex-home")
    );
}

#[test]
fn implementation_runner_error_maps_to_implementation_variant_not_planning() {
    let launcher = Arc::new(FakeSandboxLauncher::with_stdout("").fail_with(
        SandboxError::NonZero {
            status: 1,
            stderr: "codex blew up".to_string(),
        },
    ));
    let runner = make_runner(launcher.clone(), SandboxPhase::Implementation);
    let project = project("/host/proj");
    let plan_run = plan_run();
    let assignment = assignment("asgn-1", "/host/wt");
    let ctx = AssignmentContext {
        project: &project,
        plan_run: &plan_run,
        assignment: &assignment,
        process_recorder: None,
    };
    let err = ImplementationPhaseRunner::run(&runner, "p", &ctx).unwrap_err();
    assert!(
        matches!(err, PlanRunPhaseError::Implementation(_)),
        "expected Implementation, got {err:?}"
    );
}

#[test]
fn review_runner_error_maps_to_review_variant_not_planning() {
    let launcher = Arc::new(FakeSandboxLauncher::with_stdout("").fail_with(
        SandboxError::NonZero {
            status: 1,
            stderr: "codex blew up".to_string(),
        },
    ));
    let runner = make_runner(launcher.clone(), SandboxPhase::Review);
    let project = project("/host/proj");
    let plan_run = plan_run();
    let assignment = assignment("asgn-1", "/host/wt");
    let ctx = AssignmentContext {
        project: &project,
        plan_run: &plan_run,
        assignment: &assignment,
        process_recorder: None,
    };
    let err = ReviewPhaseRunner::run(&runner, "p", &ctx).unwrap_err();
    assert!(
        matches!(err, PlanRunPhaseError::Review(_)),
        "expected Review, got {err:?}"
    );
}

#[test]
fn merge_runner_error_maps_to_merge_variant() {
    let launcher = Arc::new(FakeSandboxLauncher::with_stdout("").fail_with(
        SandboxError::NonZero {
            status: 1,
            stderr: "codex blew up".to_string(),
        },
    ));
    let runner = make_runner(launcher.clone(), SandboxPhase::Merge);
    let project = project("/host/proj");
    let plan_run = plan_run();
    let assignment = assignment("asgn-1", "/host/wt");
    let ctx = AssignmentContext {
        project: &project,
        plan_run: &plan_run,
        assignment: &assignment,
        process_recorder: None,
    };
    let err = MergePhaseRunner::run(&runner, "p", &ctx).unwrap_err();
    assert!(
        matches!(err, PlanRunPhaseError::Merge(_)),
        "expected Merge, got {err:?}"
    );
}
