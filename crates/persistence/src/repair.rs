//! Repair budget bookkeeping for the bounded GitHub Repair Loop.
//!
//! Repair Assignment Attempts run inside the existing Issue Assignment and
//! Assignment Worktree after a Change Proposal's required checks fail. The
//! Control Plane bounds them by two limits, both tracked here:
//!
//! - `repair_attempt_count` — number of repair Assignment Attempts that have
//!   already been recorded against the assignment.
//! - elapsed window — wall-clock seconds since the first repair Assignment
//!   Attempt started (`repair_window_started_at`).
//!
//! Recovery Assignment Attempts are explicit human-triggered continuations of
//! blocked assignments. They must NOT advance the repair budget — see
//! `record_recovery_attempt`, which intentionally only writes the attempt row.

use crate::{Db, PersistenceError, get_issue_assignment};
use agentic_afk_contracts::{AssignmentTerminalOutcome, IssueAssignmentResponse};
use uuid::Uuid;

/// Outcome of a repair budget check before launching a repair Assignment Attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RepairBudgetDecision {
    /// Budget has room. Caller may launch a repair Assignment Attempt.
    Allow,
    /// Attempt count has reached `max_attempts`.
    AttemptsExhausted { attempt_count: i64, max_attempts: i64 },
    /// Elapsed time since the first repair attempt has reached
    /// `window_seconds`.
    WindowExpired {
        window_started_at: i64,
        window_seconds: i64,
        now: i64,
    },
}

impl RepairBudgetDecision {
    pub fn is_allow(&self) -> bool {
        matches!(self, RepairBudgetDecision::Allow)
    }

    pub fn block_detail(&self) -> Option<String> {
        match self {
            RepairBudgetDecision::Allow => None,
            RepairBudgetDecision::AttemptsExhausted {
                attempt_count,
                max_attempts,
            } => Some(format!(
                "repair budget exhausted: {attempt_count} of {max_attempts} attempts used; recovery or abandonment required"
            )),
            RepairBudgetDecision::WindowExpired {
                window_started_at,
                window_seconds,
                now,
            } => Some(format!(
                "repair budget exhausted: {window_seconds}s window started at unix:{window_started_at} elapsed (now unix:{now}); recovery or abandonment required"
            )),
        }
    }
}

/// Inspect the repair budget for an assignment without mutating it.
///
/// `now_unix_seconds` is injectable so tests can advance the elapsed window
/// without sleeping.
pub async fn evaluate_repair_budget(
    db: &Db,
    assignment_id: &str,
    now_unix_seconds: i64,
) -> Result<RepairBudgetDecision, PersistenceError> {
    let row =
        sqlx::query_as::<_, (i64, Option<i64>, i64, i64)>(
            r#"
            SELECT repair_attempt_count, repair_window_started_at, repair_max_attempts, repair_window_seconds
            FROM issue_assignments
            WHERE id = ?
            "#,
        )
        .bind(assignment_id)
        .fetch_optional(db)
        .await?
        .ok_or_else(|| PersistenceError::AssignmentNotFound(assignment_id.to_string()))?;
    let (attempt_count, window_started_at, max_attempts, window_seconds) = row;
    if attempt_count >= max_attempts {
        return Ok(RepairBudgetDecision::AttemptsExhausted {
            attempt_count,
            max_attempts,
        });
    }
    if let Some(started) = window_started_at {
        if now_unix_seconds.saturating_sub(started) >= window_seconds {
            return Ok(RepairBudgetDecision::WindowExpired {
                window_started_at: started,
                window_seconds,
                now: now_unix_seconds,
            });
        }
    }
    Ok(RepairBudgetDecision::Allow)
}

