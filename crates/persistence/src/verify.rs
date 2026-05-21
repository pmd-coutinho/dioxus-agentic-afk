//! Persistence helpers for verifying Change Proposals and completing Human Merges.
//!
//! Implements the proposal_pending -> proposal_verified -> completed lifecycle
//! for the assignment row plus the corresponding change_proposal_status,
//! release of the execution slot, and lookup helpers used by the verify route.

use crate::{Db, PersistenceError};
use agentic_afk_contracts::IssueAssignmentResponse;

/// Look up an assignment by id, regardless of its status. Returns NotFound
/// (mapped to `AssignmentNotFound`) when the assignment does not exist.
pub async fn get_assignment_by_id(
    db: &Db,
    assignment_id: &str,
) -> Result<IssueAssignmentResponse, PersistenceError> {
    crate::get_issue_assignment_public(db, assignment_id).await
}

/// Mark a Change Proposal as verified: required checks passed, release the
/// execution slot but preserve the assignment row for review.
pub async fn mark_proposal_verified(
    db: &Db,
    assignment_id: &str,
) -> Result<IssueAssignmentResponse, PersistenceError> {
    let result = sqlx::query(
        "UPDATE issue_assignments SET status = 'proposal_verified', status_detail = NULL, change_proposal_status = 'verified' WHERE id = ?",
    )
    .bind(assignment_id)
    .execute(db)
    .await?;
    if result.rows_affected() == 0 {
        return Err(PersistenceError::AssignmentNotFound(
            assignment_id.to_string(),
        ));
    }
    crate::get_issue_assignment_public(db, assignment_id).await
}

/// Mark a Change Proposal as merged + completed after Human Merge is detected.
pub async fn mark_assignment_completed(
    db: &Db,
    assignment_id: &str,
) -> Result<IssueAssignmentResponse, PersistenceError> {
    let result = sqlx::query(
        "UPDATE issue_assignments SET status = 'completed', status_detail = NULL, change_proposal_status = 'merged' WHERE id = ?",
    )
    .bind(assignment_id)
    .execute(db)
    .await?;
    if result.rows_affected() == 0 {
        return Err(PersistenceError::AssignmentNotFound(
            assignment_id.to_string(),
        ));
    }
    crate::get_issue_assignment_public(db, assignment_id).await
}

/// Record failing required checks: assignment is blocked with the supplied
/// detail and the proposal stays attached for repair.
pub async fn mark_proposal_failing(
    db: &Db,
    assignment_id: &str,
    detail: &str,
) -> Result<IssueAssignmentResponse, PersistenceError> {
    let result = sqlx::query(
        "UPDATE issue_assignments SET status = 'blocked', status_detail = ?, change_proposal_status = 'failing' WHERE id = ?",
    )
    .bind(detail)
    .bind(assignment_id)
    .execute(db)
    .await?;
    if result.rows_affected() == 0 {
        return Err(PersistenceError::AssignmentNotFound(
            assignment_id.to_string(),
        ));
    }
    crate::get_issue_assignment_public(db, assignment_id).await
}
