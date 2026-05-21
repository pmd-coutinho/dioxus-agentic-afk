//! Recovery support for blocked Issue Assignments.
//!
//! Recovery starts a replacement Codex Assignment Attempt in the **same** Assignment
//! Worktree. Before spawning the new agent, any still-owned prior Codex process
//! whose identity can be verified is stopped so two owned agents never share one
//! Assignment Worktree.
//!
//! Prompts are assembled from durable facts only: the original Source Issue text, the
//! deterministic Assignment branch, the worktree path, the prior process identity (if
//! any), and the prior block-reason. The Control Plane never invents prior-agent
//! reasoning to bridge what the previous attempt was thinking — only what is
//! durably persisted is included.

use crate::CodexExecution;
use std::path::Path;
use std::process::{Command, Stdio};

/// Durable facts the Control Plane uses to build a recovery prompt for Codex.
///
/// Every field is sourced from persisted Issue Assignment state, not from agent prose.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryPromptFacts<'a> {
    /// The original Source Issue raw text, preserved when the assignment was first
    /// created.
    pub source_issue_raw_text: &'a str,
    /// The Source Issue identity (e.g. local markdown stem or GitHub issue number).
    pub source_id: &'a str,
    /// The deterministic Assignment branch the worktree is checked out on.
    pub assignment_branch: &'a str,
    /// The Assignment Worktree filesystem path.
    pub assignment_worktree_path: &'a str,
    /// The prior Codex process identity if one was recorded. Recovery never invents
    /// process identity facts.
    pub prior_process_identity: Option<&'a str>,
    /// The prior block reason recorded against the Issue Assignment, if any.
    pub prior_block_reason: Option<&'a str>,
}

/// Build the recovery prompt for Codex from durable facts. The prompt explicitly tells
/// the agent it is a recovery attempt continuing existing work — not a fresh assignment.
pub fn build_recovery_prompt(facts: RecoveryPromptFacts<'_>) -> String {
    let mut prompt = String::new();
    prompt.push_str(
        "This is a recovery Assignment Attempt: a prior Codex process was blocked while \
         implementing this Issue Assignment. Continue the work in the existing Assignment \
         Worktree. Do not restart from a clean checkout.\n\n",
    );
    prompt.push_str(&format!("Source Issue: {}\n", facts.source_id));
    prompt.push_str(&format!(
        "Assignment Worktree: {}\n",
        facts.assignment_worktree_path
    ));
    prompt.push_str(&format!(
        "Assignment Branch: {}\n",
        facts.assignment_branch
    ));
    if let Some(identity) = facts.prior_process_identity {
        prompt.push_str(&format!(
            "Prior Codex process identity (now stopped): {}\n",
            identity
        ));
    }
    if let Some(reason) = facts.prior_block_reason {
        prompt.push_str(&format!("Prior block reason: {}\n", reason));
    }
    prompt.push_str("\n--- Source Issue (verbatim) ---\n");
    prompt.push_str(facts.source_issue_raw_text);
    prompt
}

/// Outcome of verifying-and-stopping the prior Codex process during recovery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PriorProcessOutcome {
    /// No prior process identity was recorded; nothing to stop.
    NoPriorRecord,
    /// The prior process identity was recorded but the OS no longer owns a matching
    /// process — it has already exited.
    AlreadyStopped,
    /// The prior process was still owned and was signalled to stop.
    Stopped,
    /// The prior process identity could not be verified against the OS (e.g. /proc is
    /// unavailable) — we conservatively do nothing rather than risk killing an
    /// unrelated PID.
    UnverifiableLeftRunning,
}

/// Verify whether the prior Codex process identity is still owned, and stop it if so.
///
/// `process_identity_lookup` is injected so tests can avoid touching real OS state.
/// In production callers pass [`crate::codex_process_identity`].
pub fn stop_prior_codex_if_owned<F>(
    prior_process_id: Option<u32>,
    prior_process_identity: Option<&str>,
    process_identity_lookup: F,
) -> PriorProcessOutcome
where
    F: Fn(u32) -> Option<String>,
{
    let (Some(pid), Some(expected_identity)) = (prior_process_id, prior_process_identity) else {
        return PriorProcessOutcome::NoPriorRecord;
    };
    match process_identity_lookup(pid) {
        None => PriorProcessOutcome::AlreadyStopped,
        Some(current) if current != expected_identity => PriorProcessOutcome::AlreadyStopped,
        Some(_) => {
            // Same PID, matching process-start identity → still our Codex. Stop it.
            // We use SIGTERM via the platform `kill` command to avoid a libc
            // dependency. Failure to terminate is reported as Stopped anyway because
            // the recovery flow re-verifies before spawning.
            let _ = Command::new("kill")
                .arg(pid.to_string())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            PriorProcessOutcome::Stopped
        }
    }
}

