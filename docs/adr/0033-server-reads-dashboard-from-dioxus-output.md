# Server reads dashboard from Dioxus output directory

Refines ADR-0007. The Axum server's default `dashboard_asset_dir` points directly at the Dioxus CLI release output (`target/dx/agentic-afk-dashboard/release/web/public`) instead of a staged `apps/dashboard/dist/` copy. Builds run `dx build --release` once and the server reads in place, eliminating a hand-rolled `rm`/`cp` step that previously duplicated every asset on each build.

**Considered Options**

- Stage assets into `apps/dashboard/dist/` with an explicit copy step (rejected — duplication, drift risk, slower builds).
- Read from the Dioxus output directory directly (accepted — couples server to the dioxus-cli output layout, mitigated by pinning `cargo:dioxus-cli` in `mise.toml` and keeping `AGENTIC_AFK_DASHBOARD_ASSET_DIR` as an override).

**Consequences**

- Upgrading `dioxus-cli` requires verifying the output path is unchanged.
- `cargo clean` removes shipped assets; rebuild before serving.
