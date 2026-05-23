---
status: accepted
---

# Re-enable is Source-Issue-keyed and writes Lifecycle Ready back

Human re-enable of blocked issue work is keyed to the **Source Issue**, not to the dead **Issue Assignment** row it most recently produced. The endpoint clears local blocked state on the latest blocked **Issue Assignment** of the **Source Issue** if one still exists, and writes Lifecycle `Ready` back to the **Issue Source** so the next **Plan Run** planning snapshot buckets the **Source Issue** as `eligible` instead of `active`.

Without the upstream write-back, the **Planning Snapshot** normalizer (`crates/planning-snapshot/src/lib.rs`) keeps any **Source Issue** with Lifecycle `Blocked` in the `active` bucket forever; a previous **Plan Run** that wrote `Blocked` upstream therefore poisons all future plans for that **Source Issue**. The local-only clear that existed before this decision was a no-op for the planner.

Write-back follows ADR-0035: re-enable is post-Claim, so a failed Lifecycle write does not block the local clear. The local re-enable proceeds and the failure surfaces as an **Activity** entry. This keeps re-enable available when the **Issue Source** is briefly unreachable (for example, `gh` rate-limited) and preserves the operator's local view without coupling recovery to upstream availability.

Re-enable is keyed by **Source Issue** identity, not by **Issue Assignment** identity, because the **Source Issue** is the durable authority (ADR-0025) and the **Issue Assignment** row is an ephemeral execution record whose worktree and issue branch have already been cleaned up by the time most re-enables happen. A button bound to a dead **Issue Assignment** row leaks an implementation detail into the operator UI; the load-bearing effect of re-enable (upstream Lifecycle write-back) belongs to the **Source Issue** anyway.

**Block Reason** is promoted from a freeform `Option<String>` to a typed kind plus optional freeform detail. Typed kinds are `ReviewRetryLimitExhausted`, `MergePhaseFailed`, `PushNonFastForward`, and `AbandonedStaged` (the last two introduced by ADR-0037). The typed kind drives dashboard affordances (which badge, which recovery action, which filter) and gives tests a stable assertion target; the detail carries cause-specific specifics like push stderr that have no place in a closed enum.

**Considered Options**

- Keep re-enable Assignment-keyed and skip Lifecycle write-back, fix the planner instead by joining Lifecycle `Blocked` with active Assignment rows to infer eligibility. Rejected because the planner would silently override the **Issue Source** authority, hiding the upstream/local mismatch from the operator and breaking ADR-0025.
- Re-enable Assignment-keyed, but with Lifecycle write-back. Rejected because the API surface still pivots on a dead Assignment row post-cleanup, confusing the operator mental model; the Source Issue keying is the same change with a cleaner identity.
- Write a new Lifecycle value (e.g. `Re-enabled` or `Reopened`) instead of `Ready`. Rejected because the planner already treats Lifecycle `Ready` as eligible; a parallel value would force a second planner branch for no semantic gain.
- Block local clear on Lifecycle write-back failure. Rejected because it makes re-enable depend on **Issue Source** availability; an operator could not unblock work during a `gh` outage.
- Keep `block_reason` as a freeform string. Rejected because with `PushNonFastForward` and `AbandonedStaged` joining `ReviewRetryLimitExhausted` and `MergePhaseFailed`, the **Control Plane** needs to branch on cause for per-reason affordances and reasoning that branches on string matching is fragile.
- Pure typed enum with no detail string. Rejected because cause-specific text (push stderr, conflict file list, verification log tail) has no fixed schema and is genuinely useful for the operator.
