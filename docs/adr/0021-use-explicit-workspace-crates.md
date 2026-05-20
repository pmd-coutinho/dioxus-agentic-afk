# Use explicit workspace crates

The workspace will separate the server, dashboard, API contracts, persistence, Git Summary, and orchestration boundaries into explicit crates. The `orchestrator` crate may start mostly empty so the intended product center is visible, but it should not contain placeholder behavior before agent execution semantics are defined.

**Considered Options**

- Keep all backend code inside the server crate.
- Add only crates with immediate implementation needs.
- Create explicit crates for the known long-lived boundaries, including an initially sparse orchestrator crate.
