//! Verification adapters for GitHub Change Proposals.
//!
//! Owns required-check inspection, Human Merge detection from the proposal host,
//! and accepted Assignment Worktree + branch cleanup.

use serde_json::Value;
use std::path::Path;
use std::process::Command;

/// State of the required GitHub checks on a Change Proposal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CheckState {
    /// At least one required check is still in progress.
    Pending,
    /// All required checks passed.
    Passing,
    /// At least one required check failed; payload describes which.
    Failing(String),
}

/// Extract the pull request number from a GitHub PR URL.
///
/// Accepts both `https://github.com/owner/repo/pull/NNN` and `gh`-style URLs.
pub fn parse_pull_request_number(url: &str) -> Option<u64> {
    url.rsplit('/').find_map(|segment| segment.parse().ok())
}

/// Inspect required checks for a pull request via `gh pr checks --json`.
///
/// `gh pr checks` exits non-zero when at least one check is failing or
/// pending; we still parse stdout to distinguish those two cases.
pub fn inspect_required_checks(
    gh_binary_path: &Path,
    locator: &str,
    pull_request_number: u64,
) -> Result<CheckState, String> {
    let output = Command::new(gh_binary_path)
        .args([
            "pr",
            "checks",
            &pull_request_number.to_string(),
            "--repo",
            locator,
            "--json",
            "name,state,bucket",
        ])
        .output()
        .map_err(|error| format!("failed to run gh pr checks: {error}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let parsed: Vec<Value> = if stdout.is_empty() {
        Vec::new()
    } else {
        serde_json::from_str(&stdout)
            .map_err(|error| format!("failed to parse gh pr checks output: {error}"))?
    };

    if parsed.is_empty() && !output.status.success() {
        return Err(format!(
            "gh pr checks exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let mut any_pending = false;
    let mut failing_names: Vec<String> = Vec::new();
    for check in &parsed {
        let bucket = check
            .get("bucket")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        let state = check
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_uppercase();
        let name = check
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("required check")
            .to_string();
        let pending = matches!(
            state.as_str(),
            "IN_PROGRESS" | "QUEUED" | "PENDING" | "WAITING"
        ) || matches!(bucket.as_str(), "pending");
        let passed = matches!(state.as_str(), "SUCCESS" | "NEUTRAL" | "SKIPPED")
            || matches!(bucket.as_str(), "pass");
        if pending {
            any_pending = true;
        } else if !passed {
            failing_names.push(name);
        }
    }

    if !failing_names.is_empty() {
        let detail = format!(
            "required GitHub check failing: {}",
            failing_names.join(", ")
        );
        return Ok(CheckState::Failing(detail));
    }
    if any_pending {
        return Ok(CheckState::Pending);
    }
    if parsed.is_empty() {
        // No required checks defined: treat as pending so verification waits
        // for explicit human action rather than auto-verifying empty CI.
        return Ok(CheckState::Pending);
    }
    Ok(CheckState::Passing)
}

/// Detect whether a pull request was Human Merged via `gh pr view --json state,merged`.
pub fn is_pull_request_merged(
    gh_binary_path: &Path,
    locator: &str,
    pull_request_number: u64,
) -> Result<bool, String> {
    let output = Command::new(gh_binary_path)
        .args([
            "pr",
            "view",
            &pull_request_number.to_string(),
            "--repo",
            locator,
            "--json",
            "state,mergedAt,mergeCommit",
        ])
        .output()
        .map_err(|error| format!("failed to run gh pr view: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "gh pr view exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let parsed: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("failed to parse gh pr view output: {error}"))?;
    let state = parsed
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_uppercase();
    if state == "MERGED" {
        return Ok(true);
    }
    Ok(parsed
        .get("mergedAt")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty()))
}

/// Remove the accepted Assignment Worktree and its deterministic branch via Worktrunk.
///
/// Best-effort: Worktrunk is asked to remove the worktree, then the deterministic
/// branch is deleted from the project repository. Failures are returned so the
/// caller can surface them, but they do not roll back the Completed lifecycle.
pub fn cleanup_assignment_worktree(
    worktrunk_binary_path: &Path,
    project_path: &Path,
    branch: &str,
) -> Result<(), String> {
    let output = Command::new(worktrunk_binary_path)
        .current_dir(project_path)
        .args(["remove", branch, "--yes"])
        .output()
        .map_err(|error| format!("failed to remove Assignment Worktree: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Worktrunk worktree removal exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    // Delete the deterministic branch locally; ignore missing branches.
    let _ = Command::new("git")
        .current_dir(project_path)
        .args(["branch", "-D", branch])
        .output();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pull_request_numbers_from_urls() {
        assert_eq!(
            parse_pull_request_number("https://github.com/owner/repo/pull/42"),
            Some(42)
        );
        assert_eq!(parse_pull_request_number("42"), Some(42));
        assert_eq!(parse_pull_request_number(""), None);
    }
}
