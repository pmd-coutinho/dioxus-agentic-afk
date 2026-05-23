---
status: accepted
---

# Typed Phase Output bodies with Push as its own recording slice

**Phase Output** bodies are persisted as a typed `PhaseOutputBody` enum tagged on the `phase` discriminator, with variants `Planning`, `Implementation`, `Review`, `Merge`, `Push`, and `Failed`. The on-disk encoding stays JSON in the existing `body_json` TEXT column (`crates/persistence/migrations/0015_create_plan_runs.sql`); only the in-memory and wire shape becomes typed. Outer `outcome: String` and `recorded_at` remain unchanged. The persistence write seam (`crates/persistence/src/plan_run.rs`) validates that `outcome` and body variant pair sensibly (e.g. `Review` body with `outcome="approved"` is allowed; `Implementation` body with `outcome="merged"` is not), failing fast at the single chokepoint.

The **Integration Branch** push is recorded as its own `Push` **Phase Output** row, scoped to the **Plan Run** (`assignment_id = None`), not duplicated as a `Merge`/`failed` row per merged **Issue Assignment** as today. One row per push attempt, append-only: per ADR 0037 a `failed` **Plan Run** can be followed by **Retry Push**, and each retry records a new `Push` row whose `outcome` is `succeeded` or `failed`. The body carries `stderr`, `fast_forward: bool`, and `attempt: u32`.

Phase Output bodies are truncated at the write seam to a 64 KB ceiling, with a `truncated_at: <bytes>` marker appended so the **Dashboard** can render a `[truncated]` tail. Retention is otherwise unbounded; a **Local Control Plane** running on a developer machine accumulates phase outputs at a rate that does not warrant pruning policy at this stage.

**Considered Options**

- Keep `body_json: serde_json::Value`. Rejected because writers in `crates/orchestrator/src/coordinator.rs` already produce structurally distinct shapes per phase (commits/verification/gaps for implementation, findings/verification/gaps for review, push stderr for the failure path) and the **Dashboard** needs to reach into specific fields per phase to surface review findings, merge verification, and push diagnostics; doing so on a free Value is stringly-typed at every render site.
- Fold `outcome` into the body variants (`Review::Approved { ... } | Review::Rejected { ... }`). Rejected because most body fields do not differ by outcome (a rejected review still carries findings + verification), variant count explodes, and the `outcome` column is the SQL-level filter used to query "all failed phase outputs for plan run X" — a flat column scan instead of a JSON probe.
- Record push failure as duplicated per-assignment `Merge`/`failed` rows (the current code in `coordinator.rs:1199-1208`). Rejected because push is a Plan-Run-scoped action — one `git push`, N merged **Issue Assignments**. Duplicating the same stderr per assignment is artificial and makes the **Retry Push** action history harder to read.
- Bound phase output retention by age or count. Rejected for now because **Local Control Plane** cadence is human-driven and disk is not the constraint; the 64 KB body truncate prevents runaway growth on a single phase output (push stderr or verification log dumps) without inventing a config knob.
- Per-row body size truncation only at read time. Rejected because runaway bodies would still hit disk and replicate via SSE deltas before being elided; truncation at the write seam keeps the bound honest end-to-end.