/// Record one repair Assignment Attempt and advance the repair budget.
///
/// Fails with `RepairBudgetExhausted` if `evaluate_repair_budget` would not
/// allow the attempt, so callers can rely on this function to enforce the
/// invariant atomically rather than racing the check.
pub async fn record_repair_attempt(
    db: &Db,
    assignment_id: &str,
    process_id: Option<u32>,
    process_identity: Option<&str>,
    terminal_outcome: Option<&AssignmentTerminalOutcome>,
    now_unix_seconds: i64,
) -> Result<IssueAssignmentResponse, PersistenceError> {
    let mut tx = db.begin().await?;

    let row = sqlx::query_as::<_, (i64, Option<i64>, i64, i64)>(
        r#"
        SELECT repair_attempt_count, repair_window_started_at, repair_max_attempts, repair_window_seconds
        FROM issue_assignments
        WHERE id = ?
        "#,
    )
    .bind(assignment_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| PersistenceError::AssignmentNotFound(assignment_id.to_string()))?;
    let (attempt_count, window_started_at, max_attempts, window_seconds) = row;
    if attempt_count >= max_attempts {
        return Err(PersistenceError::RepairBudgetExhausted(
            assignment_id.to_string(),
        ));
    }
    if let Some(started) = window_started_at {
        if now_unix_seconds.saturating_sub(started) >= window_seconds {
            return Err(PersistenceError::RepairBudgetExhausted(
                assignment_id.to_string(),
            ));
        }
    }
    let started_stamp = window_started_at.unwrap_or(now_unix_seconds);
    let attempt_id = Uuid::new_v4().to_string();
    let terminal_outcome_json = terminal_outcome
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| PersistenceError::Database(sqlx::Error::Decode(Box::new(error))))?;
    sqlx::query(
        r#"
        INSERT INTO assignment_attempts (
            id, assignment_id, kind, process_id, process_identity, terminal_outcome_json
        )
        VALUES (?, ?, 'repair', ?, ?, ?)
        "#,
    )
    .bind(&attempt_id)
    .bind(assignment_id)
    .bind(process_id.map(i64::from))
    .bind(process_identity)
    .bind(terminal_outcome_json)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE issue_assignments
        SET repair_attempt_count = repair_attempt_count + 1,
            repair_window_started_at = ?
        WHERE id = ?
        "#,
    )
    .bind(started_stamp)
    .bind(assignment_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    get_issue_assignment(db, assignment_id).await
}

