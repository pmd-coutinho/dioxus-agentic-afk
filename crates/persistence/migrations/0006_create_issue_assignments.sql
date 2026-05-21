CREATE TABLE issue_assignments (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL,
    source_kind TEXT NOT NULL,
    source_locator TEXT NOT NULL,
    source_id TEXT NOT NULL,
    source_title TEXT NOT NULL,
    source_raw_text TEXT NOT NULL,
    branch TEXT NOT NULL,
    worktree_path TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL,
    status_detail TEXT,
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX one_active_issue_assignment_per_project
    ON issue_assignments(project_id)
    WHERE status != 'abandoned';

CREATE UNIQUE INDEX one_issue_assignment_branch_per_project
    ON issue_assignments(project_id, branch);

CREATE TABLE assignment_attempts (
    id TEXT PRIMARY KEY NOT NULL,
    assignment_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    process_id INTEGER,
    terminal_outcome_json TEXT,
    FOREIGN KEY (assignment_id) REFERENCES issue_assignments(id) ON DELETE CASCADE
);
