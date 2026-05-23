# Dioxus Agentic AFK

Dioxus Agentic AFK is a Rust-native agent orchestration system with a dashboard-first control plane. It exists to give a developer a foundation for starting, observing, and managing agent work while away from the keyboard.

## Language

**Orchestrator**:
The system that coordinates agent work across isolated environments.
_Avoid_: Wrapper, script runner

**Control Plane**:
The user-facing surface for starting, observing, and managing agent work.
_Avoid_: Add-on dashboard, admin panel

**Dashboard**:
The primary visual interface for the **Control Plane**.
_Avoid_: UI, frontend

**Boilerplate**:
The minimal runnable foundation for the **Orchestrator** and **Dashboard** before agent-work features exist.
_Avoid_: MVP, prototype, full app

**Project**:
A local codebase root known to the **Control Plane**.
_Avoid_: Repository, workspace, task

**Project ID**:
A stable identifier for a **Project** that does not depend on its filesystem path.
_Avoid_: Database row ID, path slug

**Git Summary**:
Read-only Git metadata derived from a **Project** path for dashboard display.
_Avoid_: Project state, repository identity

**Issue Source**:
The configured place where the **Control Plane** discovers candidate work for a **Project**. An **Issue Source** may be a hosted issue tracker, local markdown files, or another explicit work source.
_Avoid_: Repository, project, task list

**Source Issue**:
An issue as represented in its **Issue Source**. A **Source Issue** remains the authoritative record for readiness, dependencies, and acceptance criteria.
_Avoid_: Control-plane issue, copied issue, internal ticket

**Lifecycle Status**:
The coarse implementation state of a **Source Issue** as reported by the **Control Plane** back to the **Issue Source**. The minimal statuses are Ready, Claimed, Running, Blocked, and Completed. Lifecycle write-back is a correctness invariant at the Claimed transition (to prevent another **Plan Run** from re-selecting the same **Source Issue**) and best-effort for transitions after Claimed.
_Avoid_: Local-only status, dashboard state, hidden progress

**Assignment Status**:
The fine-grained execution state of one **Issue Assignment** inside its **Plan Run**, distinct from **Lifecycle Status**. The values are Implementing, Implemented, Reviewed, Merging, MergeStaged, Merged, and Blocked. **MergeStaged** sits between **Merging** and **Merged**: the **Merge Phase** has integrated locally and verified, but the **Integration Branch** push has not yet succeeded; `Merged` always implies a successful push. **Assignment Status** is **Control Plane** detail and is not written back to the **Issue Source**; coarse **Lifecycle Status** is.
_Avoid_: Lifecycle Status, dashboard label, source-issue field

**Issue Assignment**:
One selected **Ready Issue** and its issue branch as it moves through implementation and the **Review Phase** within a **Plan Run**.
_Avoid_: Shared issue work, automatic sub-agent split, pooled task

**Block Reason**:
The typed cause of a blocked **Issue Assignment**, paired with an optional freeform detail string for human context. Values are `ReviewRetryLimitExhausted`, `MergePhaseFailed`, `PushNonFastForward`, and `AbandonedStaged`. The typed kind drives **Control Plane** branching (badges, recovery affordances) while the detail carries cause-specific specifics like push stderr.
_Avoid_: Freeform-only reason, dashboard-only label, agent narrative

**Plan Run**:
One manually started **Planning Phase** and the parallel issue work it selects through **Review Phase** and **Merge Phase**, all based on one refreshed **Integration Branch** baseline.
_Avoid_: Backlog, queue drain, individual assignment

**Assignment Attempt**:
One agent execution pass within an **Issue Assignment**, such as an implementation pass, review pass, or later implementation pass in a **Review Loop**.
_Avoid_: Assignment, retry branch, agent log

**Assignment Worktree**:
The isolated worktree created for one **Issue Assignment** from the **Plan Run** integration baseline. Its branch is derived from the **Source Issue** identity.
_Avoid_: Project checkout, agent workspace, mutable Project path

