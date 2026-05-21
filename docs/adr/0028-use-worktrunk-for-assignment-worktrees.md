# Use Worktrunk for Assignment Worktrees

The first agent-execution slice will require Worktrunk to create and remove one Assignment Worktree per Issue Assignment instead of adding a raw `git worktree` fallback. The Control Plane owns assignment lifecycle: it creates the branch as `agentic-afk/<source-id>` from the Project default branch, keeps existing assignment work from being silently replaced, and removes the Assignment Worktree and branch when an Issue Assignment is explicitly abandoned. GitHub-backed assignments fetch and rebase onto the latest remote default branch before agent spawn; remote-less Git-backed local markdown assignments may run from the local default branch.

Project registration and Issue Source planning may still inspect non-Git Projects, but agent execution requires a Git-backed Project because Assignment Worktrees and proposal branches depend on Git.

GitHub-backed assignment creation fails before claim when the Project Git remote does not match the enabled GitHub Issue Source repository. That keeps issue write-back, branch push, pull request creation, checks, and merge detection on the same first-slice repository contract.

Assignment claim setup is local first and source-visible only when the Assignment Worktree is ready. The Control Plane persists a provisional Issue Assignment, creates and refreshes its Assignment Worktree, writes `Claimed` to the Issue Source, marks the local claim ready, spawns the agent, and writes `Running` only after the agent process starts. If Assignment Worktree setup or `Claimed` write-back fails before the agent starts, the Control Plane releases the local claim and removes the fresh worktree.

Execution preflight failures happen before that claim boundary. Missing Worktrunk, non-Git Projects, GitHub auth failures, or GitHub source/remote mismatches are surfaced from the explicit start action without writing a claimed or blocked Source Issue lifecycle state.

One live agent process owns an Issue Assignment at a time. The Control Plane persists enough process identity to verify and stop that owned process during recovery; if a running assignment loses its agent process, it becomes blocked rather than silently spawning a replacement. Recovery continues in the existing Assignment Worktree under one replacement agent with the original Source Issue brief plus verified recovery facts such as the block reason, branch and worktree identity, diff summary, and recent commits.

The first execution slice runs at most one active Issue Assignment per Project and exposes that limit in the Dashboard when other eligible Ready Issues are waiting. This limits early execution failure combinations without changing the domain rule that independent Issue Assignments may run in parallel later.

**Considered Options**

- Let the assigned agent create its own worktree.
- Fall back to raw `git worktree` when Worktrunk is unavailable.
- Replace an existing assignment branch automatically when the same Source Issue is retried.