/// Record a recovery Assignment Attempt without touching the repair budget.
///
/// This is the explicit boundary that keeps recovery (continuation of blocked
/// work) separate from repair (CI failure response). See ADR-0029 and the
/// Repair Loop language in `CONTEXT.md`.
pub async fn record_recovery_attempt(
    db: &Db,
    assignment_id: &str,
    process_id: Option<u32>,
    process_identity: Option<&str>,
    terminal_outcome: Option<&AssignmentTerminalOutcome>,
) -> Result<IssueAssignmentResponse, PersistenceError> {
    let attempt_id = Uuid::new_v4().to_string();
    let terminal_outcome_json = terminal_outcome
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| PersistenceError::Database(sqlx::Error::Decode(Box::new(error))))?;
    sqlx::query(
        r#"
        INSERT INTO assignment_attempts (
            id, assignment_id, kind, process_id, process_identity, terminal_outcome_json
        )
        VALUES (?, ?, 'recovery', ?, ?, ?)
        "#,
    )
    .bind(attempt_id)
    .bind(assignment_id)
    .bind(process_id.map(i64::from))
    .bind(process_identity)
    .bind(terminal_outcome_json)
    .execute(db)
    .await?;
    get_issue_assignment(db, assignment_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        connect_in_memory, create_issue_assignment, create_project, enable_issue_source, migrate,
        set_assignment_change_proposal,
    };
    use agentic_afk_contracts::{
        CreateProjectRequest, EnableIssueSourceRequest, IssueSource, SourceIssueSnapshot,
    };

    async fn fixture() -> (Db, String) {
        let db = connect_in_memory().await.unwrap();
        migrate(&db).await.unwrap();
        let project = create_project(
            &db,
            &CreateProjectRequest {
                path: "/tmp".to_string(),
            },
        )
        .await
        .unwrap();
        let source = IssueSource {
            kind: "github".to_string(),
            locator: "owner/repo".to_string(),
        };
        enable_issue_source(
            &db,
            &project.id.0,
            &EnableIssueSourceRequest {
                kind: source.kind.clone(),
                locator: source.locator.clone(),
            },
        )
        .await
        .unwrap();
        let issue = SourceIssueSnapshot {
            source_id: "21".to_string(),
            title: "Failing checks".to_string(),
            readiness: "ready".to_string(),
            lifecycle_status: "running".to_string(),
            parent_issue: None,
            issue_dependencies: vec![],
            source_order: 1,
            raw_text: "Fix the CI".to_string(),
        };
        let assignment = create_issue_assignment(
            &db,
            &project.id.0,
            &source,
            &issue,
            "agentic-afk/github-21",
        )
        .await
        .unwrap();
        set_assignment_change_proposal(
            &db,
            &assignment.id,
            "failed",
            "https://github.com/owner/repo/pull/42",
        )
        .await
        .unwrap();
        (db, assignment.id)
    }

    #[tokio::test]
    async fn repair_attempts_advance_budget_and_stamp_window_start() {
        let (db, assignment_id) = fixture().await;
        let outcome = AssignmentTerminalOutcome {
            outcome: "Blocked".to_string(),
            summary: "still failing".to_string(),
        };
        let assignment =
            record_repair_attempt(&db, &assignment_id, Some(1), None, Some(&outcome), 1000)
                .await
                .unwrap();
        let budget = assignment.repair_budget.unwrap();
        assert_eq!(budget.attempt_count, 1);
        assert_eq!(budget.window_started_at, Some(1000));
        let assignment =
            record_repair_attempt(&db, &assignment_id, Some(1), None, Some(&outcome), 1500)
                .await
                .unwrap();
        let budget = assignment.repair_budget.unwrap();
        assert_eq!(budget.attempt_count, 2);
        // Window-start stamp does NOT move on subsequent repair attempts.
        assert_eq!(budget.window_started_at, Some(1000));
    }

    #[tokio::test]
    async fn repair_budget_blocks_when_attempts_exhausted() {
        let (db, assignment_id) = fixture().await;
        let outcome = AssignmentTerminalOutcome {
            outcome: "Blocked".to_string(),
            summary: "failed".to_string(),
        };
        for now in [1000, 1100, 1200] {
            record_repair_attempt(&db, &assignment_id, Some(1), None, Some(&outcome), now)
                .await
                .unwrap();
        }
        let decision = evaluate_repair_budget(&db, &assignment_id, 1300).await.unwrap();
        assert!(matches!(
            decision,
            RepairBudgetDecision::AttemptsExhausted { .. }
        ));
        let err = record_repair_attempt(&db, &assignment_id, Some(1), None, Some(&outcome), 1300)
            .await
            .unwrap_err();
        assert!(matches!(err, PersistenceError::RepairBudgetExhausted(_)));
    }

    #[tokio::test]
    async fn repair_budget_blocks_when_window_elapses() {
        let (db, assignment_id) = fixture().await;
        let outcome = AssignmentTerminalOutcome {
            outcome: "Blocked".to_string(),
            summary: "failed".to_string(),
        };
        record_repair_attempt(&db, &assignment_id, Some(1), None, Some(&outcome), 1000)
            .await
            .unwrap();
        let later = 1000 + 3600;
        let decision = evaluate_repair_budget(&db, &assignment_id, later).await.unwrap();
        assert!(matches!(decision, RepairBudgetDecision::WindowExpired { .. }));
        let err = record_repair_attempt(&db, &assignment_id, Some(1), None, Some(&outcome), later)
            .await
            .unwrap_err();
        assert!(matches!(err, PersistenceError::RepairBudgetExhausted(_)));
    }

    #[tokio::test]
    async fn recovery_attempts_do_not_advance_repair_budget() {
        let (db, assignment_id) = fixture().await;
        let outcome = AssignmentTerminalOutcome {
            outcome: "Blocked".to_string(),
            summary: "resumed".to_string(),
        };
        let assignment =
            record_recovery_attempt(&db, &assignment_id, Some(1), None, Some(&outcome))
                .await
                .unwrap();
        let budget = assignment.repair_budget.unwrap();
        assert_eq!(budget.attempt_count, 0);
        assert_eq!(budget.window_started_at, None);
        assert_eq!(assignment.latest_attempt.unwrap().kind, "recovery");
    }
}
