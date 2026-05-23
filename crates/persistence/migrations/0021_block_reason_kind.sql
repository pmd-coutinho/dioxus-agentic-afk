-- Issue #52 (ADR-0038): promote the **Block Reason** from a freeform
-- `Option<String>` to a typed kind plus optional freeform detail. The
-- existing `block_reason` column keeps the freeform `detail` text; this
-- migration adds `block_reason_kind` with the lower-snake-case wire
-- encoding for the typed kind defined by
-- `agentic_afk_contracts::BlockReason`.
--
-- Existing rows are backfilled best-effort from the freeform `block_reason`
-- text: any row whose reason mentions a review-loop concept maps to
-- `review_retry_limit_exhausted`; every other non-NULL reason maps to
-- `merge_phase_failed` (the only other block site that existed before
-- this slice). Rows with no recorded reason remain `NULL` because there
-- is no signal to typify them.

ALTER TABLE issue_assignments
    ADD COLUMN block_reason_kind TEXT;

UPDATE issue_assignments
SET block_reason_kind = 'review_retry_limit_exhausted'
WHERE block_reason IS NOT NULL
  AND lower(block_reason) LIKE '%review%';

UPDATE issue_assignments
SET block_reason_kind = 'merge_phase_failed'
WHERE block_reason IS NOT NULL
  AND block_reason_kind IS NULL;
