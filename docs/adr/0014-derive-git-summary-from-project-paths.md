# Derive Git Summary from Project paths

The dashboard may show read-only Git metadata for Projects, such as branch and dirty status, by deriving it from the Project path. Git metadata should not be persisted as core Project state, and a Project does not need to be a Git repository.

Git Summary should be gathered through a Rust Git library such as `gix`, not by shelling out to `git`.

**Considered Options**

- Store Git metadata as part of Project state.
- Require every Project to be a Git repository.
- Derive Git Summary on read when available.
- Shell out to `git` for metadata.
- Use a Rust Git library for metadata.
