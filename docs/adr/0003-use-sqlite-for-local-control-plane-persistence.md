# Use SQLite for local control-plane persistence

The boilerplate will include SQLite-backed persistence from the start so migrations, configuration, and dashboard state have a real home before agent features are added. SQLite is sufficient for the initial local-first control plane, but the schema should describe control-plane concepts rather than assume the final orchestration storage model.

**Considered Options**

- Stay stateless until the first agent feature needs storage.
- Add SQLite now as local control-plane persistence.
