---
status: accepted
---

# Integration test tier is HTTP + Fakes; no real-subprocess slow tier

Integration tests for the full **Plan Run** flow drive the HTTP router with in-memory SQLite and inject `Fake*` implementations of every external seam: `FakePlanningPhaseRunner`, `FakeImplementationPhaseRunner`, `FakeReviewPhaseRunner`, `FakeMergePhaseRunner`, `FakeIntegrationBranchPusher`, `FakeAssignmentWorktreeCleaner`, `FakeWorktreeProvisioner`, `FakeLifecycleWriter`, and the static integration-branch refresher. Tests live in `apps/control-plane-server/tests/plan_run_*.rs`, one file per scenario cluster, paired with one Playwright spec per ticket under `tests/e2e/`. There is no parallel test tier that runs real `codex`, real `gh`, or real `git` against temp repositories.

New scenarios — including the ADR 0037 push-failure recovery cluster and the re-enable Lifecycle write-back cluster — are added ticket-driven at this tier as their features land. Refactor-seam contract coverage is grown opportunistically: when a new test reveals that a seam was only exercised by its happy path, the missing case is added in the same change, not deferred into a backfill effort.

**Considered Options**

- Add a slow tier that runs real binaries (`codex`, `gh`, `git`) against temp Git repositories, gated behind a cargo feature or a separate CI lane. Rejected because the marginal fidelity over the current tier is concentrated in two narrow risks — `gh` API drift on Lifecycle write-back and `git push` fast-forward semantics — and a parallel suite buys those at the cost of ~10× test time, flakiness from network and binary-version variance, and a second CI lane to maintain. The two risks can be covered by targeted manual smoke or by adding one or two narrow real-binary tests later if and when a divergence between Fake and production behavior burns us.
- Dedicate a coverage-driven backfill pass auditing every refactor seam (`crates/planning-snapshot`, the folded event publisher, the lifecycle writer) before taking on new features. Rejected because `plan_run_*.rs` already drives the state machine through the new seams transitively, the planning-snapshot crate carries its own in-module tests, and a standalone backfill risks pinning implementation details rather than behaviors. Opportunistic per-PR seam tests catch the same gaps without postponing shippable work.
- Property-based or fuzzed state-machine driver. Rejected at this stage because the **Plan Run** state graph is small and the existing per-scenario tests are readable; a property driver pays off on larger state spaces.
- Stop writing Playwright e2e for new operator actions on the grounds that server-side coverage is sufficient. Rejected because operator-action buttons (Retry Push, Abandon Staged, Re-enable) are the user-visible point of the feature, and state-only coverage misses wire-up regressions (mutation key mismatch, missing error toast, wrong status pill).

**Revisit Triggers**

The slow tier decision is reopened if any of these occur: a `gh` schema change silently breaks Lifecycle write-back in production while Fake-driven tests pass; a `git push` non-fast-forward classification regresses because the Fake encoded it differently than the real `git`; a third risk emerges where Fake and production diverge in a way that costs operator time to diagnose. At that point one or two targeted real-binary tests are added — still not a full parallel suite.
