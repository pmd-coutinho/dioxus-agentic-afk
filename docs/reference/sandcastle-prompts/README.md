# Sandcastle phase prompt references

These files are pinned copies of Sandcastle's phase prompts from upstream commit `65063f6c8ea2fccde22d7d415be4d03212668678`.

They are reference inputs for this Project's Plan Run prompt design, not the final runtime prompts. The upstream prompts contain Sandcastle-specific assumptions such as `npm` verification commands, `main` diffing, `sandcastle/` branch names, and reviewer edits on the implementation branch. Dioxus Agentic AFK keeps the phase structure but adapts prompt behavior to its Project configuration and glossary decisions.

Copied upstream files:

- `.sandcastle/plan-prompt.md`
- `.sandcastle/implement-prompt.md`
- `.sandcastle/review-prompt.md`
- `.sandcastle/merge-prompt.md`
