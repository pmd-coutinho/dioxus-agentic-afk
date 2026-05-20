# Add minimal CI from day one

The boilerplate will include minimal GitHub Actions validation from the start. CI should run the same `mise` tasks used locally for formatting, Rust checks/tests, dashboard build, and Playwright smoke coverage when browser dependencies are available, without adding release or publish automation yet.

**Considered Options**

- Defer CI until the app has more features.
- Add minimal CI for the runnable foundation from day one.
