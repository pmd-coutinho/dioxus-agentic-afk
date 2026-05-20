# Include Playwright foundation for E2E testing

The boilerplate will include focused Rust tests for API and persistence behavior, plus a Playwright foundation for browser E2E testing. Playwright should start or reuse a local app server through its `webServer` configuration and use a shared `baseURL`, with only smoke-level dashboard coverage until richer UI workflows exist.

**Considered Options**

- Defer browser E2E setup entirely.
- Add Playwright configuration and a minimal smoke test from the start.
