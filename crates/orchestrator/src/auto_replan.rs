use agentic_afk_contracts::{
    AutoReplanState, BlockReason, PauseReason, PlanRunResponse, PlanRunState,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CycleOutcome {
    Continue,
    Pause(PauseReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncOutcome {
    Succeeded,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CycleTrigger {
    StartPlanRun,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AutoReplanCurrent {
    pub state: AutoReplanState,
    pub has_active_plan_run: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AutoReplanDecision {
    pub next_state: AutoReplanState,
    pub pause_reason: Option<PauseReason>,
    pub trigger: Option<CycleTrigger>,
}

pub struct AutoReplanDriver;

impl AutoReplanDriver {
    pub fn decide(
        current: AutoReplanCurrent,
        last_plan_run_outcome: Option<CycleOutcome>,
        sync_result: Option<SyncOutcome>,
    ) -> AutoReplanDecision {
        if current.state != AutoReplanState::Armed {
            return AutoReplanDecision {
                next_state: current.state,
                pause_reason: None,
                trigger: None,
            };
        }

        if current.has_active_plan_run {
            return AutoReplanDecision {
                next_state: AutoReplanState::Armed,
                pause_reason: None,
                trigger: None,
            };
        }

        if sync_result == Some(SyncOutcome::Failed) {
            return AutoReplanDecision {
                next_state: AutoReplanState::Paused,
                pause_reason: Some(PauseReason::SyncFailed),
                trigger: None,
            };
        }

        if let Some(CycleOutcome::Pause(reason)) = last_plan_run_outcome {
            return AutoReplanDecision {
                next_state: AutoReplanState::Paused,
                pause_reason: Some(reason),
                trigger: None,
            };
        }

        if sync_result == Some(SyncOutcome::Succeeded) && last_plan_run_outcome.is_none() {
            return AutoReplanDecision {
                next_state: AutoReplanState::Armed,
                pause_reason: None,
                trigger: Some(CycleTrigger::StartPlanRun),
            };
        }

        AutoReplanDecision {
            next_state: AutoReplanState::Armed,
            pause_reason: None,
            trigger: None,
        }
    }
}

pub fn classify_plan_run_for_auto_replan(plan_run: &PlanRunResponse) -> CycleOutcome {
    if plan_run.state != PlanRunState::Finished {
        return CycleOutcome::Pause(PauseReason::PlanningFailed);
    }

    if plan_run
        .phase_outputs
        .iter()
        .any(|output| output.phase == "planning" && output.outcome == "failed")
    {
        return CycleOutcome::Pause(PauseReason::PlanningFailed);
    }

    if plan_run.assignments.is_empty()
        && plan_run
            .phase_outputs
            .iter()
            .any(|output| output.phase == "planning" && output.outcome == "succeeded_empty")
    {
        return CycleOutcome::Pause(PauseReason::EmptyBacklog);
    }

    if plan_run.assignments.iter().any(|assignment| {
        assignment.status == "blocked"
            && assignment
                .block_reason
                .as_ref()
                .is_some_and(|reason| reason.kind == BlockReason::PushNonFastForward)
    }) {
        return CycleOutcome::Pause(PauseReason::PushNonFastForward);
    }

    if plan_run
        .assignments
        .iter()
        .any(|assignment| assignment.status == "blocked")
    {
        return CycleOutcome::Pause(PauseReason::AssignmentBlocked);
    }

    if plan_run
        .assignments
        .iter()
        .any(|assignment| assignment.status == "merge_staged")
    {
        return CycleOutcome::Pause(PauseReason::MergeStagedLeft);
    }

    let merged = plan_run
        .assignments
        .iter()
        .filter(|assignment| assignment.status == "merged")
        .count();
    if merged > 0 {
        CycleOutcome::Continue
    } else {
        CycleOutcome::Pause(PauseReason::PlanningFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_afk_contracts::{
        BlockReasonResponse, IssueAssignmentResponse, PhaseOutputResponse, ProjectId,
    };

    fn decision(
        state: AutoReplanState,
        active: bool,
        last: Option<CycleOutcome>,
        sync: Option<SyncOutcome>,
    ) -> AutoReplanDecision {
        AutoReplanDriver::decide(
            AutoReplanCurrent {
                state,
                has_active_plan_run: active,
            },
            last,
            sync,
        )
    }

    #[test]
    fn driver_covers_reachable_state_table() {
        let cases = [
            (
                "off cold start",
                AutoReplanState::Off,
                false,
                None,
                None,
                AutoReplanDecision {
                    next_state: AutoReplanState::Off,
                    pause_reason: None,
                    trigger: None,
                },
            ),
            (
                "paused cold start",
                AutoReplanState::Paused,
                false,
                None,
                None,
                AutoReplanDecision {
                    next_state: AutoReplanState::Paused,
                    pause_reason: None,
                    trigger: None,
                },
            ),
            (
                "armed active plan run",
                AutoReplanState::Armed,
                true,
                None,
                Some(SyncOutcome::Succeeded),
                AutoReplanDecision {
                    next_state: AutoReplanState::Armed,
                    pause_reason: None,
                    trigger: None,
                },
            ),
            (
                "armed sync success starts",
                AutoReplanState::Armed,
                false,
                None,
                Some(SyncOutcome::Succeeded),
                AutoReplanDecision {
                    next_state: AutoReplanState::Armed,
                    pause_reason: None,
                    trigger: Some(CycleTrigger::StartPlanRun),
                },
            ),
            (
                "armed sync failure pauses",
                AutoReplanState::Armed,
                false,
                None,
                Some(SyncOutcome::Failed),
                AutoReplanDecision {
                    next_state: AutoReplanState::Paused,
                    pause_reason: Some(PauseReason::SyncFailed),
                    trigger: None,
                },
            ),
            (
                "armed continue stays armed",
                AutoReplanState::Armed,
                false,
                Some(CycleOutcome::Continue),
                None,
                AutoReplanDecision {
                    next_state: AutoReplanState::Armed,
                    pause_reason: None,
                    trigger: None,
                },
            ),
            (
                "armed pause outcome pauses",
                AutoReplanState::Armed,
                false,
                Some(CycleOutcome::Pause(PauseReason::MergeStagedLeft)),
                None,
                AutoReplanDecision {
                    next_state: AutoReplanState::Paused,
                    pause_reason: Some(PauseReason::MergeStagedLeft),
                    trigger: None,
                },
            ),
        ];

        for (name, state, active, last, sync, expected) in cases {
            assert_eq!(decision(state, active, last, sync), expected, "{name}");
        }
    }

    fn plan_run(state: PlanRunState, assignments: Vec<IssueAssignmentResponse>) -> PlanRunResponse {
        PlanRunResponse {
            id: "pr".into(),
            project_id: ProjectId("p".into()),
            integration_branch: "main".into(),
            baseline_commit: "abc".into(),
            state,
            started_at: "unix:1".into(),
            finished_at: Some("unix:2".into()),
            phase_outputs: vec![],
            assignments,
        }
    }

    fn assignment(status: &str, block_reason: Option<BlockReason>) -> IssueAssignmentResponse {
        IssueAssignmentResponse {
            id: format!("a-{status}"),
            project_id: ProjectId("p".into()),
            source_id: "1".into(),
            source_title: "Issue".into(),
            branch: "agent/issue-1".into(),
            worktree_path: "/tmp/worktree".into(),
            status: status.into(),
            status_detail: None,
            latest_attempt: None,
            plan_run_id: Some("pr".into()),
            selection_summary: None,
            phase_outputs: vec![],
            review_rejection_count: 0,
            block_reason: block_reason.map(|kind| BlockReasonResponse { kind, detail: None }),
        }
    }

    #[test]
    fn classifier_maps_terminal_shapes() {
        let mut planning_failed = plan_run(PlanRunState::Finished, vec![]);
        planning_failed.phase_outputs.push(PhaseOutputResponse {
            phase: "planning".into(),
            outcome: "failed".into(),
            body_json: serde_json::json!({}),
            recorded_at: "unix:1".into(),
            assignment_id: None,
        });

        let cases = [
            (
                "merged-only success",
                plan_run(PlanRunState::Finished, vec![assignment("merged", None)]),
                CycleOutcome::Continue,
            ),
            (
                "empty success",
                {
                    let mut run = plan_run(PlanRunState::Finished, vec![]);
                    run.phase_outputs.push(PhaseOutputResponse {
                        phase: "planning".into(),
                        outcome: "succeeded_empty".into(),
                        body_json: serde_json::json!({}),
                        recorded_at: "unix:1".into(),
                        assignment_id: None,
                    });
                    run
                },
                CycleOutcome::Pause(PauseReason::EmptyBacklog),
            ),
            (
                "blocked-only",
                plan_run(
                    PlanRunState::Finished,
                    vec![assignment("blocked", Some(BlockReason::MergePhaseFailed))],
                ),
                CycleOutcome::Pause(PauseReason::AssignmentBlocked),
            ),
            (
                "merge-staged-only",
                plan_run(
                    PlanRunState::Finished,
                    vec![assignment("merge_staged", None)],
                ),
                CycleOutcome::Pause(PauseReason::MergeStagedLeft),
            ),
            (
                "mixed merged and blocked",
                plan_run(
                    PlanRunState::Finished,
                    vec![
                        assignment("merged", None),
                        assignment("blocked", Some(BlockReason::ReviewRetryLimitExhausted)),
                    ],
                ),
                CycleOutcome::Pause(PauseReason::AssignmentBlocked),
            ),
            (
                "mixed merged and staged",
                plan_run(
                    PlanRunState::Finished,
                    vec![assignment("merged", None), assignment("merge_staged", None)],
                ),
                CycleOutcome::Pause(PauseReason::MergeStagedLeft),
            ),
            (
                "planning failed",
                planning_failed,
                CycleOutcome::Pause(PauseReason::PlanningFailed),
            ),
            (
                "push non-fast-forward",
                plan_run(
                    PlanRunState::Finished,
                    vec![assignment("blocked", Some(BlockReason::PushNonFastForward))],
                ),
                CycleOutcome::Pause(PauseReason::PushNonFastForward),
            ),
        ];

        for (name, run, expected) in cases {
            assert_eq!(classify_plan_run_for_auto_replan(&run), expected, "{name}");
        }
    }
}
