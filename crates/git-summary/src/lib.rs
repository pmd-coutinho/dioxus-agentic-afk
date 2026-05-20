//! Read-only Git Summary boundary for Projects.

use std::path::Path;

use agentic_afk_contracts::GitSummary;

pub fn summarize_project_path(path: impl AsRef<Path>) -> Option<GitSummary> {
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

    Some(GitSummary {
        branch,
        head,
        dirty,
    })
}
