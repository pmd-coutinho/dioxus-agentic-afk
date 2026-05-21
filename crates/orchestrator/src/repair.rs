//! Repair Assignment Attempts for failed GitHub Change Proposal checks.
//!
//! A repair Assignment Attempt reuses the existing Assignment Worktree and
//! Change Proposal context rather than starting a fresh assignment. The
//! prompt carries the original Source Issue text, the proposal identity, the
//! failed required-check facts, and any verified worktree facts the
//! Control Plane already established, so Codex sees one coherent repair
//! brief instead of inferring history from process state.

use crate::{CodexExecution, run_codex_exec};
use agentic_afk_contracts::FailedCheckFact;
use std::path::Path;

/// Inputs the Control Plane passes from the assignment surface into the
/// orchestrator boundary when launching a repair Assignment Attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepairPromptFacts<'a> {
    pub source_id: &'a str,
    pub source_title: &'a str,
    pub source_raw_text: &'a str,
    pub change_proposal_url: &'a str,
    pub branch: &'a str,
    pub failed_checks: &'a [FailedCheckFact],
    pub verified_worktree_facts: Option<&'a str>,
}

/// Build the repair prompt that Codex receives. Pure function — the test
/// suite asserts the prompt facts directly without touching `codex exec`.
pub fn build_repair_prompt(facts: &RepairPromptFacts<'_>) -> String {
    let mut prompt = String::new();
    prompt.push_str(
        "Repair this Change Proposal's failed required checks inside the existing Assignment Worktree.\n",
    );
    prompt.push_str("Do not start fresh; keep prior assignment work and only address the failed checks.\n\n");
    prompt.push_str(&format!(
        "Source Issue: {source_id} {source_title}\n",
        source_id = facts.source_id,
        source_title = facts.source_title,
    ));
    prompt.push_str(&format!(
        "Change Proposal: {url}\nAssignment branch: {branch}\n\n",
        url = facts.change_proposal_url,
        branch = facts.branch,
    ));
    if let Some(verified) = facts.verified_worktree_facts {
        prompt.push_str("Verified worktree facts:\n");
        prompt.push_str(verified.trim_end_matches('\n'));
        prompt.push_str("\n\n");
    }
    prompt.push_str("Failed required checks:\n");
    if facts.failed_checks.is_empty() {
        prompt.push_str("- (none reported)\n");
    } else {
        for failed in facts.failed_checks {
            prompt.push_str(&format!("- {}", failed.name));
            if let Some(url) = failed.url.as_deref() {
                prompt.push_str(&format!(" ({url})"));
            }
            if let Some(summary) = failed.summary.as_deref() {
                prompt.push_str(&format!(": {summary}"));
            }
            prompt.push('\n');
        }
    }
    prompt.push_str("\nOriginal Source Issue brief:\n");
    prompt.push_str(facts.source_raw_text.trim_end_matches('\n'));
    prompt.push('\n');
    prompt
}

/// Launch the repair `codex exec` Assignment Attempt in the existing
/// Assignment Worktree.
pub fn run_repair_codex(
    codex_binary_path: &Path,
    worktree_path: &Path,
    facts: &RepairPromptFacts<'_>,
) -> Result<CodexExecution, String> {
    let prompt = build_repair_prompt(facts);
    run_codex_exec(codex_binary_path, worktree_path, &prompt)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts<'a>(failed: &'a [FailedCheckFact]) -> RepairPromptFacts<'a> {
        RepairPromptFacts {
            source_id: "21",
            source_title: "Repair me",
            source_raw_text: "# Repair me\n\nBody of the issue.",
            change_proposal_url: "https://github.com/owner/repo/pull/42",
            branch: "agentic-afk/github-21",
            failed_checks: failed,
            verified_worktree_facts: Some("tests previously passed locally"),
        }
    }

    #[test]
    fn prompt_carries_proposal_identity_branch_and_source_brief() {
        let prompt = build_repair_prompt(&facts(&[]));
        assert!(prompt.contains("Source Issue: 21 Repair me"));
        assert!(prompt.contains("Change Proposal: https://github.com/owner/repo/pull/42"));
        assert!(prompt.contains("Assignment branch: agentic-afk/github-21"));
        assert!(prompt.contains("Body of the issue."));
        assert!(prompt.contains("Verified worktree facts:"));
        assert!(prompt.contains("tests previously passed locally"));
    }

    #[test]
    fn prompt_lists_failed_check_facts() {
        let failed = vec![
            FailedCheckFact {
                name: "ci/lint".to_string(),
                url: Some("https://github.com/owner/repo/actions/runs/1".to_string()),
                summary: Some("clippy failure".to_string()),
            },
            FailedCheckFact {
                name: "ci/unit".to_string(),
                url: None,
                summary: None,
            },
        ];
        let prompt = build_repair_prompt(&facts(&failed));
        assert!(prompt.contains("- ci/lint (https://github.com/owner/repo/actions/runs/1): clippy failure"));
        assert!(prompt.contains("- ci/unit"));
    }

    #[test]
    fn prompt_states_no_failed_checks_when_none_reported() {
        let prompt = build_repair_prompt(&facts(&[]));
        assert!(prompt.contains("Failed required checks:\n- (none reported)"));
    }
}
