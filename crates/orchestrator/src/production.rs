//! Real (non-test) implementations of the Plan Run phase seams.
//!
//! These adapters wrap the existing process helpers (`git`, `gh`,
//! `worktrunk`, `codex`) behind the trait surface the Plan Run coordinator
//! expects. Tests still inject fakes; production wires these adapters via
//! `PlanRunDeps::production`.

use std::path::{Path, PathBuf};
use std::process::Command;

use agentic_afk_contracts::{IssueSource, ProjectResponse};

use crate::create_assignment_worktree;
use crate::plan_run::{
    AssignmentWorktreeCleaner, AssignmentWorktreeProvisioner, IntegrationBranchPusher,
    IntegrationBranchRefresher, IssueLifecycleWriter, PlanRunPhaseError, RefreshedBaseline,
};

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
    fn push(&self, project_path: &Path, integration_branch: &str) -> Result<(), PlanRunPhaseError> {
        let output = Command::new("git")
            .current_dir(project_path)
            .args(["push", "origin", integration_branch])
            .output()
            .map_err(|e| {
                PlanRunPhaseError::IntegrationPush(format!("git push failed to spawn: {e}"))
            })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            if is_non_fast_forward_stderr(&stderr) {
                return Err(PlanRunPhaseError::NonFastForward { stderr });
            }
            return Err(PlanRunPhaseError::IntegrationPush(format!(
                "git push origin {integration_branch}: {}",
                command_output(&output)
            )));
        }
        Ok(())
    }
}

