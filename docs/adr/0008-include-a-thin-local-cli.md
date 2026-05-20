# Include a thin local CLI

The boilerplate will include a thin CLI for local control-plane lifecycle and setup commands such as serving the app, running migrations, and seeding development data. Agent commands are intentionally out of scope until agent execution semantics are defined.

**Considered Options**

- Start with no CLI and rely on ad hoc development commands.
- Include a thin local CLI for lifecycle and setup only.
