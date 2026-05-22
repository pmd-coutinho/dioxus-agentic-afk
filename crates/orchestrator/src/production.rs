//! Real (non-test) implementations of the Plan Run phase seams.
//!
//! These adapters wrap the existing process helpers (`git`, `gh`,
//! `worktrunk`, `codex`) behind the trait surface the Plan Run coordinator
//! expects. Tests still inject fakes; production wires these adapters via
//! `PlanRunDeps::production`.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::plan_run::{
    AssignmentWorktreeCleaner, AssignmentWorktreeProvisioner, ImplementationPhaseRunner,
    IntegrationBranchPusher, IntegrationBranchRefresher, IssueLifecycleWriter, MergePhaseRunner,
    PlanRunPhaseError, PlanningPhaseRunner, RefreshedBaseline, ReviewPhaseRunner,
};
use crate::create_assignment_worktree;

fn command_output(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        format!("exit status {}", output.status)
    } else {
        format!("exit status {}: {stderr}", output.status)
    }
}

/// Real Integration Branch refresher: `git fetch origin <branch>` then
/// `git checkout <branch>` + `git pull --ff-only` from the project path.
/// Reports the resulting HEAD commit as the Plan Run baseline.
pub struct GitIntegrationBranchRefresher;

impl IntegrationBranchRefresher for GitIntegrationBranchRefresher {
    fn refresh(
        &self,
        project_path: &Path,
        integration_branch: &str,
    ) -> Result<RefreshedBaseline, PlanRunPhaseError> {
        let fetch = Command::new("git")
            .current_dir(project_path)
            .args(["fetch", "origin", integration_branch])
            .output()
            .map_err(|e| PlanRunPhaseError::Refresh(format!("git fetch failed: {e}")))?;
        if !fetch.status.success() {
            return Err(PlanRunPhaseError::Refresh(format!(
                "git fetch origin {integration_branch}: {}",
                command_output(&fetch)
            )));
        }
        let checkout = Command::new("git")
            .current_dir(project_path)
            .args(["checkout", integration_branch])
            .output()
            .map_err(|e| PlanRunPhaseError::Refresh(format!("git checkout failed: {e}")))?;
        if !checkout.status.success() {
            return Err(PlanRunPhaseError::Refresh(format!(
                "git checkout {integration_branch}: {}",
                command_output(&checkout)
            )));
        }
        let pull = Command::new("git")
            .current_dir(project_path)
            .args(["pull", "--ff-only", "origin", integration_branch])
            .output()
            .map_err(|e| PlanRunPhaseError::Refresh(format!("git pull failed: {e}")))?;
        if !pull.status.success() {
            return Err(PlanRunPhaseError::Refresh(format!(
                "git pull --ff-only origin {integration_branch}: {}",
                command_output(&pull)
            )));
        }
        let head = Command::new("git")
            .current_dir(project_path)
            .args(["rev-parse", "HEAD"])
            .output()
            .map_err(|e| PlanRunPhaseError::Refresh(format!("git rev-parse failed: {e}")))?;
        if !head.status.success() {
            return Err(PlanRunPhaseError::Refresh(format!(
                "git rev-parse HEAD: {}",
                command_output(&head)
            )));
        }
        let commit_sha = String::from_utf8_lossy(&head.stdout).trim().to_string();
        Ok(RefreshedBaseline { commit_sha })
    }
}

/// Real Integration Branch pusher: `git push origin <branch>` from the
/// project path. Surfaces stderr on failure so the developer can diagnose
/// upstream rejections (e.g. permission denied, non-fast-forward).
pub struct GitIntegrationBranchPusher;

impl IntegrationBranchPusher for GitIntegrationBranchPusher {
    fn push(
        &self,
        project_path: &Path,
        integration_branch: &str,
    ) -> Result<(), PlanRunPhaseError> {
        let output = Command::new("git")
            .current_dir(project_path)
            .args(["push", "origin", integration_branch])
            .output()
            .map_err(|e| {
                PlanRunPhaseError::IntegrationPush(format!("git push failed to spawn: {e}"))
            })?;
        if !output.status.success() {
            return Err(PlanRunPhaseError::IntegrationPush(format!(
                "git push origin {integration_branch}: {}",
                command_output(&output)
            )));
        }
        Ok(())
    }
}

