# Serve compiled dashboard assets from the server

In normal use, the Axum server will serve the compiled dashboard assets as well as the API so the local control plane has a single local URL. Development may run the Dioxus dashboard and Axum API as separate processes to support hot reload and faster iteration.

**Considered Options**

- Always run frontend and backend as separate servers.
- Serve compiled dashboard assets from the server for normal use, with separate dev servers only during development.
