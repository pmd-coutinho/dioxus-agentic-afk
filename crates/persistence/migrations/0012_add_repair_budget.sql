ALTER TABLE issue_assignments ADD COLUMN repair_attempt_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE issue_assignments ADD COLUMN repair_window_started_at INTEGER;
ALTER TABLE issue_assignments ADD COLUMN repair_max_attempts INTEGER NOT NULL DEFAULT 3;
ALTER TABLE issue_assignments ADD COLUMN repair_window_seconds INTEGER NOT NULL DEFAULT 3600;
