---
status: accepted
---

# Support OpenCode as an alternative Agent Backend

The **Control Plane** supports multiple **Agent Backends**. A **Project** has a default **Agent Backend** (Codex or OpenCode) in its execution config; a **Plan Run** may override this at trigger time. Both backends run inside the same generic **Agent Sandbox** infrastructure — the container image carries both CLIs, and the orchestrator resolves the correct auth path and command builder per backend.

The glossary term **Codex Sandbox** is replaced with **Agent Sandbox** to reflect that the isolation boundary is backend-agnostic. **Agent Backend** is the new canonical term for the concrete CLI (Codex, OpenCode) that runs inside the sandbox.

## Backend configuration shape

`ProjectExecutionConfig` gains two columns: `backend_kind` (`TEXT`, `codex` or `opencode`) and `backend_config_json` (`TEXT`). The JSON is typed by the backend variant in Rust:

- **Codex**: `{ planning: PhaseModel, implementation: PhaseModel, review: PhaseModel, merge: PhaseModel }` where `PhaseModel` is `{ model: String, modifier: String? }` and `modifier` maps to `reasoning_effort`.
- **OpenCode**: same `PhaseModel` shape, but `modifier` maps to `--variant` (e.g. `high` for DeepSeek reasoning modes). For models with no variant (e.g. kimi-k2.6) the field is `null` and the runner omits `--variant`.

This keeps the schema minimal (`backend_kind` + `backend_config_json`) while preserving type safety in the contract layer. A normalized schema with per-phase columns was rejected because it would explode every time a new backend or phase appears.

## Sandbox preflight is backend-aware and reconstructed per trigger

`SandboxPreflight` is constructed at trigger time with the resolved auth path, not at server boot. The handler reads the **Project** config, picks the auth path (`~/.codex/auth.json` for Codex, `~/.local/share/opencode/auth.json` for OpenCode), and builds a `SandboxPreflight` instance for that check. This preserves the existing trigger-time preflight contract: fail fast with a 422 and stable URN before any Plan Run row is written.

The preflight failure variants gain `AuthMissing(backend: String, path: PathBuf)` so the dashboard can render backend-specific guidance.

## Single runtime image with both CLIs

The Docker runtime image installs both `codex` and `opencode` CLI binaries. A per-backend image was rejected because the image is already generic (mise, git, node, playwright); adding a second CLI is a small delta and avoids N image variants. The existing content-addressed tag covers the Dockerfile and entrypoint; adding `opencode` to the Dockerfile changes the hash and triggers a rebuild.

## Generic `DockerAgentRunner` with pluggable `BackendCommandBuilder`

`DockerCodexRunner` is renamed to `DockerAgentRunner`. It holds backend-agnostic concerns (labels, mounts, resource caps, container naming) and delegates the command vector, model mapping, and auth mounts to a `BackendCommandBuilder` trait. Concrete implementations are `CodexCommandBuilder` and `OpenCodeCommandBuilder`.

This avoids duplicating sandbox mechanics across backends and keeps the phase-runner trait implementations (`PlanningPhaseRunner`, `ImplementationPhaseRunner`, etc.) in one place.

## Plan Run records full backend snapshot

The `plan_runs` table gains `backend_kind` and `backend_config_json` columns. When a Plan Run is created, the coordinator copies the resolved backend config onto the row. This captures the exact model mapping used for that run even if the **Project** config changes later. The dashboard can display "ran with OpenCode (kimi-k2.6 planning / deepseek-v4-flash high implementation)" for historical Plan Runs.

## Trigger-time override

`POST /api/projects/{id}/plan-runs` gains an optional body: `{ "backend_kind": "opencode" }`. When present, it overrides the **Project** default for this run only. The body does not accept per-phase model overrides in the first slice — the models come from the **Project**'s saved OpenCode config (or sensible defaults if none saved).

## Default auto-seed stays Codex

When a **Project** has no execution config and the first Plan Run triggers, the seeded defaults are still Codex-shaped (`integration_branch`, `max_parallel_tasks: 3`, `review_retry_limit: 3`, `backend_kind: "codex"`). This preserves backward compatibility. A developer who wants OpenCode by default sets it in the dashboard before the first run.

## Coordination changes

`resolve_deps_for_project` gains an `exec_config` parameter so it can build the correct backend-specific phase runners. All callers fetch the config from the DB and pass it; callers that only need pusher/cleaner/lifecycle ignore the resolved phase runners. `PlanRunDeps.production_sandbox` loses its hardcoded `codex_auth_path` and `codex_config_path`; the generic `AgentSandboxConfig` carries a single `auth_path`.

## Considered Options

- **Per-backend sandboxes (Codex Sandbox + OpenCode Sandbox).** Rejected because both backends share identical isolation semantics: same Docker, same labels, same resource caps, same worktree mounts. Duplicating the sandbox concept would split cleanup, sweeping, and preflight logic without gaining anything.
- **Backend-agnostic auth resolution (SandboxPreflight never knows which backend).** Rejected because preflight must fail fast at trigger time before any runner is selected. Moving auth checking into the runner would create a Plan Run row that fails mysteriously during Planning Phase instead of surfacing a clear 422 at trigger time.
- **Per-backend Docker images.** Rejected because the runtime image is already generic; installing a second CLI is a small Dockerfile delta. N image variants would complicate the build cache, sweeper logic, and content-addressed tagging for no operational benefit.
- **Normalized per-phase columns in the database.** Rejected because Codex and OpenCode config shapes are too different (reasoning_effort vs variant), and adding a column per phase per backend would explode the schema. JSON with backend-typed deserialization keeps the schema minimal.
- **Transient trigger override (no Plan Run snapshot).** Rejected because the Plan Run row is the durable record of what happened. Without a snapshot, historical Plan Runs would show an ambiguous backend if the Project config changed later.
- **Detect available auth and default to whichever backend is configured.** Rejected in favor of explicit Codex default for backward compatibility. Silent auto-switching would surprise a developer who installed both CLIs but only intended one.