**Planning Phase**:
The manually started agent pass that inspects the **Project** and its issue descriptions to select `ready-for-agent` **Ready Issues** and issue branches that can start together without editing project files.
_Avoid_: Automatic queue drain, manual issue start, planning snapshot

**Planned Claim**:
One validated planner choice from the **Planning Phase**, pairing the planner's selected **Source Issue** identity and derived issue branch with the matching eligible **Source Issue** snapshot. A **Planned Claim** is ready for **Assignment Worktree** provisioning and **Issue Assignment** creation; ineligible or capacity-exceeding planner output never becomes a **Planned Claim**.
_Avoid_: Planner suggestion, raw selection, issue task

**Max Parallel Tasks**:
The per-**Project** cap on issue tasks that may run in parallel after the **Planning Phase** selects work. It does not govern the **Planning Phase**, which has at most one active run for a **Project**.
_Avoid_: Planner count, global worker count, source order

**Integration Branch**:
The per-**Project** branch that a successful **Merge Phase** updates and pushes with the merged result of a **Plan Run**. It defaults from the **Project** detected default branch when first configured.
_Avoid_: Pull request branch, issue branch, implicit default branch

**Review Retry Limit**:
The per-**Project** cap on how many review rejections may return one **Issue Assignment** through its **Review Loop** before it blocks. It has a platform default.
_Avoid_: Infinite review loop, global retry budget, merge retry

**Review Phase**:
The agent pass that approves or rejects implemented issue work with findings before it may enter the **Merge Phase**. It may run verification needed for that decision, but it does not edit project files.
_Avoid_: Implementation self-check, human review, merge check

**Review Loop**:
The return of one **Issue Assignment** from the **Review Phase** to another implementation pass before it can be reviewed again.
_Avoid_: New assignment, whole-plan restart, human-only repair

**Phase Prompt**:
The task brief for one Sandcastle-style phase in a **Plan Run**. The initial **Phase Prompts** adapt Sandcastle's planning, implementation, review, and merge prompts and include **Project** instructions in every phase.
_Avoid_: Generic agent prompt, issue template, project instructions

**Phase Output**:
The durable result recorded for a **Plan Run** phase or **Assignment Attempt**, including planning selection, review findings, merge verification, **Integration Branch** push attempts (one row per attempt), and block reasons. Bodies are typed per recording slice (planning / implementation / review / merge / push / failed).
_Avoid_: Ephemeral terminal text, worktree artifact, source issue body

**Merge Phase**:
The agent-owned acceptance step that integrates issue work after the **Review Phase**, resolves integration problems it can, and verifies the integrated result before push.
_Avoid_: Human merge, unattended acceptance

**Ready Issue**:
An issue from an **Issue Source** that has been explicitly marked `ready-for-agent` for unattended agent implementation.
_Avoid_: Open issue, available issue, backlog item

**Issue Dependency**:
A blocker recorded in a **Source Issue** description showing that one **Ready Issue** must wait for another issue before it can be implemented.
_Avoid_: Guess, similarity, priority

**Source Order**:
The ordering of **Source Issues** as provided by the **Issue Source** for display and source reconciliation.
_Avoid_: Planner priority, batch selection, random queue

**Parent Issue**:
An issue that groups related **Ready Issues** into a larger body of work without itself defining the execution order.
_Avoid_: Epic, milestone, project

**Activity**:
A chronological record of noteworthy control-plane events for a **Project**, including **Issue Assignment** and **Assignment Attempt** lifecycle changes.
_Avoid_: Fake metrics, agent output

**Auto-Replan**:
The per-**Project** cadence that triggers consecutive **Plan Runs** without human input while it is **Armed**. **Auto-Replan** preserves the single-active **Plan Run** invariant and pauses (not stops) on any condition that needs human attention.
_Avoid_: Cron, queue drain, autonomous agent

**Auto-Replan State**:
A tri-state per-**Project** value of `Off`, `Armed`, or `Paused`. `Off` and `Armed` are human-set; `Paused` is system-set and carries a typed **Pause Reason**. Transition `Paused` → `Armed` requires an explicit human Resume; backlog change or block resolution does not auto-resume.
_Avoid_: Auto-replan on/off boolean, planner status, scheduler queue

