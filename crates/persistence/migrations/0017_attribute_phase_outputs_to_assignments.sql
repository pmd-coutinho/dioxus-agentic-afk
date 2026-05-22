-- Issue #43: implementation and review Phase Outputs are nested under an
-- Issue Assignment within their Plan Run. The planning Phase Output stays
-- scoped to the Plan Run with assignment_id = NULL.

ALTER TABLE plan_run_phase_outputs ADD COLUMN assignment_id TEXT
    REFERENCES issue_assignments(id) ON DELETE CASCADE;

CREATE INDEX idx_plan_run_phase_outputs_assignment
    ON plan_run_phase_outputs (assignment_id);
