# Use a shared contracts crate

The workspace will include a small shared contracts crate for API request and response types used by both the Axum server and Dioxus dashboard. This crate should contain wire-level DTOs and OpenAPI schema derives, not orchestration business logic.

**Considered Options**

- Define API structs separately in the server and dashboard until duplication becomes painful.
- Share API contracts through a dedicated Rust crate from day one.