/// Real Assignment Worktree provisioner: delegates to the existing
/// `create_assignment_worktree` helper that drives Worktrunk.
pub struct WorktrunkAssignmentWorktreeProvisioner {
    worktrunk_binary_path: PathBuf,
}

impl WorktrunkAssignmentWorktreeProvisioner {
    pub fn new(worktrunk_binary_path: impl Into<PathBuf>) -> Self {
        Self {
            worktrunk_binary_path: worktrunk_binary_path.into(),
        }
    }
}

impl AssignmentWorktreeProvisioner for WorktrunkAssignmentWorktreeProvisioner {
    fn provision(
        &self,
        project_path: &Path,
        _baseline_commit: &str,
        branch: &str,
    ) -> Result<PathBuf, PlanRunPhaseError> {
        create_assignment_worktree(&self.worktrunk_binary_path, project_path, branch)
            .map_err(PlanRunPhaseError::WorktreeProvision)
    }
}

/// Real Assignment Worktree cleaner: `git worktree remove --force <path>`
/// followed by `git branch -D <branch>`. Both calls are best-effort and the
/// surrounding coordinator already treats cleanup failure as a warning.
pub struct GitAssignmentWorktreeCleaner;

impl AssignmentWorktreeCleaner for GitAssignmentWorktreeCleaner {
    fn cleanup(
        &self,
        project_path: &Path,
        worktree_path: &Path,
        branch: &str,
    ) -> Result<(), PlanRunPhaseError> {
        let remove = Command::new("git")
            .current_dir(project_path)
            .args([
                "worktree",
                "remove",
                "--force",
                &worktree_path.to_string_lossy(),
            ])
            .output()
            .map_err(|e| {
                PlanRunPhaseError::Cleanup(format!("git worktree remove failed to spawn: {e}"))
            })?;
        if !remove.status.success() {
            return Err(PlanRunPhaseError::Cleanup(format!(
                "git worktree remove --force {}: {}",
                worktree_path.display(),
                command_output(&remove)
            )));
        }
        let delete = Command::new("git")
            .current_dir(project_path)
            .args(["branch", "-D", branch])
            .output()
            .map_err(|e| {
                PlanRunPhaseError::Cleanup(format!("git branch -D failed to spawn: {e}"))
            })?;
        if !delete.status.success() {
            // Branch may have been pruned by worktree remove; surface as
            // a soft error so the coordinator can log it as a warning.
            return Err(PlanRunPhaseError::Cleanup(format!(
                "git branch -D {branch}: {}",
                command_output(&delete)
            )));
        }
        Ok(())
    }
}

/// Issue Source kind passed to the production lifecycle writer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LifecycleSourceKind {
    Github,
    LocalMarkdown,
}

/// Real Issue Source lifecycle writer for the `Claimed` write-back the
/// Plan Run coordinator emits when a planner selection is durably
/// claimed. The merge / blocked write-backs go through the existing
/// `write_assignment_lifecycle` helper in the control-plane-server, which
/// already handles both Issue Source kinds; this adapter handles the
/// `Claimed` boundary the orchestrator owns directly.
pub struct GhLifecycleWriter {
    pub gh_binary_path: PathBuf,
    pub source_kind: LifecycleSourceKind,
    pub source_locator: String,
    pub project_path: PathBuf,
}

impl IssueLifecycleWriter for GhLifecycleWriter {
    fn write_claimed(&self, source_id: &str) -> Result<(), PlanRunPhaseError> {
        match self.source_kind {
            LifecycleSourceKind::Github => write_github_claimed_label(
                &self.gh_binary_path,
                &self.source_locator,
                source_id,
            ),
            LifecycleSourceKind::LocalMarkdown => write_local_markdown_claimed(
                &self.project_path,
                &self.source_locator,
                source_id,
            ),
        }
    }
}

