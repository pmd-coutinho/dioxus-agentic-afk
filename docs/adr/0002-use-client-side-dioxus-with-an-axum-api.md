# Use client-side Dioxus with an Axum API

The dashboard will be a client-side Dioxus web app that talks to an Axum JSON API, rather than a fullstack Dioxus application. This keeps the control-plane API as a first-class boundary for future CLI, automation, and agent integrations while allowing the dashboard to remain a focused browser client.

**Considered Options**

- A fullstack Dioxus application handling both dashboard and server concerns.
- A client-side Dioxus dashboard backed by an Axum API.
