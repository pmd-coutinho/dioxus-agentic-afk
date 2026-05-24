//! Process adapters owned by the Orchestrator boundary.

use agentic_afk_contracts::AssignmentTerminalOutcome;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

pub mod auto_replan;
pub mod boot_container_sweeper;
pub mod boot_recovery_scanner;
pub mod codex_runner;
pub mod coordinator;
pub mod implementation_phase;
pub mod in_flight_phase_tracker;
pub mod merge_phase;
pub mod plan_run;
pub mod plan_run_finalize;
pub mod plan_run_status;
pub mod planning_phase;
pub mod production;
pub mod push_attempt;
pub mod re_enable_source_issue;
pub mod review_loop;
pub mod sandbox;
pub mod shutdown_coordinator;

pub use codex_runner::DockerCodexRunner;
pub use sandbox::{
    AlwaysOkSandboxPreflight, BuilderImageEnsurer, CliDockerProbe, DockerProbe,
    DockerSandboxLauncher, FakeSandboxLauncher, MISE_CACHE_VOLUME, RUNTIME_IMAGE_REPO,
    RecordedLaunch, RejectingSandboxPreflight, RuntimeImageBuilder, RuntimeImageEnsurer,
    SandboxError, SandboxFailureTemplate, SandboxLaunchSpec, SandboxLauncher, SandboxMount,
    SandboxPhase, SandboxPreflight, SandboxPreflightCheck, SandboxPreflightFailure,
    runtime_image_tag,
};

pub use auto_replan::{
    AutoReplanCurrent, AutoReplanDecision, AutoReplanDriver, CycleOutcome, CycleTrigger,
    SyncOutcome, classify_plan_run_for_auto_replan,
};
pub use coordinator::{
    CoordinatorError, EventPublisher, PlanRunDeps, PlanRunEffects, PlanRunInputs,
    SandboxProductionConfig, abandon_staged, retry_push, run_plan_run,
    update_markdown_lifecycle_status,
};
pub use planning_phase::{PlannedClaim, PlanningRejection, render_planning_prompt};
pub use push_attempt::{PushOutcome, classify_push_result};
pub use re_enable_source_issue::{ReEnableOutcome, WritebackError, re_enable_source_issue};

pub use plan_run_status::{AssignmentStatus, transition_assignment};

pub use production::{
    GhLifecycleWriter, GitAssignmentWorktreeCleaner, GitIntegrationBranchPusher,
    GitIntegrationBranchRefresher, WorktrunkAssignmentWorktreeProvisioner,
};

pub use plan_run::{
    AssignmentWorktreeCleaner, AssignmentWorktreeProvisioner, FakeAssignmentWorktreeCleaner,
    FakeImplementationPhaseRunner, FakeIntegrationBranchPusher, FakeLifecycleWriter,
    FakeMergePhaseRunner, FakePlanningPhaseRunner, FakePushOutcome, FakeReviewPhaseRunner,
    FakeWorktreeProvisioner, ImplementationPhaseRunner, IntegrationBranchPusher,
    IntegrationBranchRefresher, IssueLifecycleWriter, LifecycleStatus, MergePhaseRunner,
    ParsedImplementationOutput, ParsedMergeOutput, ParsedPlanningOutput, ParsedReviewOutput,
    PerSourceImplementationPhaseRunner, PerSourceMergePhaseRunner, PerSourceReviewPhaseRunner,
    PlanRunPhaseError, PlannerSelection, PlanningPhaseRunner, RefreshedBaseline, ReviewPhaseRunner,
    StaticIntegrationBranchRefresher, UnimplementedAssignmentWorktreeCleaner,
    UnimplementedImplementationPhaseRunner, UnimplementedIntegrationBranchPusher,
    UnimplementedIntegrationBranchRefresher, UnimplementedLifecycleWriter,
    UnimplementedMergePhaseRunner, UnimplementedPlanningPhaseRunner,
    UnimplementedReviewPhaseRunner, UnimplementedWorktreeProvisioner, extract_planner_selections,
    parse_implementation_output, parse_merge_output, parse_planning_output, parse_review_output,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexExecution {
    pub process_id: u32,
    pub process_identity: Option<String>,
    pub terminal_outcome: AssignmentTerminalOutcome,
}

pub fn preflight_binary(binary_path: &Path, name: &str) -> Result<(), String> {
    let output = Command::new(binary_path)
        .arg("--version")
        .output()
        .map_err(|error| format!("failed to run {name} preflight: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("{name} preflight exited with {}", output.status))
    }
}

pub fn codex_process_identity(process_id: u32) -> Option<String> {
    let stat =
        std::fs::read_to_string(Path::new("/proc").join(process_id.to_string()).join("stat"))
            .ok()?;
    let process_start_time = stat.rsplit_once(") ")?.1.split_whitespace().nth(19)?;
    Some(format!("procfs-start-time:{process_start_time}"))
}

pub fn create_assignment_worktree(
    worktrunk_binary_path: &Path,
    project_path: &Path,
    branch: &str,
) -> Result<PathBuf, String> {
    let output = run_worktrunk_switch(worktrunk_binary_path, project_path, branch, true)?;
    if !output.status.success() {
        if worktrunk_branch_exists(branch, &output) {
            let reuse_output =
                run_worktrunk_switch(worktrunk_binary_path, project_path, branch, false)?;
            return parse_worktrunk_path("Worktrunk assignment worktree switch", &reuse_output);
        }
        return Err(command_failure(
            "Worktrunk assignment worktree creation",
            &output,
        ));
    }

    parse_worktrunk_path("Worktrunk assignment worktree creation", &output)
}

fn run_worktrunk_switch(
    worktrunk_binary_path: &Path,
    project_path: &Path,
    branch: &str,
    create: bool,
) -> Result<std::process::Output, String> {
    let mut command = Command::new(worktrunk_binary_path);
    command.current_dir(project_path).arg("switch");
    if create {
        command.arg("--create");
    }
    command.args([branch, "--yes", "--no-cd", "--format", "json"]);
    command
        .output()
        .map_err(|error| format!("failed to start Worktrunk: {error}"))
}

fn worktrunk_branch_exists(branch: &str, output: &std::process::Output) -> bool {
    let stderr = String::from_utf8_lossy(&output.stderr);
    stderr.contains("already exists") && stderr.contains(branch)
}

fn parse_worktrunk_path(label: &str, output: &std::process::Output) -> Result<PathBuf, String> {
    if !output.status.success() {
        return Err(command_failure(label, output));
    }
    let json: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("failed to parse Worktrunk worktree output: {error}"))?;
    find_path_value(&json)
        .map(PathBuf::from)
        .ok_or_else(|| "Worktrunk worktree output did not include a path".to_string())
}

// Host-only `run_codex_exec`, `run_initial_codex`, and the
// `terminal_outcome_schema` helper were removed by issue #76. Every
// Codex execution now runs inside a Codex Sandbox container driven by
// `DockerCodexRunner`.

fn find_path_value(value: &Value) -> Option<&str> {
    match value {
        Value::Object(values) => values
            .get("path")
            .or_else(|| values.get("worktree_path"))
            .and_then(Value::as_str)
            .or_else(|| values.values().find_map(find_path_value)),
        Value::Array(values) => values.iter().find_map(find_path_value),
        _ => None,
    }
}

fn command_failure(label: &str, output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        format!("{label} exited with {}", output.status)
    } else {
        format!("{label} exited with {}: {stderr}", output.status)
    }
}

