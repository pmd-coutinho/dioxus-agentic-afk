# Use UUIDs for Project identity

Projects will use UUIDs as their stable API identity, stored in SQLite as text. This avoids exposing database row IDs in contracts and avoids brittle path-derived identifiers when local codebase roots move.

**Considered Options**

- Database integer IDs.
- Path-derived identifiers.
- UUIDs.
