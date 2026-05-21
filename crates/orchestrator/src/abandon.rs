//! Worktree + branch removal for abandoned Issue Assignments.

use std::path::Path;
use std::process::Command;

/// Remove the Assignment Worktree and deterministic branch through Worktrunk so
/// abandonment is the approved lifecycle cleanup path.
///
/// The Project path is the location the Assignment Worktree was originally
/// created from. Worktrunk owns the worktree-and-branch lifecycle, so the
/// command runs in that Project repository.
pub fn remove_assignment_worktree(
    worktrunk_binary_path: &Path,
    project_path: &Path,
    branch: &str,
) -> Result<(), String> {
    let output = Command::new(worktrunk_binary_path)
        .current_dir(project_path)
        .args(["remove", "--branch", branch, "--yes", "--force"])
        .output()
        .map_err(|error| format!("failed to start Worktrunk cleanup: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(command_failure(
            "Worktrunk assignment worktree removal",
            &output,
        ))
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
