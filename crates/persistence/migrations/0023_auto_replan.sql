ALTER TABLE projects ADD COLUMN auto_replan_state TEXT NOT NULL DEFAULT 'off'
    CHECK (auto_replan_state IN ('off', 'armed', 'paused'));

ALTER TABLE projects ADD COLUMN auto_replan_pause_reason TEXT NULL
    CHECK (
        auto_replan_pause_reason IS NULL OR
        auto_replan_pause_reason IN (
            'empty_backlog',
            'assignment_blocked',
            'push_non_fast_forward',
            'merge_staged_left',
            'planning_failed',
            'sync_failed'
        )
    );
