# Accept explicit local Project paths for boilerplate

The boilerplate will accept explicit local Project paths and validate that they exist as directories, without introducing trusted-root enforcement yet. Once agent execution is added, Project paths become a security boundary and should be revisited as configured trusted roots rather than an implicit permission model.

**Considered Options**

- Require configured trusted roots before any Project can be added.
- Accept explicit local Project paths during the boilerplate phase.
