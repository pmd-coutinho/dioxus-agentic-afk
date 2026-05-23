-- Issue #68 (ADR-0042 S1): every Codex phase persists a `plan_run_phase_outputs`
-- row with `outcome = 'in_flight'` *before* the Codex Sandbox is launched, then
-- updates that row on completion. Two new nullable columns capture the spawned
-- child PID + start timestamp so the boot recovery scanner can prove the row
-- corresponds to a real OS process. `body_json` becomes nullable so an
-- in-flight row can exist before the agent returns.
--
-- The widened `outcome` domain (`in_flight`, `interrupted`) is enforced at the
-- write seam in `crates/persistence/src/plan_run.rs` rather than via a CHECK
-- constraint; SQLite does not allow modifying an existing CHECK without a
-- table rebuild and the existing column has no CHECK to widen.
--
-- The `OrchestratorRestart` BlockReasonKind is likewise enforced at the write
-- seam via `agentic_afk_contracts::BlockReason::from_wire`; the column has no
-- CHECK constraint to widen.

ALTER TABLE plan_run_phase_outputs
    ADD COLUMN process_id INTEGER;

ALTER TABLE plan_run_phase_outputs
    ADD COLUMN process_started_at TEXT;

-- SQLite cannot drop the NOT NULL constraint on an existing column without
-- rebuilding the table. Rebuild via the documented temp-table dance so
-- in-flight rows may carry NULL `body_json` until completion writes the
-- parsed body.
CREATE TABLE plan_run_phase_outputs_new (
    id TEXT PRIMARY KEY,
    plan_run_id TEXT NOT NULL REFERENCES plan_runs(id) ON DELETE CASCADE,
    phase TEXT NOT NULL,
    outcome TEXT NOT NULL,
    body_json TEXT,
    recorded_at TEXT NOT NULL,
    assignment_id TEXT REFERENCES issue_assignments(id) ON DELETE CASCADE,
    process_id INTEGER,
    process_started_at TEXT
);

INSERT INTO plan_run_phase_outputs_new (
    id, plan_run_id, phase, outcome, body_json, recorded_at, assignment_id,
    process_id, process_started_at
)
SELECT id, plan_run_id, phase, outcome, body_json, recorded_at, assignment_id,
    process_id, process_started_at
FROM plan_run_phase_outputs;

DROP TABLE plan_run_phase_outputs;
ALTER TABLE plan_run_phase_outputs_new RENAME TO plan_run_phase_outputs;

CREATE INDEX idx_plan_run_phase_outputs_plan_run
    ON plan_run_phase_outputs (plan_run_id);
CREATE INDEX idx_plan_run_phase_outputs_assignment
    ON plan_run_phase_outputs (assignment_id);

-- Index the in-flight set so the ShutdownCoordinator and BootRecoveryScanner
-- both find non-terminal rows cheaply without scanning the whole audit log.
CREATE INDEX idx_plan_run_phase_outputs_in_flight
    ON plan_run_phase_outputs (outcome)
    WHERE outcome IN ('in_flight', 'interrupted');
