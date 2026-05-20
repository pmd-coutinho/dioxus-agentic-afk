CREATE TABLE IF NOT EXISTS project_issue_sources (
    project_id TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL,
    locator TEXT NOT NULL,
    FOREIGN KEY(project_id) REFERENCES projects(id) ON DELETE CASCADE
);
