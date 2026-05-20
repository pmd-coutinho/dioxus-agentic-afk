# Build Issue Source planning before agent execution

The next product slice will make Issue Source configuration, Source Issue sync, readiness parsing, dependency parsing, parent grouping, Source Order, and Dashboard queue visibility real before spawning agents. GitHub Issues and local markdown Issue Sources will both be supported in this planning slice so the Issue Source abstraction is exercised immediately instead of being a GitHub-only shape.

GitHub and local markdown Source Issues should follow the same issue-body template, but the Control Plane should parse only the scheduling metadata it needs: source id, title, readiness, parent grouping, and explicit dependencies. The raw Source Issue text remains the agent brief because the assigned agent is responsible for interpreting the implementation details.

Local markdown Issue Sources will read from a configured directory under the Project, defaulting to `.scratch/issues/` when that convention is present but allowing Projects to point at another local issue directory.

The Control Plane may suggest Issue Source candidates from Project evidence such as a GitHub remote or a local issue directory, but a candidate is not authoritative until the user explicitly enables it for the Project. A Project will have at most one persisted enabled Issue Source in the first version to avoid identity, ordering, and dependency conflicts across sources.

Synced Source Issues will be persisted as a snapshot containing normalized scheduling metadata and raw source text. This snapshot supports stable dashboard views, offline visibility, and sync error reporting, but it is a cache of the Issue Source rather than editable Control Plane truth.

GitHub Issue Sources will use the local `gh` CLI authentication state in the first version. The Control Plane will not store GitHub tokens; if `gh` is missing or unauthenticated, manual sync fails with a visible Issue Source auth error while preserving the last successful snapshot.

The planning slice will use manual sync, with no background polling. The Dashboard may offer an explicit refresh action and show last sync status.

The planning slice is read-only with respect to Issue Sources. Lifecycle Status write-back, Change Proposal links, and Repair Loop reporting belong to the later execution slice.

This proves the source-authoritative planning model and scheduling view before adding worktrees, agent processes, Change Proposal creation, CI polling, or Repair Loops.

**Considered Options**

- Start by spawning agents from ready issues immediately.
- Build only a GitHub-specific queue and add local markdown later.
- Treat the issue-body template as a strict implementation schema owned by the Control Plane.
- Store GitHub tokens in the Control Plane database for the first GitHub Issue Source implementation.
- Defer dashboard visibility until after execution works.
- Write Lifecycle Status back to the Issue Source during the planning slice.
