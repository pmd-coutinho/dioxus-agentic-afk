//! Persistence helpers for Issue Assignment recovery.
//!
//! Recovery records a new Assignment Attempt of kind `recovery` against an existing
//! Issue Assignment without touching CI repair budget. The Assignment Worktree row is
//! preserved so the replacement Codex pass continues inside the same on-disk
//! Assignment Worktree.

use crate::{Db, PersistenceError, get_issue_assignment_public};
use agentic_afk_contracts::{
    AssignmentAttemptResponse, AssignmentTerminalOutcome, IssueAssignmentResponse,
};
use uuid::Uuid;

/// Record a recovery Assignment Attempt against `assignment_id`. The Assignment must
/// already exist; this never collapses recovery into the initial attempt slot and
/// never charges CI repair budget.
pub async fn record_recovery_attempt(
    db: &Db,
    assignment_id: &str,
    process_id: Option<u32>,
    process_identity: Option<&str>,
    terminal_outcome: Option<&AssignmentTerminalOutcome>,
) -> Result<IssueAssignmentResponse, PersistenceError> {
    // Confirm the assignment exists so callers get a clean NotFound error rather than
    // a foreign-key crash.
    get_issue_assignment_public(db, assignment_id).await?;
    let id = Uuid::new_v4().to_string();
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
    .bind(id)
    .bind(assignment_id)
    .bind(process_id.map(i64::from))
    .bind(process_identity)
    .bind(terminal_outcome_json)
    .execute(db)
    .await?;
    get_issue_assignment_public(db, assignment_id).await
}

/// List every Assignment Attempt for `assignment_id` in chronological order. The
/// caller can use this to audit initial vs recovery vs repair attempts on one
/// Issue Assignment.
pub async fn list_assignment_attempts(
    db: &Db,
    assignment_id: &str,
) -> Result<Vec<AssignmentAttemptResponse>, PersistenceError> {
    let rows =
        sqlx::query_as::<_, (String, String, Option<i64>, Option<String>, Option<String>)>(
            r#"
            SELECT id, kind, process_id, process_identity, terminal_outcome_json
            FROM assignment_attempts
            WHERE assignment_id = ?
            ORDER BY rowid ASC
            "#,
        )
        .bind(assignment_id)
        .fetch_all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, kind, process_id, process_identity, terminal_outcome_json)| {
                let terminal_outcome = terminal_outcome_json
                    .as_deref()
                    .and_then(|json| serde_json::from_str(json).ok());
                AssignmentAttemptResponse {
                    id,
                    kind,
                    process_id: process_id.and_then(|id| u32::try_from(id).ok()),
                    process_identity,
                    terminal_outcome,
                }
            },
        )
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{connect_in_memory, create_issue_assignment, migrate, record_initial_attempt};
    use agentic_afk_contracts::{
        AssignmentTerminalOutcome, CreateProjectRequest, EnableIssueSourceRequest, IssueSource,
        SourceIssueSnapshot,
    };

    async fn setup_blocked_assignment() -> (Db, IssueAssignmentResponse) {
        let db = connect_in_memory().await.unwrap();
        migrate(&db).await.unwrap();
        let project = crate::create_project(
            &db,
            &CreateProjectRequest {
                path: "/tmp".to_string(),
            },
        )
        .await
        .unwrap();
        let source = IssueSource {
            kind: "local_markdown".to_string(),
            locator: ".scratch/issues".to_string(),
        };
        crate::enable_issue_source(
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
            source_id: "rid".to_string(),
            title: "t".to_string(),
            readiness: "ready".to_string(),
            lifecycle_status: "ready".to_string(),
            parent_issue: None,
            issue_dependencies: vec![],
            source_order: 1,
            raw_text: "raw".to_string(),
        };
        let assignment = create_issue_assignment(&db, &project.id.0, &source, &issue, "b")
            .await
            .unwrap();
        record_initial_attempt(
            &db,
            &assignment.id,
            Some(1),
            Some("ident-init"),
            Some(&AssignmentTerminalOutcome {
                outcome: "Blocked".to_string(),
                summary: "init blocked".to_string(),
            }),
        )
        .await
        .unwrap();
        (db, assignment)
    }

    #[tokio::test]
    async fn record_recovery_attempt_adds_recovery_kind_alongside_initial() {
        let (db, assignment) = setup_blocked_assignment().await;
        record_recovery_attempt(
            &db,
            &assignment.id,
            Some(2),
            Some("ident-recover"),
            Some(&AssignmentTerminalOutcome {
                outcome: "Blocked".to_string(),
                summary: "still blocked".to_string(),
            }),
        )
        .await
        .unwrap();

        let attempts = list_assignment_attempts(&db, &assignment.id).await.unwrap();
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].kind, "initial");
        assert_eq!(attempts[1].kind, "recovery");
        assert_eq!(attempts[1].process_id, Some(2));
    }

    #[tokio::test]
    async fn record_recovery_attempt_rejects_unknown_assignment() {
        let db = connect_in_memory().await.unwrap();
        migrate(&db).await.unwrap();
        let result =
            record_recovery_attempt(&db, "missing", Some(1), Some("id"), None).await;
        assert!(matches!(result, Err(PersistenceError::AssignmentNotFound(_))));
    }
}