fn write_github_claimed_label(
    gh_binary_path: &Path,
    locator: &str,
    source_id: &str,
) -> Result<(), PlanRunPhaseError> {
    let output = Command::new(gh_binary_path)
        .args([
            "issue",
            "edit",
            source_id,
            "--repo",
            locator,
            "--add-label",
            "agentic-afk:claimed",
            "--remove-label",
            "agentic-afk:ready",
        ])
        .output()
        .map_err(|e| {
            PlanRunPhaseError::LifecycleWrite(format!("gh issue edit failed to spawn: {e}"))
        })?;
    if !output.status.success() {
        return Err(PlanRunPhaseError::LifecycleWrite(format!(
            "gh issue edit {source_id} on {locator}: {}",
            command_output(&output)
        )));
    }
    Ok(())
}

fn write_local_markdown_claimed(
    project_path: &Path,
    relative_dir: &str,
    source_id: &str,
) -> Result<(), PlanRunPhaseError> {
    // Local markdown Issue Source: rewrite the issue file's frontmatter
    // `lifecycle_status` field to `claimed`. Best-effort: if the file is
    // missing or has no frontmatter, surface as a lifecycle-write error
    // for the coordinator to record.
    let issue_path = project_path
        .join(relative_dir)
        .join(format!("{source_id}.md"));
    let text = std::fs::read_to_string(&issue_path).map_err(|e| {
        PlanRunPhaseError::LifecycleWrite(format!(
            "failed to read local markdown issue {}: {e}",
            issue_path.display()
        ))
    })?;
    let rewritten = update_frontmatter_status(&text, "claimed").ok_or_else(|| {
        PlanRunPhaseError::LifecycleWrite(format!(
            "local markdown issue {} has no frontmatter to update",
            issue_path.display()
        ))
    })?;
    std::fs::write(&issue_path, rewritten).map_err(|e| {
        PlanRunPhaseError::LifecycleWrite(format!(
            "failed to write local markdown issue {}: {e}",
            issue_path.display()
        ))
    })?;
    Ok(())
}

fn update_frontmatter_status(text: &str, new_status: &str) -> Option<String> {
    // Expect a leading `---\n...\n---\n` frontmatter block. Replace any
    // existing `lifecycle_status:` line; otherwise insert one at the end
    // of the frontmatter.
    let rest = text.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    let frontmatter = &rest[..end];
    let body = &rest[end + 4..]; // skip "\n---"
    let mut found = false;
    let mut new_fm = Vec::new();
    for line in frontmatter.lines() {
        if line.starts_with("lifecycle_status:") {
            new_fm.push(format!("lifecycle_status: {new_status}"));
            found = true;
        } else {
            new_fm.push(line.to_string());
        }
    }
    if !found {
        new_fm.push(format!("lifecycle_status: {new_status}"));
    }
    Some(format!("---\n{}\n---{}", new_fm.join("\n"), body))
}

// --- Codex-driven phase runners ---
//
// Each phase runner spawns `codex exec` with the rendered prompt and
// captures the structured terminal outcome. The Plan Run coordinator
// parses the agent stdout itself; these adapters only return the raw
// output string so the parsing contract stays in one place.

