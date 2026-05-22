# Use Project instructions for agent execution policy

Plan Run phase prompts will include the Project's own agent instructions in planning, implementation, review, and merge, including a TDD skill when one is available. When a Project does not provide explicit instructions, the Control Plane may supply conservative defaults such as behavior-first tests, vertical implementation slices, review findings that return to bounded implementation loops, and integrated-result verification before the Integration Branch is pushed.

**Considered Options**

- Require every Project to provide a TDD skill before issue work can run.
- Ignore Project-specific instructions and use one global Control Plane prompt.
- Treat TDD as a domain glossary concept rather than execution policy.
