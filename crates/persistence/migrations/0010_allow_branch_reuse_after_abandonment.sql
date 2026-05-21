-- Allow the same deterministic assignment branch to be reused after the prior
-- Issue Assignment has been abandoned, since the Assignment Worktree and
-- branch are removed during abandonment.
DROP INDEX IF EXISTS one_issue_assignment_branch_per_project;

CREATE UNIQUE INDEX one_active_issue_assignment_branch_per_project
    ON issue_assignments(project_id, branch)
    WHERE status != 'abandoned';