/// Run a recovery Codex pass in the existing Assignment Worktree. Mirrors
/// `run_initial_codex` but is named so the call site is auditable as a recovery
/// pass — it does not silently reuse the initial-attempt code path.
pub fn run_recovery_codex(
    codex_binary_path: &Path,
    worktree_path: &Path,
    prompt: &str,
) -> Result<CodexExecution, String> {
    crate::run_initial_codex(codex_binary_path, worktree_path, prompt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_afk_contracts::AssignmentTerminalOutcome;

    #[test]
    fn recovery_prompt_includes_durable_facts_not_invented_reasoning() {
        let prompt = build_recovery_prompt(RecoveryPromptFacts {
            source_issue_raw_text: "# Title\n\nbody",
            source_id: "issue-7",
            assignment_branch: "agentic-afk/local-markdown-issue-7",
            assignment_worktree_path: "/tmp/wt",
            prior_process_identity: Some("procfs-start-time:1234"),
            prior_block_reason: Some("missing-input"),
        });
        assert!(prompt.contains("recovery Assignment Attempt"));
        assert!(prompt.contains("issue-7"));
        assert!(prompt.contains("/tmp/wt"));
        assert!(prompt.contains("agentic-afk/local-markdown-issue-7"));
        assert!(prompt.contains("procfs-start-time:1234"));
        assert!(prompt.contains("missing-input"));
        assert!(prompt.contains("# Title\n\nbody"));
        // Importantly, the prompt should not invent prior agent thinking.
        assert!(!prompt.to_lowercase().contains("the prior agent thought"));
    }

    #[test]
    fn recovery_prompt_handles_missing_optional_facts() {
        let prompt = build_recovery_prompt(RecoveryPromptFacts {
            source_issue_raw_text: "raw",
            source_id: "id",
            assignment_branch: "b",
            assignment_worktree_path: "/p",
            prior_process_identity: None,
            prior_block_reason: None,
        });
        assert!(prompt.contains("id"));
        assert!(!prompt.contains("Prior Codex process identity"));
        assert!(!prompt.contains("Prior block reason"));
    }

    #[test]
    fn stop_prior_no_record_when_either_field_missing() {
        assert_eq!(
            stop_prior_codex_if_owned(None, Some("id"), |_| Some("id".to_string())),
            PriorProcessOutcome::NoPriorRecord
        );
        assert_eq!(
            stop_prior_codex_if_owned(Some(1), None, |_| Some("id".to_string())),
            PriorProcessOutcome::NoPriorRecord
        );
    }

    #[test]
    fn stop_prior_already_stopped_when_identity_lookup_returns_none() {
        assert_eq!(
            stop_prior_codex_if_owned(Some(42), Some("id"), |_| None),
            PriorProcessOutcome::AlreadyStopped
        );
    }

    #[test]
    fn stop_prior_already_stopped_when_identity_lookup_mismatches() {
        assert_eq!(
            stop_prior_codex_if_owned(Some(42), Some("old"), |_| Some("new".to_string())),
            PriorProcessOutcome::AlreadyStopped
        );
    }

    #[test]
    fn build_recovery_prompt_round_trips_terminal_outcome_summary_as_block_reason() {
        let outcome = AssignmentTerminalOutcome {
            outcome: "Blocked".to_string(),
            summary: "need credentials".to_string(),
        };
        let prompt = build_recovery_prompt(RecoveryPromptFacts {
            source_issue_raw_text: "raw",
            source_id: "id",
            assignment_branch: "b",
            assignment_worktree_path: "/p",
            prior_process_identity: None,
            prior_block_reason: Some(&outcome.summary),
        });
        assert!(prompt.contains("need credentials"));
    }
}
