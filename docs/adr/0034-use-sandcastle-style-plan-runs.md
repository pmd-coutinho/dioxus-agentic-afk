---
status: accepted
---

# Use Sandcastle-style plan runs

Issue execution will follow Sandcastle's batch flow instead of the earlier single-assignment Change Proposal flow. A developer manually starts one Plan Run for a Project; it refreshes the configured Integration Branch once, plans a runnable batch of `ready-for-agent` Source Issues whose description blockers are resolved, runs bounded parallel issue tasks through implementation and review, merges reviewed successes with one merger agent, verifies and pushes the Integration Branch, completes merged Source Issues, records durable phase outputs, and cleans up per-issue worktrees and branches.

This supersedes the Change Proposal and human-merge execution boundary described in ADR-0025 and ADR-0029, and the single-issue execution shape described in ADR-0027 and ADR-0030. The source-authoritative issue model remains: readiness stays in the Issue Source, lifecycle write-back stays coarse, blocked issue work remains blocked until a human re-enables it, and the Dashboard should center Plan Runs with nested Issue Assignments.

The initial phase prompt references are pinned from Sandcastle under `docs/reference/sandcastle-prompts/`. Product-owned Plan Run prompt templates live under `crates/orchestrator/prompts/plan-run/` so the Project can reuse Sandcastle's flow vocabulary while adapting prompt behavior that differs here, especially review approval/rejection instead of upstream reviewer edits and Project Instructions in every phase.
