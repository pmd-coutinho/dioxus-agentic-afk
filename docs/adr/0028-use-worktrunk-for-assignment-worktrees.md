# Use Worktrunk for Assignment Worktrees

The first agent-execution slice will require Worktrunk to create and remove one Assignment Worktree per Issue Assignment instead of adding a raw `git worktree` fallback. The Control Plane owns assignment lifecycle: it creates issue branches from the Plan Run integration baseline, keeps each Issue Assignment isolated through implementation and review, and removes Assignment Worktrees and issue branches when the Plan Run finishes. The Plan Run refreshes the configured Integration Branch once before planning so selected Issue Assignments share one batch baseline.

Project registration and Issue Source planning may still inspect non-Git Projects, but agent execution requires a Git-backed Project because Assignment Worktrees and issue branches depend on Git.

GitHub-backed Plan Run creation fails before claim when the Project Git remote does not match the enabled GitHub Issue Source repository. That keeps issue write-back, Integration Branch push, and completion on the same first-slice repository contract.

Assignment claim setup is local first and source-visible only when the Assignment Worktree is ready. The Control Plane persists a provisional Issue Assignment, creates and refreshes its Assignment Worktree, writes `Claimed` to the Issue Source, marks the local claim ready, spawns the agent, and writes `Running` only after the agent process starts. If Assignment Worktree setup or `Claimed` write-back fails before the agent starts, the Control Plane releases the local claim and removes the fresh worktree.

Execution preflight failures happen before that claim boundary. Missing Worktrunk, non-Git Projects, GitHub auth failures, or GitHub source/remote mismatches are surfaced from the manual Plan Run trigger without writing a claimed or blocked Source Issue lifecycle state.

One live phase attempt owns an Issue Assignment at a time. The Control Plane persists enough process identity and phase output history for supervision and diagnosis; if running issue work blocks, its Source Issue stays blocked until a human re-enables it for a later Plan Run.

The Project Max Parallel Tasks setting bounds concurrent issue work after planning. The single active Plan Run rule keeps planning and merge batch ownership explicit while independent Issue Assignments run in parallel within that bound.

**Considered Options**

- Let the assigned agent create its own worktree.
- Fall back to raw `git worktree` when Worktrunk is unavailable.
- Replace a prior blocked issue branch automatically when the Source Issue is re-enabled later.