/// Heuristic detector for `git push` non-fast-forward rejection text.
/// Matches the same canonical phrases as
/// [`crate::push_attempt::classify_push_result`] so the production
/// pusher and the classifier share one taxonomy.
fn is_non_fast_forward_stderr(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    ["non-fast-forward", "non fast forward", "fetch first"]
        .iter()
        .any(|needle| lower.contains(needle))
        || lower.contains("would not be a fast-forward")
        || lower.contains("would not be a fast forward")
        || lower.contains("updates were rejected")
        || lower.contains("tip of your current branch is behind")
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

/// Real Issue Source lifecycle writer for the `Claimed` write-back the
/// Plan Run coordinator emits when a planner selection is durably claimed.
/// Delegates to the canonical `write_assignment_lifecycle` helper in
/// `coordinator.rs` so the `Claimed` write-back uses the exact same label
/// scheme (`agentic-afk:claimed`) and local-markdown body format
/// (`Lifecycle Status: claimed`) as the later blocked / completed
/// write-backs.
pub struct GhLifecycleWriter {
    pub gh_binary_path: PathBuf,
    pub project: ProjectResponse,
    pub source: IssueSource,
}

impl GhLifecycleWriter {
    /// Construct a lifecycle writer from a Project + its enabled Issue
    /// Source. Used by the Plan Run coordinator to wire production deps
    /// per Plan Run once the project context is known.
    pub fn for_project(
        gh_binary_path: impl Into<PathBuf>,
        project: ProjectResponse,
        source: IssueSource,
    ) -> Self {
        Self {
            gh_binary_path: gh_binary_path.into(),
            project,
            source,
        }
    }
}

impl IssueLifecycleWriter for GhLifecycleWriter {
    fn write(
        &self,
        source_id: &str,
        status: crate::plan_run::LifecycleStatus,
    ) -> Result<(), PlanRunPhaseError> {
        crate::coordinator::write_assignment_lifecycle(
            &self.gh_binary_path,
            &self.project,
            &self.source,
            source_id,
            status.as_str(),
        )
        .map_err(PlanRunPhaseError::LifecycleWrite)
    }
}

// Codex phase runners now live in [`crate::codex_runner`] as
// `DockerCodexRunner`. The host-only `Codex*PhaseRunner` adapters and
// `run_codex_capture_stdout` that used to live here were removed by
// issue #76; every Codex phase now runs inside a Codex Sandbox
// container.

#[cfg(test)]
mod tests {
    use super::*;

    fn project_with_path(path: &Path) -> ProjectResponse {
        ProjectResponse {
            id: agentic_afk_contracts::ProjectId("test-project".to_string()),
            path: path.to_string_lossy().into_owned(),
            git_summary: None,
            trusted: true,
            enabled_issue_source: None,
            auto_replan_state: agentic_afk_contracts::AutoReplanState::Off,
            auto_replan_pause_reason: None,
        }
    }

    fn git_init_user_config(path: &Path) {
        // Local repo identity is required for `git commit` to succeed in
        // sandbox environments where the global git config may be absent.
        for (key, val) in [("user.email", "test@example.com"), ("user.name", "Test")] {
            assert!(
                Command::new("git")
                    .current_dir(path)
                    .args(["config", key, val])
                    .status()
                    .unwrap()
                    .success()
            );
        }
    }

    fn git(path: &Path, args: &[&str]) -> std::process::Output {
        Command::new("git")
            .current_dir(path)
            .args(args)
            .output()
            .expect("git command")
    }

    fn unique_dir(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "agentic-afk-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn git_integration_branch_pusher_returns_non_fast_forward_on_diverged_remote() {
        // Issue #63: production pusher must detect a non-fast-forward
        // rejection from `git push` and return the typed `NonFastForward`
        // variant (carrying the stderr) rather than the generic
        // `IntegrationPush(String)`.
        let root = unique_dir("ibp-nonff");
        std::fs::create_dir_all(&root).unwrap();

        let remote = root.join("remote.git");
        let local = root.join("local");
        let other = root.join("other");

        // Bare remote.
        assert!(
            git(
                &root,
                &["init", "--bare", "-b", "main", remote.to_str().unwrap()]
            )
            .status
            .success()
        );

        // Local clone, commit, push so the branch exists upstream.
        assert!(
            git(
                &root,
                &["clone", remote.to_str().unwrap(), local.to_str().unwrap()],
            )
            .status
            .success()
        );
        git_init_user_config(&local);
        std::fs::write(local.join("README.md"), "hello\n").unwrap();
        assert!(git(&local, &["add", "."]).status.success());
        assert!(git(&local, &["commit", "-m", "base"]).status.success());
        assert!(
            git(&local, &["push", "-u", "origin", "main"])
                .status
                .success()
        );

        // Second clone advances `main` so the remote diverges from `local`.
        assert!(
            git(
                &root,
                &["clone", remote.to_str().unwrap(), other.to_str().unwrap()],
            )
            .status
            .success()
        );
        git_init_user_config(&other);
        std::fs::write(other.join("OTHER.md"), "other\n").unwrap();
        assert!(git(&other, &["add", "."]).status.success());
        assert!(
            git(&other, &["commit", "-m", "other-commit"])
                .status
                .success()
        );
        assert!(git(&other, &["push", "origin", "main"]).status.success());

        // Local makes a divergent commit (does NOT pull first).
        std::fs::write(local.join("LOCAL.md"), "local\n").unwrap();
        assert!(git(&local, &["add", "."]).status.success());
        assert!(
            git(&local, &["commit", "-m", "local-commit"])
                .status
                .success()
        );

        // Drive the production pusher; expect `NonFastForward`.
        let pusher = GitIntegrationBranchPusher;
        let result = pusher.push(&local, "main");
        let cleanup = || {
            let _ = std::fs::remove_dir_all(&root);
        };
        match result {
            Err(PlanRunPhaseError::NonFastForward { stderr }) => {
                let lower = stderr.to_ascii_lowercase();
                assert!(
                    lower.contains("non-fast-forward")
                        || lower.contains("fetch first")
                        || lower.contains("rejected"),
                    "stderr should carry remote rejection text, got: {stderr}"
                );
                cleanup();
            }
            other => {
                cleanup();
                panic!("expected NonFastForward, got {other:?}");
            }
        }
    }

    #[test]
    fn git_integration_branch_pusher_returns_integration_push_on_unreachable_remote() {
        // Issue #63: failures that are NOT non-fast-forward (here: an
        // unreachable remote URL) must still report through the generic
        // `IntegrationPush(String)` variant so the classifier routes them
        // as `PushOutcome::Other` and the operator can retry.
        let root = unique_dir("ibp-unreachable");
        let local = root.join("local");
        std::fs::create_dir_all(&local).unwrap();

        assert!(git(&local, &["init", "-b", "main"]).status.success());
        git_init_user_config(&local);
        std::fs::write(local.join("README.md"), "hi\n").unwrap();
        assert!(git(&local, &["add", "."]).status.success());
        assert!(git(&local, &["commit", "-m", "init"]).status.success());
        // Point `origin` at a definitely-unreachable URL.
        assert!(
            git(
                &local,
                &[
                    "remote",
                    "add",
                    "origin",
                    "file:///nonexistent/agentic-afk-unreachable.git",
                ],
            )
            .status
            .success()
        );

        let pusher = GitIntegrationBranchPusher;
        let result = pusher.push(&local, "main");
        let cleanup = || {
            let _ = std::fs::remove_dir_all(&root);
        };
        match result {
            Err(PlanRunPhaseError::IntegrationPush(detail)) => {
                assert!(!detail.is_empty());
                cleanup();
            }
            other => {
                cleanup();
                panic!("expected IntegrationPush, got {other:?}");
            }
        }
    }

    #[test]
    fn gh_lifecycle_writer_writes_local_markdown_claimed() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let project_path = std::env::temp_dir().join(format!(
            "agentic-afk-lifecycle-{}-{nonce}",
            std::process::id()
        ));
        let issues_dir = project_path.join("issues");
        std::fs::create_dir_all(&issues_dir).unwrap();
        let issue_path = issues_dir.join("42.md");
        std::fs::write(
            &issue_path,
            "# 42 — Sample issue\n\nLifecycle Status: ready\n\nbody\n",
        )
        .unwrap();

        let project = project_with_path(&project_path);
        let source = IssueSource {
            kind: "local_markdown".to_string(),
            locator: "issues".to_string(),
        };
        let writer = GhLifecycleWriter::for_project(PathBuf::from("gh"), project, source);
        let result = writer.write("42", crate::plan_run::LifecycleStatus::Claimed);

        let updated = std::fs::read_to_string(&issue_path).unwrap();
        let _ = std::fs::remove_dir_all(&project_path);
        result.expect("local markdown claimed write");
        assert!(updated.contains("Lifecycle Status: claimed"));
        assert!(!updated.contains("Lifecycle Status: ready"));
    }
}
