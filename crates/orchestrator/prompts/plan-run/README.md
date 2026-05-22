# Plan Run phase prompts

These are product-owned prompt templates for the Sandcastle-style Plan Run flow.

They adapt the pinned upstream prompt references in `docs/reference/sandcastle-prompts/` to Dioxus Agentic AFK decisions:

- every phase receives Project Instructions
- planning sees eligible `ready-for-agent` Source Issues and hard blockers from issue descriptions
- one Plan Run baseline is shared by planning and selected issue assignments
- Project configuration supplies the Integration Branch and Max Parallel Tasks
- reviewers approve or reject with findings and do not edit project files
- merge integrates reviewed successes, verifies the integrated result, pushes the Integration Branch, and reports merged issues
- phase results are shaped for durable Phase Outputs rather than issue-body side effects

The placeholders use `{{NAME}}` form until the orchestrator owns concrete prompt rendering and structured phase output schemas.
