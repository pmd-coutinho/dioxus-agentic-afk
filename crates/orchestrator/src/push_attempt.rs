//! Push attempt classification (issue #53 / ADR-0037).
//!
//! Both the **Merge Phase** first push and the operator-initiated
//! **Retry Push** action call into the same `IntegrationBranchPusher`
//! seam. This module owns the pure classification of one push attempt's
//! `Result<(), PlanRunPhaseError>` into a typed [`PushOutcome`] so the
//! coordinator and the Retry Push handler branch on the same taxonomy.
//!
//! The classification is intentionally heuristic: `git push` does not
//! emit a structured exit code for "non-fast-forward". The phrase
//! detection covers the strings real `git push` and the canonical
//! GitHub remote error use; anything else is reported as
//! [`PushOutcome::Other`] so the operator can decide whether the
//! failure is transient (retry again) or permanent (abandon).
//!
//! ADR-0037 dictates the routing on each outcome:
//! - [`PushOutcome::Success`] advances `merge_staged` → `merged` and
//!   triggers the Lifecycle `Completed` write-back (best-effort per
//!   ADR-0035).
//! - [`PushOutcome::NonFastForward`] routes the Issue Assignment to
//!   `blocked` with [`BlockReason::PushNonFastForward`] because the
//!   Integration Branch has diverged and the staged local tree is no
//!   longer a valid update.
//! - [`PushOutcome::Other`] leaves the Issue Assignment at
//!   `merge_staged` so the operator may retry again or abandon.

use crate::plan_run::PlanRunPhaseError;

/// Classification of one `git push` attempt against the Integration
/// Branch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PushOutcome {
    /// The push completed successfully and the upstream now reflects the
    /// locally integrated tree.
    Success,
    /// The remote rejected the push because the Integration Branch has
    /// diverged. The staged local tree is no longer a valid update and
    /// recovery belongs in a new Plan Run with a refreshed baseline.
    NonFastForward { detail: String },
    /// Any other failure (network outage, auth token expiry, transient
    /// remote unavailability, branch protection rejection). The detail
    /// carries the upstream error text so the operator can decide whether
    /// to retry again or abandon the staged work.
    Other { detail: String },
}

impl PushOutcome {
    /// Stable string discriminator used for activity / phase output
    /// outcomes. `Success` maps to `succeeded`; the failure variants map
    /// to `failed` and let the body carry the discriminator detail.
    pub fn outcome_str(&self) -> &'static str {
        match self {
            Self::Success => "succeeded",
            Self::NonFastForward { .. } | Self::Other { .. } => "failed",
        }
    }

    /// `true` iff the upstream accepted the push as a fast-forward
    /// update. Used to fill the `fast_forward: bool` field of the push
    /// Phase Output body per ADR-0038.
    pub fn fast_forward(&self) -> bool {
        matches!(self, Self::Success)
    }
}

/// Pure classifier: turns the `Result<(), PlanRunPhaseError>` returned
/// by an `IntegrationBranchPusher` into a typed [`PushOutcome`].
///
/// The non-fast-forward branch is triggered when the upstream error
/// text mentions one of the canonical phrases used by `git push`
/// (`non-fast-forward`, `non fast forward`, `would not be a fast
/// forward`) or the GitHub remote rejection (`fetch first`,
/// `Updates were rejected`, `tip of your current branch is behind`).
/// Anything else falls through to [`PushOutcome::Other`].
pub fn classify_push_result(result: Result<(), PlanRunPhaseError>) -> PushOutcome {
    match result {
        Ok(()) => PushOutcome::Success,
        Err(PlanRunPhaseError::NonFastForward { stderr }) => {
            PushOutcome::NonFastForward { detail: stderr }
        }
        Err(PlanRunPhaseError::IntegrationPush(detail)) => classify_failure_detail(detail),
        Err(other) => PushOutcome::Other {
            detail: other.to_string(),
        },
    }
}

