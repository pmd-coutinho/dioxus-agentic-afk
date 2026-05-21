# Use Codex exec for first agent backend

The first agent-execution slice will launch Codex as its single coding-agent backend through non-interactive `codex exec`. That gives the Control Plane one concrete process contract to supervise while it defines Issue Assignment prompts, process ownership, structured terminal outcomes, recovery, repair, and Project instruction loading before adding multiple backend integrations.

The Control Plane runs Codex with full non-interactive autonomy inside a Control Plane-owned Assignment Worktree only, never directly in the registered Project path. Codex returns its terminal `ReadyForProposal`, `Blocked`, or `Failed` outcome through a required structured output schema rather than prose parsing.

Assignment execution uses a Control Plane-owned Codex profile or config overlay for backend contract settings while still loading the Project's Codex instructions and skills. Control Plane prompts add Issue Assignment lifecycle constraints and briefs; they do not replace Project execution policy.

The Control Plane owns prompt templates for initial assignments, recovery, and repair. A `ReadyForProposal` Codex outcome reports verification evidence from the Project-appropriate checks Codex could reasonably discover and run, plus explicit verification gaps when local checks could not be completed; remote required checks still decide whether a Change Proposal becomes verified.

Structured `Blocked` outcomes report the reason and the human, input, or environment change needed. Structured `Failed` outcomes report the reason and what Codex attempted before giving up. Both outcomes preserve the Assignment Worktree and become source-visible blocked assignments for recovery or abandonment rather than adding a separate Source Issue failure lifecycle.

The Control Plane persists the structured Codex terminal outcome on the Issue Assignment with process metadata needed for lifecycle recovery. Full Codex JSONL event stream persistence is deferred as a separate observability concern rather than mixing agent output into Activity.

Initial assignment work, recovery, and repair all use the Codex backend as distinct `initial`, `recovery`, and `repair` Assignment Attempts under the same Issue Assignment. Each attempt keeps its own process identity and structured outcome history while the assignment still permits at most one live Codex process at a time.

**Considered Options**

- Support multiple coding-agent backends in the first execution slice.
- Leave the first backend unspecified while designing execution lifecycle.
