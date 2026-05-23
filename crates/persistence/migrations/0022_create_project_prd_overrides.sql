-- Operator-marked Source Issues to treat as Parent-Issue-style PRDs.
-- These rows hide a Source Issue from every active Planning Snapshot bucket so
-- it cannot be picked for direct agent implementation. The marking is local;
-- it does not write back to the upstream Issue Source.
CREATE TABLE project_prd_overrides (
    project_id TEXT NOT NULL,
    source_id  TEXT NOT NULL,
    marked_at  TEXT NOT NULL,
    PRIMARY KEY (project_id, source_id),
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
);
