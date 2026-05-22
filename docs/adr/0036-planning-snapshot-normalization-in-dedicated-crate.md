---
status: accepted
---

# Planning Snapshot normalization lives in a dedicated crate

**Planning Snapshot** bucketing (sorting **Source Issues** into `eligible`, `dependency_blocked`, `active`, `completed`, `non_ready`) lives in a dedicated `agentic-afk-planning-snapshot` crate. The persistence layer reads raw `planning_snapshot_issues` rows and returns a `RawPlanningSnapshot` value; every consumer — the **Planning Phase** in the orchestrator, server handlers serving the `/api` snapshot route, and the sync handler republishing the **Dashboard** delta — calls `planning_snapshot::normalize` to produce the bucketed `PlanningSnapshotResponse`.

Previously the bucketing logic lived inside `persistence::get_planning_snapshot` alongside the SQL read. The rules that define which **Source Issue** counts as `eligible` versus `dependency_blocked` versus `active` are **Planning Phase** scheduling rules, not data-access concerns; co-locating them with SQL made the rules unreadable without a database and forced any change to an **Issue Source** kind to ripple across `contracts`, `persistence`, and the handler. Pulling normalization into its own crate lets the rules be expressed and tested as pure functions over `Vec<SourceIssueSnapshot>`, and lets persistence shrink to row CRUD.

The crate is a separate workspace member rather than a module inside `agentic-afk-orchestrator` because the **Planning Snapshot** is consumed by code that should not depend on the orchestrator (the `/api` snapshot route, the **Dashboard** SSE delta payload). A standalone crate exposes the domain to all consumers without dragging orchestrator dependencies into the server's read path, and the crate boundary makes "all bucket rules live here" enforceable by the compiler.

The collision between the `dependency_blocked` bucket name and the `Blocked` **Lifecycle Status** is preserved deliberately rather than renamed away. They are distinct concepts (planner skipping for the current run versus needing human re-enable across runs) and forcing one to use an unrelated word would hide the symmetry. The glossary in `CONTEXT.md` carries the disambiguation.

**Considered Options**

- Keep normalization inside `persistence::get_planning_snapshot`. Rejected because **Planning Phase** rules cannot be reasoned about without a database and adding an **Issue Source** kind requires edits across multiple crates.
- Put normalization in `agentic-afk-contracts` alongside the wire types. Rejected because ADR-0012 reserves `contracts` for serialization shapes and adding scheduling logic there blurs the wire/domain split.
- Put normalization in `agentic-afk-orchestrator`. Rejected because the `/api` snapshot route and SSE payload would then transitively depend on orchestrator internals, inverting the intended dependency direction.
- Keep `persistence::get_planning_snapshot` returning the bucketed `PlanningSnapshotResponse` as a convenience wrapper that calls `normalize` internally. Rejected because the goal is for persistence to carry zero domain logic so that adding a new **Issue Source** kind never touches the persistence crate; a convenience wrapper leaves the dependency in place.
