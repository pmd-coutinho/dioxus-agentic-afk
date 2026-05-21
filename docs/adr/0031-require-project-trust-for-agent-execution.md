# Require Project trust for agent execution

Project registration and Issue Source planning do not grant execution trust. The first agent-execution slice requires the user to explicitly trust a Project before `Start Assignment` is available because Codex assignments run non-interactively with full autonomy inside Control Plane-owned Assignment Worktrees and there is no usable unattended fallback through interactive approval prompts.

**Considered Options**

- Treat every registered Project as trusted for agent execution.
- Run untrusted Projects through interactive Codex approval prompts from the AFK flow.
