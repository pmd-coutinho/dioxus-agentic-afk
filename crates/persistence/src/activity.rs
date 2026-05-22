//! Persistence helpers for Project Activity (the chronological record of
//! Control Plane lifecycle events surfaced in the Dashboard).

use crate::{Db, PersistenceError};
use uuid::Uuid;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectActivityEntry {
    pub id: String,
    pub project_id: String,
    pub assignment_id: Option<String>,
    pub kind: String,
    pub detail: Option<String>,
    pub recorded_at: String,
}

/// Maximum byte length stored for `detail`. Activity is a control-plane event
/// log, not an agent output channel — full Codex output must never land here
/// (ADR-0030).
pub const PROJECT_ACTIVITY_DETAIL_MAX_BYTES: usize = 512;

fn truncate_detail(detail: Option<&str>) -> Option<String> {
    detail.map(|raw| {
        if raw.len() <= PROJECT_ACTIVITY_DETAIL_MAX_BYTES {
            return raw.to_string();
        }
        let mut end = PROJECT_ACTIVITY_DETAIL_MAX_BYTES;
        while end > 0 && !raw.is_char_boundary(end) {
            end -= 1;
        }
        let mut truncated = raw[..end].to_string();
        truncated.push('…');
        truncated
    })
}

pub async fn record_project_activity(
    db: &Db,
    project_id: &str,
    assignment_id: Option<&str>,
    kind: &str,
    detail: Option<&str>,
) -> Result<ProjectActivityEntry, PersistenceError> {
    let detail = truncate_detail(detail);
    let id = Uuid::new_v4().to_string();
    let recorded_at = current_unix_timestamp();
    sqlx::query(
        r#"
        INSERT INTO project_activity (id, project_id, assignment_id, kind, detail, recorded_at)
        VALUES (?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&id)
    .bind(project_id)
    .bind(assignment_id)
    .bind(kind)
    .bind(&detail)
    .bind(&recorded_at)
    .execute(db)
    .await?;
    Ok(ProjectActivityEntry {
        id,
        project_id: project_id.to_string(),
        assignment_id: assignment_id.map(str::to_string),
        kind: kind.to_string(),
        detail,
        recorded_at,
    })
}

pub async fn list_project_activity(
    db: &Db,
    project_id: &str,
    limit: i64,
) -> Result<Vec<ProjectActivityEntry>, PersistenceError> {
    let rows = sqlx::query_as::<
        _,
        (
            String,
            String,
            Option<String>,
            String,
            Option<String>,
            String,
        ),
    >(
        r#"
        SELECT id, project_id, assignment_id, kind, detail, recorded_at
        FROM project_activity
        WHERE project_id = ?
        ORDER BY recorded_at DESC, rowid DESC
        LIMIT ?
        "#,
    )
    .bind(project_id)
    .bind(limit)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, project_id, assignment_id, kind, detail, recorded_at)| ProjectActivityEntry {
                id,
                project_id,
                assignment_id,
                kind,
                detail,
                recorded_at,
            },
        )
        .collect())
}

fn current_unix_timestamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("unix:{seconds}")
}
