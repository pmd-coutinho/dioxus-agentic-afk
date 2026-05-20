# Use source-authoritative issue workflow

The Control Plane will discover work from configured Issue Sources and treat Source Issues as authoritative for readiness, dependencies, parent grouping, and acceptance criteria. It may normalize this data for scheduling and dashboard display, but it should write Lifecycle Status and Change Proposal links back to the Issue Source rather than maintaining a separate planning model.

Ready Issues may run in parallel only across separate Issue Assignments when they have no unresolved Issue Dependencies, with Source Order deciding which eligible issues fill available slots. A Verified Change Proposal frees the agent slot for unrelated work, but dependent issues remain blocked until Human Merge resolves the dependency.

Claiming a Ready Issue will be enforced by an atomic persisted Issue Assignment in the Control Plane before an agent is spawned. Lifecycle Status is then written back to the Issue Source; if that write-back fails, the Control Plane releases the local Issue Assignment and does not spawn the agent.

Future scheduling excludes claimed work using both layers: the local Issue Assignment is the atomic lock for this Control Plane, and the Issue Source Lifecycle Status is the cross-process and cross-machine signal that the issue is already taken.

**Considered Options**

- Copy issues into a Control Plane-owned planning database.
- Infer independence from issue text, changed files, or semantic similarity.
- Treat passing but unmerged Change Proposals as sufficient to unblock dependent work.
- Auto-merge successful Change Proposals.
- Keep Lifecycle Status only in the local dashboard.
- Let agents claim issues by writing directly to the Issue Source without a local atomic assignment.
