# Keep the control plane local-only

The control plane is intended to run locally on one developer's machine rather than as a hosted or team service. The boilerplate should therefore bind locally by default and omit authentication, while treating any future remote access requirement as a reopening of this decision rather than an incremental configuration tweak.

**Considered Options**

- Design for hosted or team access from the start.
- Keep the control plane local-only and unauthenticated.
