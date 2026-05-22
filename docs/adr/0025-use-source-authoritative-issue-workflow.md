# Use source-authoritative issue workflow

The Control Plane will discover work from configured Issue Sources and treat Source Issues as authoritative for readiness, issue-description dependencies, parent grouping, and acceptance criteria. It may normalize this data for planning and dashboard display, but it should write coarse Lifecycle Status back to the Issue Source rather than maintaining a separate source-of-truth planning model.

Ready Issues may run in parallel only across separate Issue Assignments when they have no unresolved Issue Dependencies. A manually started Plan Run chooses the immediately runnable batch among eligible `ready-for-agent` issues within Project capacity, while Source Order remains source ordering rather than the final batch scheduler. Dependent issues remain blocked until the blocking Source Issue is accepted through the Merge Phase.

Claiming a selected Ready Issue will be enforced by an atomic persisted Issue Assignment in the Control Plane before its implementation pass starts. Lifecycle Status is then written back to the Issue Source; if that write-back fails, the Control Plane releases the local Issue Assignment and does not start the implementation pass.

Future scheduling excludes claimed work using both layers: the local Issue Assignment is the atomic lock for this Control Plane, and the Issue Source Lifecycle Status is the cross-process and cross-machine signal that the issue is already taken.

**Considered Options**

- Copy issues into a Control Plane-owned planning database.
- Let hidden local planner state replace Source Issue readiness or blockers.
- Treat successful implementation or review as sufficient to unblock dependent work.
- Keep Lifecycle Status only in the local dashboard.
- Let agents claim issues by writing directly to the Issue Source without a local atomic assignment.