**Auto-Replan Cycle**:
One iteration driven by **Auto-Replan**: sync the **Issue Source**, then start a **Planning Phase**, then run its **Plan Run** to a terminal state. A cycle is skipped when the **Project** already has an active **Plan Run**.
_Avoid_: Plan run retry, planning attempt, scheduler tick

**Pause Reason**:
The typed cause of an **Auto-Replan** transition from `Armed` to `Paused`. Values are `EmptyBacklog`, `AssignmentBlocked`, `PushNonFastForward`, `MergeStagedLeft`, `PlanningFailed`, and `SyncFailed`. The detail string (push stderr, sync error) belongs on the matching **Activity** entry, not duplicated on the **Project** row.
_Avoid_: Freeform pause string, block reason, planner error

**Local Control Plane**:
A **Control Plane** intended to run on the developer's own machine for that developer's Projects.
_Avoid_: Hosted service, team workspace

**agentic-afk**:
The command-line entrypoint for operating the local **Control Plane**.
_Avoid_: afk, dioxus-agentic-afk

## Relationships

- The **Control Plane** belongs to the **Orchestrator**
- The **Dashboard** is the primary interface for the **Control Plane**
- The **Boilerplate** establishes the **Orchestrator** and **Dashboard** without requiring agent-work features
- The **Control Plane** manages zero or more **Projects**
- A **Project** has exactly one **Project ID**
- A **Project** may have one derived **Git Summary**
- A **Project** may have zero or more **Issue Sources**
- An **Issue Source** may provide zero or more **Source Issues**
- A **Source Issue** may be a **Ready Issue**
- A **Source Issue** may have one **Lifecycle Status**
- A **Plan Run** may contain one or more **Issue Assignments**
- A **Ready Issue** may have at most one active **Issue Assignment**
- An **Issue Assignment** may have one or more **Assignment Attempts**
- An **Issue Assignment** has one **Assignment Worktree**
- The **Planning Phase** chooses eligible **Ready Issues** that can start together as parallel issue work
- A **Project** may run issue tasks in parallel up to its **Max Parallel Tasks**
- A **Project** may have at most one active **Plan Run**
- A **Project** has one **Integration Branch** for successful **Plan Runs**
- Selected **Issue Assignments** are claimed before their implementation passes start
- A **Plan Run** starts from the latest fetched and pulled **Integration Branch** baseline
- The **Planning Phase** and selected **Issue Assignments** share the **Plan Run** integration baseline
- A failed **Planning Phase** leaves its **Plan Run** visible for diagnosis
- A **Planning Phase** may finish with an empty successful **Plan Run** when no eligible work is selected
- The **Planning Phase** may choose a **Ready Issue** only when its **Issue Dependencies** are resolved
- Implemented issue work enters the **Review Phase** before it may enter the **Merge Phase**
- An **Issue Assignment** rejected in the **Review Phase** enters a **Review Loop**
- A **Review Loop** is bounded by the **Project** **Review Retry Limit** before its **Issue Assignment** blocks
- Reviewed issue work enters the **Merge Phase** before it is accepted
- The **Merge Phase** attempts to resolve conflicts while integrating reviewed issue work
- The **Merge Phase** verifies the integrated result before it pushes the **Integration Branch**
- The **Merge Phase** may fix integration verification failures before it blocks
- A failed **Merge Phase** blocks its **Plan Run**
- A successful **Merge Phase** updates and pushes the **Project** **Integration Branch**
- A successful **Merge Phase** completes the merged **Source Issues**
- Worktree and issue-branch cleanup for an **Issue Assignment** gates on its terminal **Assignment Status** (`merged` or `blocked`), not on **Plan Run** finish
- A staged **Issue Assignment** in `merge_staged` keeps its worktree and issue branch until it reaches `merged` or `blocked`
- Blocked **Issue Assignments** stay outside the **Merge Phase**
- A **Plan Run** may finish after merging reviewed work while blocked **Issue Assignments** remain
- Dormant blocked and `merge_staged` **Issue Assignments** do not consume **Max Parallel Tasks**
- Blocked issue work must be re-enabled by a human before a later **Plan Run** may select it again
- Finished **Plan Runs** clean up blocked **Issue Assignment** worktrees and issue branches
- Human re-enable is keyed to the **Source Issue**, not its dead **Issue Assignment** row; it clears local blocked state for the latest blocked **Issue Assignment** of that **Source Issue** if one exists, writes Lifecycle `Ready` back to the **Issue Source** (best-effort per ADR-0035 — failure proceeds locally and surfaces an **Activity** entry), and does not redefine **Ready Issue** readiness
- A blocked **Issue Assignment** carries a typed **Block Reason** plus optional human-readable detail; **Block Reasons** include `ReviewRetryLimitExhausted`, `MergePhaseFailed`, `PushNonFastForward`, and `AbandonedStaged`
- A **Plan Run** uses **Phase Prompts** for planning, implementation, review, and merge
- A **Plan Run** preserves **Phase Outputs** after worktrees and issue branches are cleaned up
- Finishing one **Plan Run** does not automatically start another **Planning Phase**
- A **Ready Issue** may have zero or more **Issue Dependencies**
- An **Issue Dependency** is resolved by the accepted result of the **Merge Phase**
- **Source Order** preserves source ordering without deciding **Planning Phase** batch selection
- A **Ready Issue** may belong to one **Parent Issue**
- A **Project** may have zero or more **Activity** entries
- A **Local Control Plane** is operated by one developer on their own machine
- **agentic-afk** operates the **Local Control Plane**
- A **Project** has one **Auto-Replan State**
- An `Armed` **Auto-Replan State** drives **Auto-Replan Cycles** for that **Project**
- An **Auto-Replan Cycle** syncs the **Issue Source** before its **Planning Phase**
- An **Auto-Replan Cycle** preserves the single-active **Plan Run** invariant; if a **Plan Run** is already active the cycle is skipped
- A **Plan Run** that finishes with at least one merged **Issue Assignment** and no blocked **Issue Assignments** and no `merge_staged` **Issue Assignments** allows the next **Auto-Replan Cycle** to continue
- A **Plan Run** that finishes empty-successful, with any blocked **Issue Assignment**, with any `merge_staged` **Issue Assignment**, or with a failed **Planning Phase** transitions **Auto-Replan State** from `Armed` to `Paused`
- A failed **Issue Source** sync inside an **Auto-Replan Cycle** transitions **Auto-Replan State** from `Armed` to `Paused`
- A `Paused` **Auto-Replan State** requires explicit human Resume; backlog change or block resolution does not auto-resume
- **Auto-Replan State** is per-**Project** scalar persistence; `Paused` survives server restart

