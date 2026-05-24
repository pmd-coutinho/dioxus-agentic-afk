---
status: accepted
---

# Plan Run State is lifecycle-only

**Plan Run State** is limited to `Running` and `Finished`. A **Plan Run** state records whether the run is still active; it does not encode the active run's current position or the finished run's outcome. Active progress is derived as **Plan Run Stage**, while empty backlog, planning failure, blocked assignments, merge-staged recovery, and successful merged work are derived as **Plan Run Outcome** from **Phase Outputs** and **Issue Assignments**.

**Plan Run Stage** aggregates parallel **Issue Assignments** by the least advanced active stage, so it shows the run's current bottleneck rather than the furthest-progressed assignment. If one assignment is reviewing while another is still implementing, the **Plan Run Stage** is `Implementing`.

## Considered Options

- **Outcome-coded Plan Run states** such as `succeeded`, `succeeded_empty`, `failed`, and `finished`. Rejected because the same facts already live on **Phase Outputs** and **Issue Assignments**, and duplicating them into `plan_runs.state` creates drift between Auto-Replan, Dashboard rendering, boot recovery, and persistence tests.
- **An `InProgress` Plan Run outcome for active runs.** Rejected because active progress is not a terminal result. Treating it as an outcome would recreate outcome-coded state under a new field name.
- **A separate persisted Plan Run outcome column.** Rejected for now because the outcome can be reconstructed from durable child facts. If later querying proves too expensive, a cached outcome can be added as a projection, not as the domain source of truth.

## Consequences

- The contract should expose a typed `PlanRunState` with only `Running` and `Finished`.
- The contract should expose derived `PlanRunStage` for active runs and derived `PlanRunOutcome` for finished runs.
- **Plan Run Stage** should be derived server-side by least-advanced active stage: `Planning`, `Implementing`, `Reviewing`, `Merging`, then `Pushing`. Restart recovery is surfaced through **Activity**, **Block Reason**, and **Plan Run Outcome**, not as an active stage.
- Persistence should stop writing outcome-coded terminal strings to `plan_runs.state`.
- Auto-Replan and Dashboard presentation should classify **Plan Run Stage** and **Plan Run Outcome** from **Phase Outputs** and **Issue Assignments**, not from state string names.
