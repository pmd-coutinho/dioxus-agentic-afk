//! Operator-marked Parent-Issue-style PRDs.
//!
//! The Control Plane lets a human flag a Source Issue as a Parent Issue when
//! the upstream Issue Source did not. PRD-marked rows are filtered out of
//! every active Planning Snapshot bucket so no agent picks them for
//! implementation. See ADR (Parent Issue glossary in `CONTEXT.md`).

use crate::{Db, PersistenceError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrdOverride {
    pub source_id: String,
    pub marked_at: String,
}

/// Mark a Source Issue as a PRD. Re-marking the same `(project_id, source_id)`
/// is a no-op (the original `marked_at` is preserved). Returns the resulting
/// list of PRD source ids for the project so the caller can publish an event
/// without a second query.
pub async fn mark_prd(
    db: &Db,
    project_id: &str,
    source_id: &str,
) -> Result<Vec<PrdOverride>, PersistenceError> {
    sqlx::query(
        "INSERT INTO project_prd_overrides (project_id, source_id, marked_at) \
         VALUES (?1, ?2, datetime('now')) \
         ON CONFLICT(project_id, source_id) DO NOTHING",
    )
    .bind(project_id)
    .bind(source_id)
    .execute(db)
    .await?;
    list_prd_overrides(db, project_id).await
}

pub async fn unmark_prd(
    db: &Db,
    project_id: &str,
    source_id: &str,
) -> Result<Vec<PrdOverride>, PersistenceError> {
    sqlx::query(
        "DELETE FROM project_prd_overrides WHERE project_id = ?1 AND source_id = ?2",
    )
    .bind(project_id)
    .bind(source_id)
    .execute(db)
    .await?;
    list_prd_overrides(db, project_id).await
}

pub async fn list_prd_overrides(
    db: &Db,
    project_id: &str,
) -> Result<Vec<PrdOverride>, PersistenceError> {
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT source_id, marked_at FROM project_prd_overrides \
         WHERE project_id = ?1 ORDER BY marked_at, source_id",
    )
    .bind(project_id)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(source_id, marked_at)| PrdOverride { source_id, marked_at })
        .collect())
}