## Example dialogue

> **Dev:** "Are we wrapping another orchestrator and adding a dashboard?"
> **Domain expert:** "No - this is a Rust-native **Orchestrator**, and the **Dashboard** is the primary way to operate its **Control Plane**."
> **Dev:** "Does the dashboard center the single active assignment?"
> **Domain expert:** "No - the **Dashboard** centers **Plan Runs** and shows their **Issue Assignments** inside them."
> **Dev:** "Does the **Boilerplate** need to run agents?"
> **Domain expert:** "No - the **Boilerplate** only needs the runnable foundation where agent-work features will be added later."
> **Dev:** "Is a **Project** the same thing as a Git repository?"
> **Domain expert:** "No - a **Project** is a local codebase root; it may have Git metadata, but Git does not define the concept."
> **Dev:** "Can the dashboard show sample agent activity?"
> **Domain expert:** "No - the dashboard can reserve space for **Activity**, but it should only show truthful data."
> **Dev:** "Is this a hosted dashboard for a team?"
> **Domain expert:** "No - this is a **Local Control Plane** for one developer's machine."
> **Dev:** "Should the binary be called `afk`?"
> **Domain expert:** "No - use **agentic-afk** so the command is specific enough for daily local use."
> **Dev:** "Can a **Project** be identified by its path?"
> **Domain expert:** "No - paths can move, so a **Project** has a stable **Project ID**."
> **Dev:** "Is Git required for a **Project**?"
> **Domain expert:** "No - Git can provide a derived **Git Summary**, but it does not define whether a local codebase root is a **Project**."
> **Dev:** "Are issues always pulled from GitHub?"
> **Domain expert:** "No - GitHub Issues can be an **Issue Source**, but a **Project** may use local markdown or another configured source."
> **Dev:** "Can the **Control Plane** implement any open issue it finds?"
> **Domain expert:** "No - only a **Ready Issue** is suitable for unattended agent implementation."
> **Dev:** "Does starting one issue by hand decide what runs next?"
> **Domain expert:** "No - the developer starts the **Planning Phase**, and it selects the **Ready Issues** for parallel issue work."
> **Dev:** "Does planner concurrency use the same cap as issue work?"
> **Domain expert:** "No - a **Project** may have only one active **Plan Run**, while **Max Parallel Tasks** caps parallel issue tasks after planning."
> **Dev:** "Can implementers start before selected issues are claimed?"
> **Domain expert:** "No - selected **Issue Assignments** are claimed before their implementation passes start."
> **Dev:** "Can planning inspect a stale integration checkout?"
> **Domain expert:** "No - the **Plan Run** starts from the latest fetched and pulled **Integration Branch** baseline."
> **Dev:** "Should implementation pull again after planning selects a batch?"
> **Domain expert:** "No - selected **Issue Assignments** branch from the same **Plan Run** integration baseline so the plan does not drift."
> **Dev:** "Can the planner edit project files while it chooses the batch?"
> **Domain expert:** "No - the **Planning Phase** inspects the **Project** and issue descriptions but does not edit project files."
> **Dev:** "If the planner fails, does the plan run disappear?"
> **Domain expert:** "No - the failed **Planning Phase** leaves its **Plan Run** visible for diagnosis."
> **Dev:** "If the planner finds no eligible work, is that a failure?"
> **Domain expert:** "No - the **Planning Phase** may finish with an empty successful **Plan Run**."
> **Dev:** "Does one finished batch automatically plan the next batch?"
> **Domain expert:** "No - each **Plan Run** starts from an explicit manual planning trigger for now."
> **Dev:** "If the dashboard disagrees with GitHub about whether an issue is blocked, which wins?"
> **Domain expert:** "The **Source Issue** wins because it is authoritative for issue-work planning."
> **Dev:** "Can implementation progress live only in the local dashboard?"
> **Domain expert:** "No - **Lifecycle Status** should be written back to the **Issue Source** so progress is visible there too."
> **Dev:** "Does the **Issue Source** need a label for every internal phase?"
> **Domain expert:** "No - **Lifecycle Status** stays coarse while the **Control Plane** shows plan and phase detail."
> **Dev:** "Is an issue completed when the implementer finishes editing files locally?"
> **Domain expert:** "No - implemented work enters the **Review Phase** before reviewed work may be accepted through the **Merge Phase**."
> **Dev:** "If review rejects one issue branch, is the issue task replaced?"
> **Domain expert:** "No - the same **Issue Assignment** enters a **Review Loop** and returns for another implementation pass."
> **Dev:** "Can the reviewer patch the branch directly?"
> **Domain expert:** "No - the **Review Phase** approves or rejects with findings; implementation passes make project edits."
> **Dev:** "Can review run checks before approval?"
> **Domain expert:** "Yes - the **Review Phase** may run verification needed for its approval decision."
> **Dev:** "If reviewed branches conflict during merge, does the plan stop immediately?"
> **Domain expert:** "No - the **Merge Phase** attempts to resolve conflicts while integrating the reviewed work."
> **Dev:** "Can merge push reviewed branches without checking the integrated result?"
> **Domain expert:** "No - the **Merge Phase** verifies the integrated result before it pushes the **Integration Branch**."
> **Dev:** "If integrated verification fails, does merge block immediately?"
> **Domain expert:** "No - the **Merge Phase** may fix integration verification failures before it blocks."
> **Dev:** "If the merger still cannot finish, does it retry automatically?"
> **Domain expert:** "No - a failed **Merge Phase** blocks its **Plan Run**."
> **Dev:** "Does a successful merge stop at a reviewable proposal branch?"
> **Domain expert:** "No - the successful **Merge Phase** updates and pushes the **Project** **Integration Branch** directly."
> **Dev:** "Do selected issues stay open after a successful merge?"
> **Domain expert:** "No - a successful **Merge Phase** completes the merged **Source Issues**."
> **Dev:** "Do merged issue branches stay around after the plan finishes?"
> **Domain expert:** "No - finished **Plan Runs** clean up merged **Issue Assignment** worktrees and issue branches."
> **Dev:** "If one issue task blocks, does its branch get merged with the reviewed work?"
> **Domain expert:** "No - the **Merge Phase** may accept reviewed work from the **Plan Run** while blocked **Issue Assignments** stay outside the merge."
> **Dev:** "Does one blocked issue task keep the plan slot forever after the reviewed work merges?"
> **Domain expert:** "No - the **Plan Run** may finish while blocked **Issue Assignments** remain outside the merge."
> **Dev:** "Do dormant blocked assignments prevent new planned work from running?"
> **Domain expert:** "No - only running issue tasks consume **Max Parallel Tasks**."
> **Dev:** "Does blocked issue work automatically return to the next plan?"
> **Domain expert:** "No - blocked issue work must be re-enabled by a human before a later **Plan Run** may select it again."
> **Dev:** "Do blocked issue branches stay around after the plan finishes?"
> **Domain expert:** "No - finished **Plan Runs** clean up blocked **Issue Assignment** worktrees and issue branches."
> **Dev:** "Does cleanup erase planner and reviewer evidence?"
> **Domain expert:** "No - the **Plan Run** preserves **Phase Outputs** after worktrees and issue branches are cleaned up."
> **Dev:** "Does re-enable mean reapplying readiness too?"
> **Domain expert:** "No - human re-enable clears blocked **Lifecycle Status** while **Ready Issue** readiness stays separate."
> **Dev:** "Can agentic-afk accept reviewed issue work itself?"
> **Domain expert:** "Yes - reviewed issue work enters the **Merge Phase** for acceptance."
> **Dev:** "Can a dependent issue start after its blocker has passing checks?"
> **Domain expert:** "No - the blocker must be accepted through the **Merge Phase** before dependent work can start."
> **Dev:** "Can agentic-afk split one large issue across multiple issue tasks?"
> **Domain expert:** "No - one active **Issue Assignment** carries one **Ready Issue** through implementation and the **Review Phase**; parallelism happens across independent issues."
> **Dev:** "Can an assigned agent edit the registered Project checkout directly?"
> **Domain expert:** "No - the **Control Plane** creates an **Assignment Worktree** from the **Plan Run** integration baseline before implementation starts."
> **Dev:** "Can the **Control Plane** run two ready issues in parallel because their titles look unrelated?"
> **Domain expert:** "Yes, if neither has an unresolved **Issue Dependency**; the **Planning Phase** decides which eligible issues enter the batch."
> **Dev:** "Does a parent issue decide which child issue runs first?"
> **Domain expert:** "No - a **Parent Issue** groups related work; **Issue Dependencies** determine what must wait."