fn classify_failure_detail(detail: String) -> PushOutcome {
    let lower = detail.to_ascii_lowercase();
    let is_non_ff = ["non-fast-forward", "non fast forward", "fetch first"]
        .iter()
        .any(|needle| lower.contains(needle))
        || lower.contains("would not be a fast-forward")
        || lower.contains("would not be a fast forward")
        || lower.contains("updates were rejected")
        || lower.contains("tip of your current branch is behind");
    if is_non_ff {
        PushOutcome::NonFastForward { detail }
    } else {
        PushOutcome::Other { detail }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_classifies_as_success() {
        assert_eq!(classify_push_result(Ok(())), PushOutcome::Success);
        assert!(PushOutcome::Success.fast_forward());
        assert_eq!(PushOutcome::Success.outcome_str(), "succeeded");
    }

    #[test]
    fn integration_push_with_non_fast_forward_phrase_classifies_as_non_fast_forward() {
        let result = Err(PlanRunPhaseError::IntegrationPush(
            "git push origin main: ! [rejected] main -> main (non-fast-forward)".to_string(),
        ));
        match classify_push_result(result) {
            PushOutcome::NonFastForward { detail } => {
                assert!(detail.contains("non-fast-forward"));
            }
            other => panic!("expected NonFastForward, got {other:?}"),
        }
    }

    #[test]
    fn integration_push_with_fetch_first_phrase_classifies_as_non_fast_forward() {
        let result = Err(PlanRunPhaseError::IntegrationPush(
            "hint: Updates were rejected because the remote contains work that you do\nhint: not have locally. fetch first".to_string(),
        ));
        assert!(matches!(
            classify_push_result(result),
            PushOutcome::NonFastForward { .. }
        ));
    }

    #[test]
    fn integration_push_with_tip_behind_phrase_classifies_as_non_fast_forward() {
        let result = Err(PlanRunPhaseError::IntegrationPush(
            "hint: the tip of your current branch is behind its remote counterpart".to_string(),
        ));
        assert!(matches!(
            classify_push_result(result),
            PushOutcome::NonFastForward { .. }
        ));
    }

    #[test]
    fn integration_push_with_generic_error_classifies_as_other() {
        let result = Err(PlanRunPhaseError::IntegrationPush(
            "ssh: connect to host github.com port 22: Connection timed out".to_string(),
        ));
        match classify_push_result(result) {
            PushOutcome::Other { detail } => {
                assert!(detail.contains("Connection timed out"));
            }
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn auth_failure_classifies_as_other_not_non_fast_forward() {
        let result = Err(PlanRunPhaseError::IntegrationPush(
            "remote: Permission to owner/repo.git denied to alice.".to_string(),
        ));
        assert!(matches!(
            classify_push_result(result),
            PushOutcome::Other { .. }
        ));
    }

    #[test]
    fn non_fast_forward_variant_classifies_as_non_fast_forward() {
        // Issue #63: production pushers can now return the typed
        // `NonFastForward` variant directly instead of relying on the
        // classifier to inspect stderr inside an `IntegrationPush` payload.
        let result = Err(PlanRunPhaseError::NonFastForward {
            stderr: "! [rejected] main -> main (non-fast-forward)".to_string(),
        });
        match classify_push_result(result) {
            PushOutcome::NonFastForward { detail } => {
                assert!(detail.contains("non-fast-forward"));
            }
            other => panic!("expected NonFastForward, got {other:?}"),
        }
    }

    #[test]
    fn non_push_phase_error_classifies_as_other() {
        // A caller misusing the classifier with a non-push error still
        // gets a typed outcome rather than panicking. Routes to Other so
        // the operator may retry.
        let result = Err(PlanRunPhaseError::Refresh("baseline missing".to_string()));
        match classify_push_result(result) {
            PushOutcome::Other { detail } => assert!(detail.contains("baseline missing")),
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn outcome_str_matches_phase_output_taxonomy() {
        assert_eq!(
            PushOutcome::NonFastForward { detail: "x".into() }.outcome_str(),
            "failed"
        );
        assert_eq!(
            PushOutcome::Other { detail: "x".into() }.outcome_str(),
            "failed"
        );
        assert!(!PushOutcome::NonFastForward { detail: "x".into() }.fast_forward());
        assert!(!PushOutcome::Other { detail: "x".into() }.fast_forward());
    }
}
