---
status: accepted
---

# Auto-Replan as a per-Project tri-state with pause-on-attention semantics

**Auto-Replan** is the per-**Project** cadence that closes the gap between the product name (`agentic-afk`) and its behaviour. Without it, every **Plan Run** still requires a manual trigger, so "AFK" means "one batch while you make coffee" rather than "an evening of unattended work." **Auto-Replan** is modelled as a per-**Project** tri-state stored on the `projects` row: `Off` (default), `Armed` (human-set, driver runs cycles), and `Paused` (system-set, carries a typed **Pause Reason**, requires explicit human Resume to return to `Armed`). A single tick driver in the control-plane server scans all `Armed` **Projects** on a 60-second cooldown and, for any **Project** without an active **Plan Run**, runs one **Auto-Replan Cycle**: `sync_issue_source` → **Planning Phase** → **Plan Run** to terminal. The cycle continues from `Armed` only when the **Plan Run** finishes with at least one merged **Issue Assignment** and no `blocked` or `merge_staged` **Issue Assignments**. Every other terminal — empty-successful **Plan Run**, any block, any `merge_staged` left, failed **Planning Phase**, failed sync — transitions `Armed` → `Paused` with a typed **Pause Reason** (`EmptyBacklog` / `AssignmentBlocked` / `PushNonFastForward` / `MergeStagedLeft` / `PlanningFailed` / `SyncFailed`) and records an **Activity** entry. Pause survives server restart. Resume is a single explicit action that flips `Paused` → `Armed`; backlog change and block resolution do not auto-resume.

## Considered Options

- **Boolean Auto-Replan on/off.** Rejected because the driver needs three semantically distinct states: the developer setting (`Off`/`Armed`) and the system-observed need-attention state (`Paused`). Folding `Paused` into `Off` loses the resume affordance and the pause reason; folding `Paused` into `Armed` keeps the loop spinning on conditions that need human attention.

- **Continue on empty backlog with exponential backoff.** Initially considered to honour "AFK = run all night even if there is nothing to do." Rejected after noticing that **Planning Phase** burns Codex tokens on every cycle, and a drained backlog with unchanged **Issue Source** snapshot would re-run the planner for guaranteed-empty results. A snapshot-hash short-circuit was considered to skip planning on unchanged input, but added a config surface (cooldown, max backoff, hash format) for a state that is already a human-attention signal. Pause-on-empty is louder, simpler, and matches the resume model used for every other pause condition.

- **Auto-resume on backlog change or block resolution.** Rejected because it couples two state machines (`Auto-Replan State` and `Lifecycle Status`) and surprises a developer who walked away expecting the loop stopped. Re-enabling a blocked **Source Issue** is already its own human action; expecting it to also re-arm the driver is implicit and hard to undo. Explicit Resume on a banner is one click and carries no surprise.

- **Independent timer for Issue Source refresh, separate from the Auto-Replan driver.** Rejected to avoid two-loop interaction (mid-cycle source mutation, cache invalidation during planning, divergent cadences). Bundling sync into the **Auto-Replan Cycle** keeps the planner reading the freshest snapshot it can without a second timer; the cost is at most one **Plan Run** of staleness, which is bounded by the existing single-active **Plan Run** invariant.

- **One tokio task per Armed Project.** Rejected because the per-Project task count would grow with the dashboard and bring N task lifetimes to manage (start on arm, cancel on disarm, restart on server boot). A single tick driver scanning all `Armed` **Projects** is simpler, observable, and matches how the **Local Control Plane** already coordinates work.

- **Per-Project cooldown configuration.** Deferred. A single 60-second default fits the **Local Control Plane** cadence (human-driven, not bursty) and avoids inventing a config knob before any **Project** asks for one.

- **History table for Auto-Replan transitions.** Rejected because the existing **Activity** stream already records noteworthy control-plane events, including arm/pause/resume/disarm. Duplicating into a dedicated table would split the audit trail. State stays a per-Project scalar; transitions go to **Activity**.

## Consequences

- `projects` gains two columns: `auto_replan_state TEXT NOT NULL DEFAULT 'off'` and `auto_replan_pause_reason TEXT NULL`. One migration appends both.
- Three new endpoints under `/api/projects/{id}/auto-replan/`: `arm`, `disarm`, `resume`. `arm` and `resume` are 409 on wrong source state; `disarm` is idempotent.
- New `ProjectEvent::AutoReplanStateChanged { state, reason }` SSE variant. The dashboard surfaces a banner on each project page when `Paused`, naming the **Pause Reason** and offering a Resume button.
- New **Activity** kinds: `AutoReplanArmed`, `AutoReplanPaused { reason }`, `AutoReplanResumed`, `AutoReplanDisarmed`. Detail strings (push stderr, sync error) live on the matching **Activity** entry, not on the **Project** row.
- The manual "Start Plan Run" button remains available while `Armed`. A manual trigger races the driver via the existing single-active **Plan Run** guard — whichever wins, the other no-ops. No special UI is needed.
- `Paused` is the resting state after any condition needing human attention. The developer who walks away with `Armed` and comes back to `Paused` reads the **Pause Reason** in the banner, resolves the cause (re-enable, fix, push retry, ack), and clicks Resume to continue.
