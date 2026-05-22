---
status: superseded by ADR-0034
---

# Control Plane creates Change Proposals

The Control Plane will create and link Change Proposals for completed Issue Assignment work instead of relying on the assigned agent to open and wire them up. Agents produce the branch state inside the Assignment Worktree; the Control Plane owns the proposal lifecycle boundary so Source Issue links and completion semantics do not depend on prompt compliance.

Process exit alone is not proposal readiness. An assigned agent reports one explicit terminal outcome in the first execution slice: `ReadyForProposal`, `Blocked`, or `Failed`; unexpected process loss is handled separately by the Control Plane as a blocked assignment.

After `ReadyForProposal`, the Control Plane pushes the assignment branch and creates the Change Proposal before remote required checks decide whether it becomes a Verified Change Proposal. In the first execution slice, GitHub Issue Sources use pull requests in the same GitHub repository as Change Proposals; local markdown Issue Sources without a proposal target stop at `ReadyForProposal` and become blocked rather than completed.

The Issue Assignment remains active while required checks are pending, but no idle agent process waits on those checks. The Control Plane monitors the Change Proposal and spawns a repair agent in the same Assignment Worktree only when required checks fail, using a repair brief with the original Source Issue brief, proposal identity, failed-check facts, and verified worktree state. Repair Loops are bounded by both repair Assignment Attempt count and elapsed repair window; human-triggered recovery attempts are outside that CI repair budget. Exceeding either repair bound blocks the assignment.

When required checks pass, the Change Proposal becomes a Verified Change Proposal and releases the agent slot while preserving its branch and Assignment Worktree for review. Human Merge is detected from the proposal host; after merge, the Control Plane writes `Completed` back to the Issue Source and removes the accepted Assignment Worktree and deterministic assignment branch.

For GitHub Issue Sources, the Control Plane uses GitHub issue labels for lifecycle status and comments for human-readable proposal links or reasons instead of editing the Source Issue body. Enabling execution against a GitHub Issue Source idempotently ensures the `agentic-afk:claimed`, `agentic-afk:running`, `agentic-afk:blocked`, and `agentic-afk:completed` labels exist; `ready-for-agent` remains the readiness signal. Lifecycle write-back keeps at most one `agentic-afk:*` lifecycle label on the issue at a time, comments blocked issues with their reason and recovery options, and comments Source Issues with Change Proposal links as soon as their pull requests exist.

GitHub lifecycle write-back leaves `ready-for-agent` on claimed and running issues. Future scheduling excludes open issues that carry active AFK lifecycle labels such as claimed, running, or blocked even when they remain ready; completed merged issues normally leave the open-issue sync set through GitHub closure.

GitHub pull request bodies link back to their Source Issues with native closing syntax so Human Merge also drives the host issue relationship. The Control Plane still detects that merge, writes `Completed`, and performs Assignment Worktree cleanup.

When a GitHub-backed Issue Assignment is explicitly abandoned, the Control Plane comments that its work was discarded, removes its AFK lifecycle label, and leaves `ready-for-agent` unchanged so the Source Issue can receive a fresh assignment if it is still ready.

**Considered Options**

- Ask each assigned agent to create and link its own Change Proposal.
- Treat local branch completion as enough until a human creates a proposal.
