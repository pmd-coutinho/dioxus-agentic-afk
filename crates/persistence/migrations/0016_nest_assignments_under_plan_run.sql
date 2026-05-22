-- Issue #42: nest Issue Assignments under a Plan Run.
--
-- Previously issue_assignments held one active assignment per project. With
-- ADR-0034 Plan Runs, an assignment lives inside one Plan Run and a project
-- can hold multiple assignments across runs (one per selected Ready Issue).

DROP INDEX IF EXISTS one_active_issue_assignment_per_project;
DROP INDEX IF EXISTS one_issue_assignment_branch_per_project;

ALTER TABLE issue_assignments ADD COLUMN plan_run_id TEXT
    REFERENCES plan_runs(id) ON DELETE CASCADE;
ALTER TABLE issue_assignments ADD COLUMN selection_summary TEXT;

CREATE INDEX idx_issue_assignments_plan_run ON issue_assignments(plan_run_id);

-- One assignment per (plan_run_id, source_id): the planner cannot select
-- the same Source Issue twice within one Plan Run.
CREATE UNIQUE INDEX idx_issue_assignments_plan_run_source
    ON issue_assignments(plan_run_id, source_id)
    WHERE plan_run_id IS NOT NULL;

-- Branch reuse across Plan Runs is allowed; only within one Plan Run is the
-- branch unique.
CREATE UNIQUE INDEX idx_issue_assignments_plan_run_branch
    ON issue_assignments(plan_run_id, branch)
    WHERE plan_run_id IS NOT NULL;
