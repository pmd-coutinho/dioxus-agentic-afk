---
status: accepted
---

# Use a `merge_staged` Assignment Status for push-failure recovery

The **Merge Phase** of a **Plan Run** ends in `git push` of the **Integration Branch**. The push can fail for transient reasons (network outage, auth token expiry) or durable ones (remote diverged, branch protection rejected the push) after the **Issue Assignment** has been locally integrated and verified. The **Issue Assignment** records a `merge_staged` **Assignment Status** between `merging` and `merged`. Only a successful push transitions `merge_staged` → `merged` and writes **Lifecycle Status** `Completed`. `Merged` therefore always implies a pushed **Integration Branch**.

When push fails, the **Issue Assignment** stays `merge_staged` and the **Plan Run** finishes `failed`. Recovery is two explicit operator actions on the staged **Issue Assignment**:

- **Retry Push** re-runs `git push` only. No re-verify, no rebase. A non-fast-forward result routes the **Issue Assignment** to `blocked` because the **Integration Branch** has diverged and the local integration tree is no longer a valid update; recovery belongs in a new **Plan Run** with a refreshed baseline, not in the retry button.
- **Abandon Staged** routes the **Issue Assignment** to `blocked` without attempting push, for cases where the operator has decided the staged work should not land.

`merge_staged` is dormant: it does not consume **Max Parallel Tasks**, matching the existing rule for dormant blocked **Issue Assignments**. The agent is not running; the work is awaiting a single `git push` command, not capacity.

Worktree and issue-branch cleanup gate on the **Issue Assignment** reaching a terminal status (`merged` or `blocked`), not on the **Plan Run** finishing. A `failed` **Plan Run** can therefore co-exist with a non-terminal `merge_staged` **Issue Assignment** awaiting retry. A `failed` **Plan Run** stays `failed` even if a later retry advances the staged **Issue Assignment** to `merged`; the **Plan Run** status records the original Merge Phase outcome, not the eventual fate of every **Issue Assignment**, mirroring how a finished **Plan Run** already coexists with `blocked` **Issue Assignments** outside the merge.

**Considered Options**

- Keep the existing single `merged` status and retry push idempotently inside the **Merge Phase**. Rejected because a durable push failure (auth revoked, branch protection) is not fixed by retries inside one phase invocation and the half-state already documented in `CONTEXT.md` remains.
- Stage at the **Plan Run** level (one `staged` set on the **Plan Run** holding all locally-merged **Issue Assignments**, push as a batch). Rejected because the current **Merge Phase** integrates one **Issue Assignment** at a time per ADR 0045-era work; per-Assignment granularity matches the existing state machine without inventing a new aggregate.
- Auto-retry push on next **Plan Run** start or server boot. Rejected because durable push failures would loop noisily and auto-push after a `failed` **Plan Run** removes the operator's chance to abandon staged work.
- Retry Push that also fetches, rebases, and re-verifies. Rejected because rebasing post-stage silently introduces unverified remote changes into a tree that was approved at staging time; this is a new merge, and new merges belong in new **Plan Runs**.
- Flip `Plan Run` from `failed` to `succeeded` when a later retry lands the staged **Issue Assignment**. Rejected because mutating a terminal **Plan Run** status late hides the original Merge Phase outcome from the audit log.
