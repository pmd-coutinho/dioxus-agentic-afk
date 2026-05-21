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
The current implementation state of a **Source Issue** as reported by the **Control Plane** back to the **Issue Source**. The minimal statuses are Ready, Claimed, Running, Blocked, and Completed.
_Avoid_: Local-only status, dashboard state, hidden progress

**Change Proposal**:
A proposed code change produced for a **Source Issue** and linked back to that issue so the issue can close when the change is accepted.
_Avoid_: Local patch, hidden branch, untracked work

**Verified Change Proposal**:
A **Change Proposal** whose required checks have succeeded and no longer needs active agent work.
_Avoid_: Open pull request, unchecked proposal, local success

**Repair Loop**:
The continuation of agent work on a **Change Proposal** after required checks fail, bounded by both retry count and elapsed time.
_Avoid_: Infinite retry, new issue, manual-only fix

**Issue Assignment**:
The ownership of one **Ready Issue** by one agent while that issue is being implemented.
_Avoid_: Shared issue work, automatic sub-agent split, pooled task

**Assignment Attempt**:
One agent execution pass within an **Issue Assignment**, such as the initial run, a recovery run, or a repair run.
_Avoid_: Assignment, retry branch, agent log

**Abandoned Assignment**:
An **Issue Assignment** whose current work has been explicitly discarded so the same **Source Issue** may receive a fresh assignment. Its **Assignment Worktree** is removed when abandonment is accepted.
_Avoid_: Silent retry, branch collision, automatic replacement

**Recovered Assignment**:
A **Blocked** **Issue Assignment** that continues in its existing **Assignment Worktree** under one replacement agent after any still-owned prior agent process is stopped.
_Avoid_: Fresh retry, silent restart, parallel takeover

**Assignment Worktree**:
The isolated worktree created for one **Issue Assignment** from the **Project** default branch before the assigned agent starts. Its branch is derived from the **Source Issue** identity.
_Avoid_: Project checkout, agent workspace, mutable Project path

**Human Merge**:
The rule that accepting a **Change Proposal** remains a human decision rather than an automatic **Control Plane** action.
_Avoid_: Auto-merge, unattended acceptance

**Ready Issue**:
An issue from an **Issue Source** that has been explicitly marked as suitable for unattended agent implementation.
_Avoid_: Open issue, available issue, backlog item

**Issue Dependency**:
A relationship showing that one **Ready Issue** must wait for another issue before it can be implemented.
_Avoid_: Guess, similarity, priority

**Source Order**:
The ordering of **Source Issues** as provided by the **Issue Source** for choosing the next eligible **Ready Issue**.
_Avoid_: Inferred priority, dashboard order, random queue

**Parent Issue**:
An issue that groups related **Ready Issues** into a larger body of work without itself defining the execution order.
_Avoid_: Epic, milestone, project

**Activity**:
A chronological record of noteworthy control-plane events for a **Project**, including **Issue Assignment** and **Assignment Attempt** lifecycle changes.
_Avoid_: Fake metrics, agent output

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
- A **Source Issue** may have one or more **Change Proposals**
- A **Ready Issue** may have at most one active **Issue Assignment**
- An **Issue Assignment** may have one or more **Assignment Attempts**
- An **Issue Assignment** may become an **Abandoned Assignment**
- A **Blocked** **Issue Assignment** may become a **Recovered Assignment**
- An **Issue Assignment** has one **Assignment Worktree**
- A **Change Proposal** may become a **Verified Change Proposal**
- A **Change Proposal** with failed checks may enter a **Repair Loop**
- A **Change Proposal** requires **Human Merge** before it is accepted
- A **Ready Issue** may have zero or more **Issue Dependencies**
- An **Issue Dependency** is resolved by **Human Merge**, not by a **Verified Change Proposal**
- **Source Order** decides which eligible **Ready Issues** fill available **Issue Assignments**
- A **Ready Issue** may belong to one **Parent Issue**
- A **Project** may have zero or more **Activity** entries
- A **Local Control Plane** is operated by one developer on their own machine
- **agentic-afk** operates the **Local Control Plane**

## Example dialogue

