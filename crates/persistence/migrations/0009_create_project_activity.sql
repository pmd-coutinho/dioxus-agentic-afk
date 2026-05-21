CREATE TABLE project_activity (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL,
    assignment_id TEXT,
    kind TEXT NOT NULL,
    detail TEXT,
    recorded_at TEXT NOT NULL,
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
);

CREATE INDEX project_activity_by_project ON project_activity(project_id, recorded_at);
