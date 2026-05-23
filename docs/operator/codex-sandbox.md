# Codex Sandbox — Operator Guide

Every Codex execution (**Planning Phase**, **Implementation Phase**,
**Review Phase**, **Merge Phase**) runs inside an ephemeral Docker
container — the **Codex Sandbox**. See ADR-0041.

## Host prerequisites

- **Docker daemon** reachable from the orchestrator process. Either run
  the daemon as root and put your user in the `docker` group, or run
  the daemon rootful and have the orchestrator authenticate via the
  socket. Rootless Docker also works as long as your user can list
  containers and write to bind mounts owned by the host UID.
- **`~/.codex/auth.json`** present. Run `codex login` once on the host
  to create it. The file is bind-mounted read-write into every Codex
  Sandbox so OAuth refresh-token rotation lands on the host.
- **`~/.codex/config.toml`** present (can be empty). The orchestrator
  bind-mounts it read-only at `$HOME/.codex/config.toml` inside the
  container.
- **`mise.toml`** at every **Project** worktree root. The runtime
  image carries `mise` only; the entrypoint runs `mise install`
  against the bind-mounted worktree so per-Project toolchains come
  from the worktree, not from the image.

Environment overrides (all optional):

- `AGENTIC_AFK_DOCKER_BIN` — docker CLI path. Default `docker`.
- `AGENTIC_AFK_CODEX_AUTH_PATH` — alternate auth.json path. Default
  `$HOME/.codex/auth.json`.

## Image lifecycle

The runtime image is built **locally** on first Plan Run.

- Image repo: `agentic-afk-runtime`.
- Tag: 12 hex chars of `sha256(Dockerfile || entrypoint.sh)`, so any
  change to the embedded Dockerfile or entrypoint produces a fresh
  tag and a fresh build. Multiple SHAs can coexist on the host until
  you prune them.
- Manual cleanup: `docker image rm agentic-afk-runtime:<tag>`. Use
  `docker images --filter reference=agentic-afk-runtime` to list.

The first build is the slow one (≈1–2 minutes for a clean Docker).
Subsequent Plan Runs reuse the cached image — the orchestrator
short-circuits via `docker image inspect <tag>` before any rebuild.

## `agentic-afk-mise-cache` volume

Project toolchains live in a **named Docker volume** shared across
**Projects** and Codex Sandboxes. The first `mise install` for a given
toolchain version is slow; subsequent installs hit the cache and
finish in seconds.

- Volume name: `agentic-afk-mise-cache`.
- Manual reset: `docker volume rm agentic-afk-mise-cache`. The next
  Plan Run rebuilds the cache from scratch.

There is no auto-prune for the image or the volume in the first slice.
The cache is content-addressed by mise version, so growth is slow.

## Trigger-time preflight (URN troubleshooting)

A Plan Run trigger fails with HTTP 422 and an RFC-7807 problem-JSON
body before any **Source Issue** lifecycle write happens if any of the
four sandbox preflight checks fails. See [issue #73] for the source.

### `urn:agentic-afk:sandbox-docker-unavailable`

The orchestrator could not reach the Docker daemon.

- Verify `docker version` works as the orchestrator user.
- If using rootful Docker, confirm group membership: `getent group
  docker | grep $USER`. Re-login if you just added yourself.
- If using a non-default socket, set `AGENTIC_AFK_DOCKER_BIN` to a
  wrapper that targets the right `DOCKER_HOST`.

### `urn:agentic-afk:sandbox-codex-auth-missing`

`~/.codex/auth.json` does not exist or is not readable.

- Run `codex login` once on the host.
- Verify the file exists at the path returned in the response
  `detail`, or set `AGENTIC_AFK_CODEX_AUTH_PATH` to its real
  location.

### `urn:agentic-afk:sandbox-mise-toml-missing`

The **Project** worktree has no `mise.toml`.

- Add a `mise.toml` listing the toolchains the project needs (Rust,
  Node, Python, etc.) at the worktree root. The Codex Sandbox runs
  `mise install` against this file at container start.

### `urn:agentic-afk:sandbox-runtime-image-build-failed`

`docker build` could not produce the runtime image. The `detail`
field carries the truncated `docker build` stderr.

- Read the stderr for the immediate cause (network outage, disk
  pressure, base-image pull failure, etc.).
- Common case: transient registry outage. Retry the Plan Run trigger.
- Disk pressure: `docker system df`, then `docker image prune` and
  `docker volume rm agentic-afk-mise-cache` if needed.

## Boot container sweep

On every orchestrator boot, after the DB-side recovery scanner
(ADR-0042), the orchestrator sweeps any Codex Sandbox containers
carrying our labels. See [issue #75] for the source.

- Containers whose owning Issue Assignment is still non-terminal are
  killed and removed; the assignment transitions to `blocked` with
  `Block Reason::OrchestratorRestart`. The Dashboard surfaces this
  with the standard recovery affordance — re-enable the Source
  Issue to retry in a new Plan Run.
- Containers whose owner is already terminal are removed without
  further DB transitions.
- Docker daemon unavailable at boot is **warn-not-fatal**: the
  read-only Dashboard still serves; the next Plan Run trigger fails
  preflight with `urn:agentic-afk:sandbox-docker-unavailable` until
  Docker is back.

[issue #73]: https://github.com/pmd-coutinho/dioxus-agentic-afk/issues/73
[issue #75]: https://github.com/pmd-coutinho/dioxus-agentic-afk/issues/75
