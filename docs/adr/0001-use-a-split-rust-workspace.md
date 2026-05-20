# Use a split Rust workspace

The boilerplate will use a single Cargo workspace with separate application crates for the Axum API server and the Dioxus dashboard, with shared crates added only when shared domain types or services justify them. This costs a little more setup than a single crate, but it keeps the dashboard, server, and future orchestration logic from growing into one boundaryless application.

**Considered Options**

- A single Rust binary containing both server and dashboard code.
- A split Rust workspace with separate frontend and backend crates.
