# Use an API prefix for JSON routes

JSON endpoints will live under `/api/*`, while non-API paths are reserved for the dashboard and its browser routes. The server may still expose simple operational endpoints such as `/health`, but project and app data routes should use the API prefix to avoid ambiguity with compiled dashboard assets and SPA fallback routing.

**Considered Options**

- Put JSON endpoints at top-level paths such as `/projects`.
- Put JSON endpoints under `/api/*`.
