//! Plan Run finalization: pure `decide_plan_run_terminal` decision.
//!
//! After every reviewed Issue Assignment in a Plan Run has been routed
//! through `decide_merge_outcome`, the coordinator collects the per-asgn
//! outcomes and asks this module what terminal state the Plan Run row
//! should land in. This decision used to live inline in the coordinator
//! and had a dead `else` arm; this module names the invariants explicitly
//! and tests them.

use crate::merge_phase::AssignmentMergeOutcome;

/// Terminal state of one Plan Run. Mirrors the persisted strings used by
/// `persistence::finish_plan_run`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanRunTerminal {
    /// At least one reviewed Issue Assignment merged successfully and
    /// (the coordinator's responsibility) the Integration Branch push
    /// succeeded.
    Succeeded,
    /// The Planning Phase chose no eligible work. Distinct from
    /// `Succeeded` so the Dashboard can show "no work" rather than
    /// "merged work."
    SucceededEmpty,
    /// No reviewed work made it through the Merge Phase, or every
    /// reviewed assignment blocked during merge.
    Failed,
}

impl PlanRunTerminal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::SucceededEmpty => "succeeded_empty",
            Self::Failed => "failed",
        }
    }
}

/// Inputs to the Plan Run terminal decision. `outcomes` is empty when the
/// Planning Phase finished with no eligible selections (the empty-success
/// path); otherwise it holds one entry per claimed Issue Assignment.
#[derive(Clone, Debug, Default)]
pub struct PlanRunFinalize {
    pub planning_was_empty: bool,
    pub outcomes: Vec<AssignmentMergeOutcome>,
}

/// Decide the terminal state of a Plan Run from its per-assignment merge
/// outcomes. Pure: no I/O.
///
/// Invariants:
///
/// * Empty Planning Phase (no selections) ⇒ `SucceededEmpty`.
/// * Any `Merged` outcome ⇒ `Succeeded` (partial-success path).
/// * No `Merged` outcomes ⇒ `Failed`. This collapses the previous dead
///   `else` arm in the coordinator that always returned `"failed"`
///   regardless of the reviewed-but-not-merged count.
pub fn decide_plan_run_terminal(input: &PlanRunFinalize) -> PlanRunTerminal {
    if input.planning_was_empty {
        return PlanRunTerminal::SucceededEmpty;
    }
    if input
        .outcomes
        .iter()
        .any(|outcome| matches!(outcome, AssignmentMergeOutcome::Merged))
    {
        return PlanRunTerminal::Succeeded;
    }
    PlanRunTerminal::Failed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocked() -> AssignmentMergeOutcome {
        AssignmentMergeOutcome::Blocked {
            reason: "conflict".to_string(),
        }
    }

    #[test]
    fn empty_planning_is_succeeded_empty() {
        let input = PlanRunFinalize {
            planning_was_empty: true,
            outcomes: vec![],
        };
        assert_eq!(decide_plan_run_terminal(&input), PlanRunTerminal::SucceededEmpty);
    }

    #[test]
    fn any_merged_is_succeeded() {
        let input = PlanRunFinalize {
            planning_was_empty: false,
            outcomes: vec![AssignmentMergeOutcome::Merged, blocked()],
        };
        assert_eq!(decide_plan_run_terminal(&input), PlanRunTerminal::Succeeded);
    }

    #[test]
    fn all_blocked_is_failed() {
        // Regression: the previous inline `else if` arm in the
        // coordinator returned `"failed"` from both branches. The
        // collapsed decision still finishes `Failed` here, but the
        // invariant is now under a named test.
        let input = PlanRunFinalize {
            planning_was_empty: false,
            outcomes: vec![blocked(), blocked()],
        };
        assert_eq!(decide_plan_run_terminal(&input), PlanRunTerminal::Failed);
    }

    #[test]
    fn all_not_attempted_is_failed() {
        let input = PlanRunFinalize {
            planning_was_empty: false,
            outcomes: vec![
                AssignmentMergeOutcome::NotAttempted,
                AssignmentMergeOutcome::NotAttempted,
            ],
        };
        assert_eq!(decide_plan_run_terminal(&input), PlanRunTerminal::Failed);
    }
}
