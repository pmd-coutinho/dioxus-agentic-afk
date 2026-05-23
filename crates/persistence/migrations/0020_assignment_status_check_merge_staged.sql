-- Issue #51 (ADR-0037): extend the `issue_assignments.status` taxonomy to
-- include `merge_staged`, the dormant **Assignment Status** that sits
-- between `merging` and `merged` when the **Merge Phase** has integrated
-- locally and verified but the **Integration Branch** push has not yet
-- succeeded.
--
-- SQLite does not support `ALTER TABLE ... ADD CHECK`, so we rebuild
-- `issue_assignments` with a CHECK constraint that names every valid
-- **Assignment Status** discriminator the application writes today. The
-- enum lives in `agentic_afk_orchestrator::plan_run_status::AssignmentStatus`
-- and the contracts taxonomy in `agentic_afk_contracts::AssignmentStatusKind`.
--
-- Statuses recognized:
--   provisional, claimed, re_enabled, implementing, implemented,
--   reviewed, rejected, merging, merge_staged, merged, blocked, abandoned.
--
-- `re_enabled` and `abandoned` predate ADR-0037 and are kept for back-compat
-- with legacy rows that may exist in long-lived developer databases.

PRAGMA foreign_keys=OFF;

CREATE TABLE issue_assignments_new (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL,
    source_kind TEXT NOT NULL,
    source_locator TEXT NOT NULL,
    source_id TEXT NOT NULL,
    source_title TEXT NOT NULL,
    source_raw_text TEXT NOT NULL,
    branch TEXT NOT NULL,
    worktree_path TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL CHECK (status IN (
        'provisional',
        'claimed',
        're_enabled',
        'implementing',
        'implemented',
        'reviewed',
        'rejected',
        'merging',
        'merge_staged',
        'merged',
        'blocked',
        'abandoned'
    )),
    status_detail TEXT,
    plan_run_id TEXT REFERENCES plan_runs(id) ON DELETE CASCADE,
    selection_summary TEXT,
    review_rejection_count INTEGER NOT NULL DEFAULT 0,
    block_reason TEXT,
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
);

INSERT INTO issue_assignments_new (
    id, project_id, source_kind, source_locator, source_id, source_title,
    source_raw_text, branch, worktree_path, status, status_detail,
    plan_run_id, selection_summary, review_rejection_count, block_reason
)
SELECT
    id, project_id, source_kind, source_locator, source_id, source_title,
    source_raw_text, branch, worktree_path, status, status_detail,
    plan_run_id, selection_summary, review_rejection_count, block_reason
FROM issue_assignments;

DROP TABLE issue_assignments;
ALTER TABLE issue_assignments_new RENAME TO issue_assignments;

CREATE INDEX idx_issue_assignments_plan_run ON issue_assignments(plan_run_id);

CREATE UNIQUE INDEX idx_issue_assignments_plan_run_source
    ON issue_assignments(plan_run_id, source_id)
    WHERE plan_run_id IS NOT NULL;

CREATE UNIQUE INDEX idx_issue_assignments_plan_run_branch
    ON issue_assignments(plan_run_id, branch)
    WHERE plan_run_id IS NOT NULL;

PRAGMA foreign_keys=ON;