/// Codex `--output-last-message` does not stream stdout to the parent
/// process, so spawning the agent through `run_codex_exec` would lose the
/// `<plan>` / `<impl>` / `<review>` / `<merge>` tagged body the
/// coordinator parses. Until the agent gains a structured plan-run
/// terminal output, the production runners exec `codex exec` directly and
/// capture stdout themselves.
fn run_codex_capture_stdout(
    codex_binary_path: &Path,
    project_path: &Path,
    prompt: &str,
) -> Result<String, String> {
    let output = Command::new(codex_binary_path)
        .current_dir(project_path)
        .args(["exec", "--dangerously-bypass-approvals-and-sandbox"])
        .arg(prompt)
        .output()
        .map_err(|e| format!("failed to spawn codex: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "codex exec failed: {}",
            command_output(&output)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Production planner: runs `codex exec` from the project path and
/// returns raw stdout.
pub struct CodexPlanningPhaseRunner {
    codex_binary_path: PathBuf,
    project_path: PathBuf,
}

impl CodexPlanningPhaseRunner {
    pub fn new(codex_binary_path: impl Into<PathBuf>, project_path: impl Into<PathBuf>) -> Self {
        Self {
            codex_binary_path: codex_binary_path.into(),
            project_path: project_path.into(),
        }
    }
}

impl PlanningPhaseRunner for CodexPlanningPhaseRunner {
    fn run(&self, prompt: &str) -> Result<String, PlanRunPhaseError> {
        run_codex_capture_stdout(&self.codex_binary_path, &self.project_path, prompt)
            .map_err(PlanRunPhaseError::Planning)
    }
}

/// Production implementation runner: runs `codex exec` from the project
/// path (the worktree path is encoded in the prompt; the agent executes
/// against the worktree per its instructions).
pub struct CodexImplementationPhaseRunner {
    codex_binary_path: PathBuf,
    project_path: PathBuf,
}

impl CodexImplementationPhaseRunner {
    pub fn new(codex_binary_path: impl Into<PathBuf>, project_path: impl Into<PathBuf>) -> Self {
        Self {
            codex_binary_path: codex_binary_path.into(),
            project_path: project_path.into(),
        }
    }
}

impl ImplementationPhaseRunner for CodexImplementationPhaseRunner {
    fn run(&self, prompt: &str) -> Result<String, PlanRunPhaseError> {
        run_codex_capture_stdout(&self.codex_binary_path, &self.project_path, prompt)
            .map_err(PlanRunPhaseError::Planning)
    }
}

/// Production review runner.
pub struct CodexReviewPhaseRunner {
    codex_binary_path: PathBuf,
    project_path: PathBuf,
}

impl CodexReviewPhaseRunner {
    pub fn new(codex_binary_path: impl Into<PathBuf>, project_path: impl Into<PathBuf>) -> Self {
        Self {
            codex_binary_path: codex_binary_path.into(),
            project_path: project_path.into(),
        }
    }
}

impl ReviewPhaseRunner for CodexReviewPhaseRunner {
    fn run(&self, prompt: &str) -> Result<String, PlanRunPhaseError> {
        run_codex_capture_stdout(&self.codex_binary_path, &self.project_path, prompt)
            .map_err(PlanRunPhaseError::Planning)
    }
}

/// Production merge runner.
pub struct CodexMergePhaseRunner {
    codex_binary_path: PathBuf,
    project_path: PathBuf,
}

impl CodexMergePhaseRunner {
    pub fn new(codex_binary_path: impl Into<PathBuf>, project_path: impl Into<PathBuf>) -> Self {
        Self {
            codex_binary_path: codex_binary_path.into(),
            project_path: project_path.into(),
        }
    }
}

impl MergePhaseRunner for CodexMergePhaseRunner {
    fn run(&self, prompt: &str) -> Result<String, PlanRunPhaseError> {
        run_codex_capture_stdout(&self.codex_binary_path, &self.project_path, prompt)
            .map_err(PlanRunPhaseError::Merge)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_frontmatter_status_replaces_existing_field() {
        let text = "---\ntitle: Foo\nlifecycle_status: ready\n---\nbody\n";
        let updated = update_frontmatter_status(text, "claimed").unwrap();
        assert!(updated.contains("lifecycle_status: claimed"));
        assert!(updated.contains("body"));
        assert!(!updated.contains("lifecycle_status: ready"));
    }

    #[test]
    fn update_frontmatter_status_inserts_when_missing() {
        let text = "---\ntitle: Foo\n---\nbody\n";
        let updated = update_frontmatter_status(text, "claimed").unwrap();
        assert!(updated.contains("lifecycle_status: claimed"));
    }

    #[test]
    fn update_frontmatter_status_returns_none_without_frontmatter() {
        assert!(update_frontmatter_status("no frontmatter here", "claimed").is_none());
    }
}
