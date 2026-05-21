-- Verified and completed Change Proposals release the Project execution slot,
-- so they must not occupy the one-active-assignment-per-project index.
DROP INDEX IF EXISTS one_active_issue_assignment_per_project;
CREATE UNIQUE INDEX one_active_issue_assignment_per_project
    ON issue_assignments(project_id)
    WHERE status NOT IN ('abandoned', 'proposal_verified', 'completed');
