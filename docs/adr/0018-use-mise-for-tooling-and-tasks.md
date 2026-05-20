# Use mise for tooling and tasks

The repo will use `mise` to manage development tooling and common tasks. This keeps Rust, Node-based browser testing, Dioxus tooling, formatting, database commands, and app lifecycle commands discoverable through one task interface.

**Considered Options**

- Document raw commands only.
- Use a dedicated task runner such as `just`.
- Use `mise` for both tool versions and tasks.
