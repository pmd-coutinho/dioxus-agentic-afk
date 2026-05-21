# Live dashboard state via SSE deltas over a central reactive store

The Dashboard hydrates initial Project state with a single REST snapshot, then subscribes to a Server-Sent Events stream that pushes typed state deltas into one project-scoped reactive store, because the prior pattern of per-mutation full-page reloads and per-panel REST refetches makes long Control Plane operations (such as Start Assignment, which creates an Assignment Worktree and boots an agent) appear frozen, encourages duplicate user clicks, and gives no live view of subsequent Assignment Attempt or Activity transitions. The snapshot response carries a monotonic `sequence` that the client passes back through SSE `Last-Event-ID` so the server replays only missed deltas from a bounded per-Project ring buffer, falling back to a `Resync` event that triggers a fresh REST snapshot when the buffer has rolled over. Deltas reuse the existing `Activity` envelope as the wire format wherever the variant already exists, so the audit log and the live wire format remain a single source of truth.

**Considered Options**

- Keep the existing pattern of `window.location.reload()` after each mutation and per-panel `use_resource` polling.
- Use SSE invalidation events that only carry an entity key and have the client refetch through REST.
- Use WebSockets instead of SSE for bidirectional updates.
- Hydrate the store entirely through SSE by emitting the full snapshot as the first event and dropping the REST snapshot endpoint.
- Keep the existing per-panel `use_resource` hooks and have SSE handlers call `.restart()` instead of introducing a central reactive store.
