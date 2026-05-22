-- Issue #44: bound the Review Loop and block exhausted assignments.
--
-- `review_rejection_count` tracks how many rejected Review Phases an Issue
-- Assignment has accumulated. When the count reaches the Project Review
-- Retry Limit, the assignment is moved into a coarse `blocked` lifecycle
-- state and remains there until a human re-enables it.
--
-- `block_reason` captures the human-readable reason persisted alongside
-- the blocked lifecycle state so the Dashboard and Issue Source write-back
-- can surface it without re-parsing review findings.

ALTER TABLE issue_assignments
    ADD COLUMN review_rejection_count INTEGER NOT NULL DEFAULT 0;

ALTER TABLE issue_assignments
    ADD COLUMN block_reason TEXT;
