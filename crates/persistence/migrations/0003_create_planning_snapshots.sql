CREATE TABLE issue_source_sync_status (
    project_id TEXT PRIMARY KEY NOT NULL,
    source_kind TEXT NOT NULL,
    source_locator TEXT NOT NULL,
    last_successful_sync_at TEXT,
    last_failure TEXT,
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
);

CREATE TABLE planning_snapshot_issues (
    project_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    title TEXT NOT NULL,
    readiness TEXT NOT NULL,
    parent_issue TEXT,
    issue_dependencies_json TEXT NOT NULL,
    source_order INTEGER NOT NULL,
    raw_text TEXT NOT NULL,
    PRIMARY KEY (project_id, source_id),
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
);
