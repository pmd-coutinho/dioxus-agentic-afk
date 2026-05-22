---
status: accepted
---

# Lifecycle write-back is fatal at claim, best-effort after

When the **Control Plane** writes a **Lifecycle Status** back to the **Issue Source**, the Claimed transition is treated as a fatal failure if it fails, while every later transition (Running, Blocked, Completed) is treated as best-effort and surfaced as an **Activity** entry rather than aborting the **Plan Run**.

The Claimed transition is the only one that prevents another **Plan Run** from re-selecting the same **Source Issue**. If the **Issue Source** never receives the Claimed signal, a later refresh leaves the **Source Issue** apparently Ready and a subsequent **Plan Run** can race the in-flight **Issue Assignment**. The cost of proceeding without a confirmed Claimed write is therefore correctness loss, not just a stale upstream label, so claim must fail loudly when **Issue Source** write-back is unavailable (for example, `gh` down or unauthenticated, or a local markdown file unwritable).

Every later transition writes a label whose only consumer is human observation of the **Issue Source**. Failing the **Plan Run** because an upstream label could not be updated would discard real progress (a reviewed, merged, pushed **Integration Branch**) for a cosmetic signal. Best-effort write-back with the failure recorded as **Activity** keeps the developer informed without amplifying upstream outages into local rollbacks.

Recording lifecycle write-back failures as **Activity** rather than `eprintln!` to stderr keeps the developer notification path uniform with the rest of the **Control Plane** and visible in the **Dashboard** instead of in shell history.

**Considered Options**

- Treat every lifecycle write-back as fatal. Rejected because a post-merge upstream outage would mask successful merged work as a failed **Plan Run**.
- Treat every lifecycle write-back as best-effort. Rejected because a missed Claimed write lets concurrent **Plan Runs** race the same **Source Issue**.
- Keep lifecycle failures on stderr only. Rejected because failures invisible to the **Dashboard** lose the **Activity** locality the rest of the **Control Plane** already provides.
