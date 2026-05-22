---
status: superseded by ADR-0034
---

# Start with Project-focused dashboard routes

The dashboard boilerplate will start with Project-focused routes: `/`, `/projects`, `/projects/:id`, and `/settings`. Agent execution concepts such as runs, agents, and tasks should not appear as top-level navigation until their domain behavior is defined.

The first execution slice stays on the Project detail route: it adds one active Issue Assignment panel alongside the planning queue and shows the one-active-assignment-per-Project limit when other Ready Issues wait. Assignment controls stay lifecycle-specific: recover blocked work, abandon disposable work, and open an existing Change Proposal.

**Considered Options**

- Add placeholder top-level routes for future agent concepts.
- Start with Project-focused routes and reserve future navigation until the concepts are real.
