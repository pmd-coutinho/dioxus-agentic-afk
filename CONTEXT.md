# Dioxus Agentic AFK

Dioxus Agentic AFK is a Rust-native agent orchestration system with a dashboard-first control plane. It exists to give a developer a foundation for starting, observing, and managing agent work while away from the keyboard.

## Language

**Orchestrator**:
The system that coordinates agent work across isolated environments.
_Avoid_: Wrapper, script runner

**Control Plane**:
The user-facing surface for starting, observing, and managing agent work.
_Avoid_: Add-on dashboard, admin panel

**Dashboard**:
The primary visual interface for the **Control Plane**.
_Avoid_: UI, frontend

**Boilerplate**:
The minimal runnable foundation for the **Orchestrator** and **Dashboard** before agent-work features exist.
_Avoid_: MVP, prototype, full app

**Project**:
A local codebase root known to the **Control Plane**.
_Avoid_: Repository, workspace, task

**Project ID**:
A stable identifier for a **Project** that does not depend on its filesystem path.
_Avoid_: Database row ID, path slug

**Git Summary**:
Read-only Git metadata derived from a **Project** path for dashboard display.
_Avoid_: Project state, repository identity

**Activity**:
A chronological record of noteworthy control-plane events for a **Project**.
_Avoid_: Fake metrics, agent output

**Local Control Plane**:
A **Control Plane** intended to run on the developer's own machine for that developer's Projects.
_Avoid_: Hosted service, team workspace

**agentic-afk**:
The command-line entrypoint for operating the local **Control Plane**.
_Avoid_: afk, dioxus-agentic-afk

## Relationships

- The **Control Plane** belongs to the **Orchestrator**
- The **Dashboard** is the primary interface for the **Control Plane**
- The **Boilerplate** establishes the **Orchestrator** and **Dashboard** without requiring agent-work features
- The **Control Plane** manages zero or more **Projects**
- A **Project** has exactly one **Project ID**
- A **Project** may have one derived **Git Summary**
- A **Project** may have zero or more **Activity** entries
- A **Local Control Plane** is operated by one developer on their own machine
- **agentic-afk** operates the **Local Control Plane**

## Example dialogue

> **Dev:** "Are we wrapping another orchestrator and adding a dashboard?"
> **Domain expert:** "No - this is a Rust-native **Orchestrator**, and the **Dashboard** is the primary way to operate its **Control Plane**."
> **Dev:** "Does the **Boilerplate** need to run agents?"
> **Domain expert:** "No - the **Boilerplate** only needs the runnable foundation where agent-work features will be added later."
> **Dev:** "Is a **Project** the same thing as a Git repository?"
> **Domain expert:** "No - a **Project** is a local codebase root; it may have Git metadata, but Git does not define the concept."
> **Dev:** "Can the dashboard show sample agent activity?"
> **Domain expert:** "No - the dashboard can reserve space for **Activity**, but it should only show truthful data."
> **Dev:** "Is this a hosted dashboard for a team?"
> **Domain expert:** "No - this is a **Local Control Plane** for one developer's machine."
> **Dev:** "Should the binary be called `afk`?"
> **Domain expert:** "No - use **agentic-afk** so the command is specific enough for daily local use."
> **Dev:** "Can a **Project** be identified by its path?"
> **Domain expert:** "No - paths can move, so a **Project** has a stable **Project ID**."
> **Dev:** "Is Git required for a **Project**?"
> **Domain expert:** "No - Git can provide a derived **Git Summary**, but it does not define whether a local codebase root is a **Project**."

## Flagged ambiguities

- "similar to Sandcastle" was resolved to mean a new Rust-native **Orchestrator** with a dashboard-first **Control Plane**, not a wrapper around Sandcastle.
- "boilerplate" was resolved to mean the runnable project foundation, not the first agent-execution feature slice.
- "project" was resolved to mean a local codebase root, not a Git repository, workspace, or agent task.
- "activity" was reserved for real control-plane events, not fabricated dashboard content.
- "local only" was resolved as a **Local Control Plane** constraint, not merely an unauthenticated deployment choice.
- "afk" was rejected as too generic for the CLI; **agentic-afk** is the canonical entrypoint name.
- "project identity" was resolved as a stable **Project ID**, not a database row ID or filesystem path.
- "git metadata" was resolved as a derived **Git Summary**, not persisted **Project** state.
