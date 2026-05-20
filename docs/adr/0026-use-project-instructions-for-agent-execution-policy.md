# Use Project instructions for agent execution policy

Issue Assignments will be executed using the Project's own agent instructions first, including a TDD skill when one is available. When a Project does not provide explicit instructions, the Control Plane may supply conservative defaults such as behavior-first tests, vertical implementation slices, Change Proposal creation with source issue links, and bounded Repair Loops for failed checks.

**Considered Options**

- Require every Project to provide a TDD skill before issue work can run.
- Ignore Project-specific instructions and use one global Control Plane prompt.
- Treat TDD as a domain glossary concept rather than execution policy.