## Flagged ambiguities

- "similar to Sandcastle" was resolved to mean a new Rust-native **Orchestrator** with a dashboard-first **Control Plane**, not a wrapper around Sandcastle.
- "boilerplate" was resolved to mean the runnable project foundation, not the first agent-execution feature slice.
- "project" was resolved to mean a local codebase root, not a Git repository, workspace, or agent task.
- "activity" was reserved for real control-plane events, not fabricated dashboard content.
- "dashboard execution view" was resolved around **Plan Runs** with nested **Issue Assignments**, not a single active assignment panel.
- "local only" was resolved as a **Local Control Plane** constraint, not merely an unauthenticated deployment choice.
- "afk" was rejected as too generic for the CLI; **agentic-afk** is the canonical entrypoint name.
- "project identity" was resolved as a stable **Project ID**, not a database row ID or filesystem path.
- "git metadata" was resolved as a derived **Git Summary**, not persisted **Project** state.
- "project repo issues" was resolved as an **Issue Source** for a **Project**, not as part of the **Project** identity.
- "available issues" was resolved to mean issues visible from an **Issue Source**; only `ready-for-agent` **Ready Issues** may be scheduled for agent implementation.
- "plan" was resolved as a manually started **Planning Phase** that selects one immediately runnable parallel issue batch, not a single-issue start action or a waiting backlog.
- "max parallel tasks" was resolved as per-**Project** **Max Parallel Tasks** for parallel issue work, excluding the single active **Plan Run** planner step.
- "review retry bound" was resolved as the per-**Project** **Review Retry Limit** with a platform default.
- "batch" was resolved as a **Plan Run** that owns one Sandcastle-style plan, implementation, review, and merge flow across selected **Issue Assignments**.
- "claim" was resolved as a prerequisite for implementation passes selected by a **Plan Run**.
- "planning baseline" was resolved as one latest fetched and pulled **Integration Branch** baseline shared by the **Planning Phase** and its selected **Issue Assignments**.
- "planner edits" were rejected; the **Planning Phase** inspects project state and issue descriptions to select work.
- "planner failure" was resolved as a visible failed **Plan Run**, not a disappearing attempt.
- "empty plan" was resolved as a successful empty **Plan Run**, not a blocked or failed planner result.
- "next plan" was resolved as another manual trigger, not an automatic planning cycle.
- "control plane issue" was rejected; a **Source Issue** remains authoritative instead of being copied into a separate planning model.
- "lifecycle status" was resolved as status written back to the **Issue Source**, not hidden local dashboard state.
- "phase status" was resolved as control-plane detail, not extra **Lifecycle Status** churn in the **Issue Source**.
- "completed issue" was resolved as reviewed issue work accepted through the **Merge Phase**, not merely finished local edits.
- "change proposal" was removed from the issue flow so acceptance has one canonical path.
- "review" was resolved as the agent-owned **Review Phase**, not an implementer self-check or a human-only step.
- "review edits" were rejected; the **Review Phase** approves or rejects with findings.
- "review verification" was resolved as checks the **Review Phase** may run to decide approval.
- "review rejection" was resolved as a **Review Loop** on the same **Issue Assignment**, not a new assignment or whole-plan restart.
- "phase prompts" was resolved as adapting Sandcastle's prompts for planning, implementation, review, and merge with **Project** instructions in every phase.
- "phase outputs" were resolved as durable plan-run evidence after branch and worktree cleanup.
- "merge conflict" was resolved as work for the **Merge Phase** to attempt before the **Plan Run** blocks.
- "merge verification" was resolved as verification of the integrated result before the **Integration Branch** is pushed.
- "merge verification failure" was resolved as an integration problem the **Merge Phase** may fix before blocking.
- "merge failure" was resolved as blocking the **Plan Run** after the failed **Merge Phase**, not an automatic merge retry loop.
- "merge target" was resolved as the pushed per-**Project** **Integration Branch**, defaulted from the detected default branch rather than left implicit or replaced by a reviewable proposal branch.
- "completion boundary" was resolved as a merged **Source Issue** accepted by a successful **Merge Phase** into the **Integration Branch**.
- "merged artifacts" were resolved as cleaned up with the finished **Plan Run**, not preserved issue branches or worktrees.
- "blocked work in a batch" was resolved as blocked **Issue Assignments** outside the **Merge Phase**, while reviewed work from the same **Plan Run** may still merge.
- "partial plan success" was resolved as a finished **Plan Run** with merged reviewed work and blocked **Issue Assignments** left outside that merge.
- "blocked capacity" was resolved so dormant blocked **Issue Assignments** do not consume **Max Parallel Tasks**.
- "blocked requeue" was rejected; blocked issue work requires human re-enable before later planning.
- "blocked artifacts" were resolved as cleaned up with the finished **Plan Run**, not preserved branches or worktrees for recovery.
- "re-enable" was resolved as a **Source Issue**-keyed action that clears local blocked state on the latest blocked **Issue Assignment** (if any) and writes Lifecycle `Ready` back to the **Issue Source** (best-effort per ADR-0035 — failure proceeds locally with an **Activity** entry) so the next **Plan Run** snapshot buckets the **Source Issue** as eligible; it does not redefine `ready-for-agent` readiness.
- "block reason" was resolved as a typed **Block Reason** kind plus optional freeform detail, not a freeform-only string, to support per-cause dashboard affordances and observable taxonomy.
- "merge" was resolved as the agent-owned **Merge Phase**.
- "dependency resolved" was resolved as acceptance through the **Merge Phase**, not merely successful implementation or review.
- "parallel work" was resolved as parallel **Issue Assignments** across different **Ready Issues**, not multiple issue tasks for one issue.
- "agent workspace" was resolved as an **Assignment Worktree** created from the **Plan Run** integration baseline, not the mutable **Project** path.
- "assignment branch" was resolved as a branch derived from the **Source Issue** identity, not a title-based or agent-invented branch name.
- "agent retry" was resolved as another **Assignment Attempt** inside the existing **Issue Assignment**, not a new assignment branch.
- "fresh retry" was resolved as human re-enable followed by a later **Plan Run**, not assignment abandonment.
- "recover" was removed from the core flow in favor of later Sandcastle-style **Plan Runs** starting fresh from eligible source issues.
- "blocker" was resolved as an explicit **Issue Dependency** recorded in the **Source Issue** description, not a GitHub blocked-by relationship or inferred file-level independence.
- "next issue" was resolved by the **Planning Phase** among eligible **Ready Issues**, not by **Source Order** alone.
- "parent issue" was resolved as grouping metadata, not execution order.
- "AFK cadence" was resolved as **Auto-Replan**, a per-**Project** tri-state (`Off`/`Armed`/`Paused`) driver that runs consecutive **Auto-Replan Cycles** while `Armed` and pauses (not stops) on any condition that needs human attention. Continuous spinning on identical backlogs is avoided by pausing on empty-successful **Plan Runs** rather than backing off, on the rationale that a drained backlog is a human-attention signal, not a transient state.
- "auto-replan resume" was rejected as automatic on backlog or block resolution; resume is an explicit human action that flips `Paused` → `Armed`, recorded as an **Activity** entry. Auto-resume was rejected because coupling two state machines (`Auto-Replan State` and `Lifecycle Status`) hides intent and surprises a developer who walked away expecting the loop stopped.
- "auto-replan cadence" was resolved as a single tick driver in the control-plane server scanning all `Armed` **Projects** on a fixed cooldown (60s default, no per-**Project** knob yet). Per-project tokio tasks were rejected as N task lifetimes to manage with no felt-need yet.
- "auto-replan issue source freshness" was resolved as bundled: each **Auto-Replan Cycle** runs `sync_issue_source` immediately before its **Planning Phase**. An independent refresh timer was rejected to avoid two-loop interaction; the cost is at most one **Plan Run** of staleness, which is bounded by the existing single-active **Plan Run** invariant.
- "push failure after local merge" is resolved by the `merge_staged` **Assignment Status**: the **Merge Phase** integrates locally and verifies, records `merge_staged`, then pushes; only on push success does it transition to `merged` and write Lifecycle `Completed`. If the push fails, the **Issue Assignment** stays `merge_staged` and the **Plan Run** finishes `failed`. Recovery is an operator-initiated **Retry Push** (push only — no re-verify, non-fast-forward auto-routes the **Issue Assignment** to `blocked`) or **Abandon Staged** (routes to `blocked`). `merge_staged` is dormant for **Max Parallel Tasks**. Worktree and issue-branch cleanup gate on the **Issue Assignment** reaching a terminal status (`merged` or `blocked`), not on the **Plan Run** finishing; a `failed` **Plan Run** stays `failed` even when a later retry advances the staged **Issue Assignment** to `merged`.
