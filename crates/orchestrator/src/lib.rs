//! Process adapters owned by the Orchestrator boundary.

use agentic_afk_contracts::AssignmentTerminalOutcome;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub mod plan_run;
pub mod production;

pub use production::{
    CodexImplementationPhaseRunner, CodexMergePhaseRunner, CodexPlanningPhaseRunner,
    CodexReviewPhaseRunner, GhLifecycleWriter, GitAssignmentWorktreeCleaner,
    GitIntegrationBranchPusher, GitIntegrationBranchRefresher, LifecycleSourceKind,
    WorktrunkAssignmentWorktreeProvisioner,
};

pub use plan_run::{
    AssignmentWorktreeCleaner, AssignmentWorktreeProvisioner, FakeAssignmentWorktreeCleaner,
    FakeImplementationPhaseRunner, FakeIntegrationBranchPusher, FakeLifecycleWriter,
    FakeMergePhaseRunner, FakePlanningPhaseRunner, FakeReviewPhaseRunner,
    FakeWorktreeProvisioner, ImplementationPhaseRunner, IntegrationBranchPusher,
    IntegrationBranchRefresher, IssueLifecycleWriter, MergePhaseRunner,
    ParsedImplementationOutput, ParsedMergeOutput, ParsedPlanningOutput, ParsedReviewOutput,
    PerSourceImplementationPhaseRunner, PerSourceMergePhaseRunner, PerSourceReviewPhaseRunner,
    PlanRunPhaseError, PlannerSelection, PlanningPhaseRunner, RefreshedBaseline,
    ReviewPhaseRunner, StaticIntegrationBranchRefresher,
    UnimplementedAssignmentWorktreeCleaner, UnimplementedImplementationPhaseRunner,
    UnimplementedIntegrationBranchPusher, UnimplementedIntegrationBranchRefresher,
    UnimplementedLifecycleWriter, UnimplementedMergePhaseRunner,
    UnimplementedPlanningPhaseRunner, UnimplementedReviewPhaseRunner,
    UnimplementedWorktreeProvisioner, extract_planner_selections, parse_implementation_output,
    parse_merge_output, parse_planning_output, parse_review_output,
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
    let output = Command::new(worktrunk_binary_path)
        .current_dir(project_path)
        .args([
            "switch", "--create", branch, "--yes", "--no-cd", "--format", "json",
        ])
        .output()
        .map_err(|error| format!("failed to start Worktrunk: {error}"))?;
    if !output.status.success() {
        return Err(command_failure(
            "Worktrunk assignment worktree creation",
            &output,
        ));
    }

    let json: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("failed to parse Worktrunk worktree output: {error}"))?;
    find_path_value(&json)
        .map(PathBuf::from)
        .ok_or_else(|| "Worktrunk worktree output did not include a path".to_string())
}

pub fn run_initial_codex(
    codex_binary_path: &Path,
    worktree_path: &Path,
    prompt: &str,
) -> Result<CodexExecution, String> {
    run_codex_exec(codex_binary_path, worktree_path, prompt)
}

/// Shared `codex exec` invocation used by Plan Run Assignment Attempts
/// (initial implementation, Review Loop re-implementation, review, and
/// merge passes).
pub fn run_codex_exec(
    codex_binary_path: &Path,
    worktree_path: &Path,
    prompt: &str,
) -> Result<CodexExecution, String> {
    codex_exec_impl(codex_binary_path, worktree_path, prompt)
}

fn codex_exec_impl(
    codex_binary_path: &Path,
    worktree_path: &Path,
    prompt: &str,
) -> Result<CodexExecution, String> {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let output_path = std::env::temp_dir().join(format!(
        "agentic-afk-codex-outcome-{}-{nonce}.json",
        std::process::id()
    ));
    let schema_path = std::env::temp_dir().join(format!(
        "agentic-afk-codex-schema-{}-{nonce}.json",
        std::process::id()
    ));
    std::fs::write(&schema_path, terminal_outcome_schema())
        .map_err(|error| format!("failed to write Codex outcome schema: {error}"))?;

    let child = Command::new(codex_binary_path)
        .current_dir(worktree_path)
        .args([
            "exec",
            "--dangerously-bypass-approvals-and-sandbox",
            "--output-schema",
        ])
        .arg(&schema_path)
        .arg("--output-last-message")
        .arg(&output_path)
        .arg(prompt)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to spawn Codex: {error}"))?;
    let process_id = child.id();
    let process_identity = codex_process_identity(process_id);
    let output = child
        .wait_with_output()
        .map_err(|error| format!("failed to wait for Codex: {error}"))?;
    let _ = std::fs::remove_file(&schema_path);

    if !output.status.success() {
        let _ = std::fs::remove_file(&output_path);
        return Err(command_failure("Codex exec", &output));
    }

    let outcome_json = std::fs::read_to_string(&output_path)
        .map_err(|error| format!("failed to read Codex terminal outcome: {error}"))?;
    let _ = std::fs::remove_file(&output_path);
    let terminal_outcome = serde_json::from_str(&outcome_json)
        .map_err(|error| format!("failed to parse Codex terminal outcome: {error}"))?;
    Ok(CodexExecution {
        process_id,
        process_identity,
        terminal_outcome,
    })
}

fn terminal_outcome_schema() -> &'static str {
    r#"{
  "type": "object",
  "additionalProperties": false,
  "required": ["outcome", "summary"],
  "properties": {
    "outcome": { "type": "string", "enum": ["ReadyForReview", "Blocked", "Failed"] },
    "summary": { "type": "string" }
  }
}"#
}

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