#[cfg(test)]
mod tests {
    use super::create_assignment_worktree;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn create_assignment_worktree_reuses_existing_branch() {
        let fixture = WorktrunkFixture::new("reuse");
        fixture.write_fake_worktrunk(
            r#"#!/bin/sh
printf '%s\n' "$@" >> "__CALLS__"
if [ "$2" = "--create" ]; then
  echo 'Branch agent/issue-79 already exists' >&2
  echo 'To switch to the existing branch, run without --create: wt switch agent/issue-79' >&2
  exit 1
fi
printf '{"path":"/tmp/reused-agent-issue-79"}'
"#,
        );

        let path = create_assignment_worktree(
            fixture.worktrunk_path(),
            fixture.project_path(),
            "agent/issue-79",
        )
        .expect("existing branch should be reused");

        assert_eq!(path, PathBuf::from("/tmp/reused-agent-issue-79"));
        let calls = fs::read_to_string(fixture.calls_path()).unwrap();
        assert!(calls.contains("switch\n--create\nagent/issue-79"));
        assert!(calls.contains("switch\nagent/issue-79\n--yes"));
    }

    #[test]
    fn create_assignment_worktree_does_not_retry_unrelated_failure() {
        let fixture = WorktrunkFixture::new("fatal");
        fixture.write_fake_worktrunk(
            r#"#!/bin/sh
printf '%s\n' "$@" >> "__CALLS__"
echo 'docker is unavailable' >&2
exit 1
"#,
        );

        let error = create_assignment_worktree(
            fixture.worktrunk_path(),
            fixture.project_path(),
            "agent/issue-79",
        )
        .expect_err("unrelated failures should remain fatal");

        assert!(error.contains("docker is unavailable"));
        let calls = fs::read_to_string(fixture.calls_path()).unwrap();
        assert_eq!(calls.matches("switch").count(), 1);
    }

    struct WorktrunkFixture {
        root: PathBuf,
        project: PathBuf,
        worktrunk: PathBuf,
        calls: PathBuf,
    }

    impl WorktrunkFixture {
        fn new(name: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "agentic-afk-worktrunk-{name}-{}-{nanos}",
                std::process::id()
            ));
            let project = root.join("project");
            fs::create_dir_all(&project).unwrap();
            let worktrunk = root.join("wt");
            let calls = root.join("calls");
            Self {
                root,
                project,
                worktrunk,
                calls,
            }
        }

        fn write_fake_worktrunk(&self, body: &str) {
            let script = body.replace("__CALLS__", &self.calls.display().to_string());
            fs::write(&self.worktrunk, script).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut permissions = fs::metadata(&self.worktrunk).unwrap().permissions();
                permissions.set_mode(0o755);
                fs::set_permissions(&self.worktrunk, permissions).unwrap();
            }
        }

        fn worktrunk_path(&self) -> &Path {
            &self.worktrunk
        }

        fn project_path(&self) -> &Path {
            &self.project
        }

        fn calls_path(&self) -> &Path {
            &self.calls
        }
    }

    impl Drop for WorktrunkFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
