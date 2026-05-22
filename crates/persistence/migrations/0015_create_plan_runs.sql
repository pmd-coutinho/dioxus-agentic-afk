CREATE TABLE project_execution_configs (
    project_id TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    integration_branch TEXT NOT NULL,
    max_parallel_tasks INTEGER NOT NULL,
    review_retry_limit INTEGER NOT NULL
);

CREATE TABLE plan_runs (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    integration_branch TEXT NOT NULL,
    baseline_commit TEXT NOT NULL,
    state TEXT NOT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT
);

CREATE INDEX idx_plan_runs_project_state ON plan_runs (project_id, state);

CREATE TABLE plan_run_phase_outputs (
    id TEXT PRIMARY KEY,
    plan_run_id TEXT NOT NULL REFERENCES plan_runs(id) ON DELETE CASCADE,
    phase TEXT NOT NULL,
    outcome TEXT NOT NULL,
    body_json TEXT NOT NULL,
    recorded_at TEXT NOT NULL
);

CREATE INDEX idx_plan_run_phase_outputs_plan_run ON plan_run_phase_outputs (plan_run_id);