> **Dev:** "Are we wrapping another orchestrator and adding a dashboard?"
> **Domain expert:** "No - this is a Rust-native **Orchestrator**, and the **Dashboard** is the primary way to operate its **Control Plane**."
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
> **Dev:** "If the dashboard disagrees with GitHub about whether an issue is blocked, which wins?"
> **Domain expert:** "The **Source Issue** wins because it is authoritative for issue-work planning."
> **Dev:** "Can implementation progress live only in the local dashboard?"
> **Domain expert:** "No - **Lifecycle Status** should be written back to the **Issue Source** so progress is visible there too."
> **Dev:** "Is an issue completed when the agent finishes editing files locally?"
> **Domain expert:** "No - completion requires a **Verified Change Proposal** so the **Source Issue** can close when the change is accepted."
> **Dev:** "If CI fails, is the issue done from the agent's point of view?"
> **Domain expert:** "No - failed checks put the **Change Proposal** into a bounded **Repair Loop**."
> **Dev:** "Can agentic-afk merge its own pull request when checks pass?"
> **Domain expert:** "No - **Human Merge** keeps acceptance of a **Change Proposal** under developer control."
> **Dev:** "Can a dependent issue start after its blocker has a passing pull request?"
> **Domain expert:** "No - the blocker must be accepted through **Human Merge** before dependent work can start."
> **Dev:** "Can agentic-afk split one large issue across multiple agents?"
> **Domain expert:** "No - one active **Issue Assignment** owns one **Ready Issue**; parallelism happens across independent issues."
> **Dev:** "Can an assigned agent edit the registered Project checkout directly?"
> **Domain expert:** "No - the **Control Plane** creates an **Assignment Worktree** from the Project default branch before the assigned agent starts."
> **Dev:** "Can a new agent replace the old branch for the same issue if it already exists?"
> **Domain expert:** "Only after the current **Issue Assignment** becomes an **Abandoned Assignment** so its work is explicitly discarded."
> **Dev:** "Is a repair pass a new assignment?"
> **Domain expert:** "No - it is another **Assignment Attempt** in the same **Issue Assignment** and **Assignment Worktree**."
> **Dev:** "If an agent process is lost, does recovery start from a fresh checkout?"
> **Domain expert:** "No - a **Recovered Assignment** continues in the existing **Assignment Worktree** under one replacement agent."
> **Dev:** "Can the **Control Plane** run two ready issues in parallel because their titles look unrelated?"
> **Domain expert:** "Yes, if neither has an unresolved **Issue Dependency**; **Source Order** decides which eligible issues fill available assignments first."
> **Dev:** "Does a parent issue decide which child issue runs first?"
> **Domain expert:** "No - a **Parent Issue** groups related work; **Issue Dependencies** determine what must wait."

## Flagged ambiguities

- "similar to Sandcastle" was resolved to mean a new Rust-native **Orchestrator** with a dashboard-first **Control Plane**, not a wrapper around Sandcastle.
- "boilerplate" was resolved to mean the runnable project foundation, not the first agent-execution feature slice.
- "project" was resolved to mean a local codebase root, not a Git repository, workspace, or agent task.
- "activity" was reserved for real control-plane events, not fabricated dashboard content.
- "local only" was resolved as a **Local Control Plane** constraint, not merely an unauthenticated deployment choice.
- "afk" was rejected as too generic for the CLI; **agentic-afk** is the canonical entrypoint name.
- "project identity" was resolved as a stable **Project ID**, not a database row ID or filesystem path.
- "git metadata" was resolved as a derived **Git Summary**, not persisted **Project** state.
- "project repo issues" was resolved as an **Issue Source** for a **Project**, not as part of the **Project** identity.
- "available issues" was resolved to mean issues visible from an **Issue Source**; only **Ready Issues** may be scheduled for agent implementation.
- "control plane issue" was rejected; a **Source Issue** remains authoritative instead of being copied into a separate planning model.
- "lifecycle status" was resolved as status written back to the **Issue Source**, not hidden local dashboard state.
- "completed issue" was resolved as having a linked **Verified Change Proposal**, not merely finished local edits or an unchecked pull request.
- "failed CI" was resolved as a bounded **Repair Loop**, not immediate completion or an unrelated new issue.
- "merge" was resolved as **Human Merge**, not automatic acceptance by the **Control Plane**.
- "dependency resolved" was resolved as **Human Merge**, not a passing but unmerged **Change Proposal**.
- "parallel work" was resolved as parallel **Issue Assignments** across different **Ready Issues**, not multiple agents collaborating on one issue.
- "agent workspace" was resolved as an **Assignment Worktree** created before the assigned agent starts, not the mutable **Project** path.
- "assignment branch" was resolved as a branch derived from the **Source Issue** identity, not a title-based or agent-invented branch name.
- "agent retry" was resolved as another **Assignment Attempt** inside the existing **Issue Assignment**, not a new assignment branch.
- "fresh retry" was resolved as a new assignment after an **Abandoned Assignment**, not silent replacement of an existing assignment branch.
- "recover" was resolved as continuing a blocked assignment in its existing **Assignment Worktree** under one replacement agent, not a fresh retry.
- "not blocked by each other" was resolved as no unresolved **Issue Dependency**, not inferred file-level or semantic independence.
- "next issue" was resolved by **Source Order** among eligible **Ready Issues**, not by an internal priority model.
- "parent issue" was resolved as grouping metadata, not execution order.
