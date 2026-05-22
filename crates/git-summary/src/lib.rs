//! Read-only Git Summary boundary for Projects.

use std::path::Path;
use std::process::Command;

use agentic_afk_contracts::GitSummary;

pub fn summarize_project_path(path: impl AsRef<Path>) -> Option<GitSummary> {
    let path = path.as_ref();
    let repo = gix::discover(path).ok()?;
    let head = repo.head().ok()?;

    let branch = head.referent_name().map(|name| {
        let name = name.as_bstr().to_string();
        name.strip_prefix("refs/heads/")
            .unwrap_or(&name)
            .to_string()
    });
    let head = head.id().map(|id| id.to_string());
    let dirty = repo.is_dirty().unwrap_or(false);
    let default_branch = detect_default_branch(path);

    Some(GitSummary {
        branch,
        head,
        dirty,
        default_branch,
    })
}

/// Detect the Project's default branch from `refs/remotes/origin/HEAD`.
/// Returns `None` when origin HEAD is not configured (e.g. local-only
/// repo, fresh clone before fetch, etc.); callers can fall back to a
/// platform default like `"main"`.
pub fn detect_default_branch(project_path: &Path) -> Option<String> {
    let output = Command::new("git")
        .current_dir(project_path)
        .args(["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        return None;
    }
    // `origin/main` -> `main`.
    Some(text.strip_prefix("origin/").unwrap_or(&text).to_string())
}
