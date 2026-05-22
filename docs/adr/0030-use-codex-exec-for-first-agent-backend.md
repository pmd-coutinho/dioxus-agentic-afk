# Use Codex exec for first agent backend

The first agent-execution slice will launch Codex as its single coding-agent backend through non-interactive `codex exec`. That gives the Control Plane one concrete process contract to supervise while it defines Plan Run phase prompts, process ownership, structured phase outputs, review loops, merge behavior, and Project instruction loading before adding multiple backend integrations.

The Control Plane runs Codex with full non-interactive autonomy inside Control Plane-owned Plan Run and Assignment workspaces, never directly in the registered Project path. Planning, implementation, review, and merge return required structured phase outputs rather than relying on prose parsing.

Phase execution uses a Control Plane-owned Codex profile or config overlay for backend contract settings while still loading the Project's Codex instructions and skills. Control Plane prompts add Plan Run lifecycle constraints and briefs; they do not replace Project execution policy.

The Control Plane owns prompt templates for the Plan Run planning, implementation, review, and merge phases. Phase outputs report verification evidence from Project-appropriate checks plus explicit verification gaps; review may reject work back to a bounded implementation loop, while merge verifies the integrated result before the Integration Branch is pushed.

Structured blocked outputs report the reason and the human, input, or environment change needed. Structured failed outputs report the reason and what Codex attempted before giving up. Blocked issue work becomes source-visible through coarse Lifecycle Status and remains blocked until a human re-enables it; finished Plan Runs keep durable phase outputs while cleaning per-issue worktrees and branches.

The Control Plane persists structured phase outputs on Plan Runs and Assignment Attempts with process metadata needed for supervision and diagnosis. Full Codex JSONL event stream persistence is deferred as a separate observability concern rather than mixing agent output into Activity.

Implementation and review passes use the Codex backend as distinct Assignment Attempts under the same Issue Assignment. Planning and merge remain Plan Run phases outside a single issue assignment, and each live phase attempt keeps its own process identity and structured output history.

**Considered Options**

- Support multiple coding-agent backends in the first execution slice.
- Leave the first backend unspecified while designing execution lifecycle.
