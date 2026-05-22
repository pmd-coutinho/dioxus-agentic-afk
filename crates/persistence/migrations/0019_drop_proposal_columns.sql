-- Drop proposal-era columns from `issue_assignments`. Issue #47 retired the
-- proposal-era execution surfaces; PRD #40 validation closed the remaining
-- dead-schema gap.
--
-- These columns are no longer read or written by any production code path:
--   * change_proposal_status / change_proposal_url (added in 0008)
--   * repair_attempt_count / repair_window_started_at /
--     repair_max_attempts / repair_window_seconds (added in 0012)
--
-- The associated unique index `one_active_issue_assignment_per_project`
-- that referenced `proposal_verified` was already dropped in 0016 when the
-- assignment nesting under Plan Runs landed, so no extra index cleanup is
-- needed here.

ALTER TABLE issue_assignments DROP COLUMN change_proposal_status;
ALTER TABLE issue_assignments DROP COLUMN change_proposal_url;
ALTER TABLE issue_assignments DROP COLUMN repair_attempt_count;
ALTER TABLE issue_assignments DROP COLUMN repair_window_started_at;
ALTER TABLE issue_assignments DROP COLUMN repair_max_attempts;
ALTER TABLE issue_assignments DROP COLUMN repair_window_seconds;
