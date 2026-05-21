//! Persistence boundary for the Local Control Plane.
//!
//! Provides SQLite-backed storage for Projects.

use agentic_afk_contracts::{
    AssignmentAttemptResponse, AssignmentTerminalOutcome, ChangeProposalResponse,
    CreateProjectRequest, EnableIssueSourceRequest, IssueAssignmentResponse, IssueSource,
    IssueSourceSyncResponse, IssueSourceSyncStatusResponse, PlanningSnapshotResponse, ProjectId,
    ProjectResponse, SourceIssueSnapshot,
};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Pool, Sqlite};
use std::path::Path;
use std::str::FromStr;
use uuid::Uuid;

mod abandon;
mod recover;

pub use abandon::{
    ProjectActivityEntry, abandon_blocked_assignment, get_project_assignment,
    list_project_activity, record_project_activity,
};
pub use recover::{list_assignment_attempts, record_recovery_attempt};

#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    #[error("project not found: {0}")]
    NotFound(String),
    #[error("project path already exists: {0}")]
    Duplicate(String),
    #[error("path does not exist or is not a directory: {0}")]
    InvalidPath(String),
    #[error("invalid issue source: {0}")]
    InvalidIssueSource(String),
    #[error("planning snapshot not found: {0}")]
    SnapshotNotFound(String),
    #[error("Project already has an active Issue Assignment: {0}")]
    ActiveAssignment(String),
    #[error("Issue Assignment not found: {0}")]
    AssignmentNotFound(String),
    #[error("Issue Assignment is not in an abandonable state: {0}")]
    AssignmentNotAbandonable(String),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

pub type Db = Pool<Sqlite>;

/// Create an in-memory SQLite pool for testing.
pub async fn connect_in_memory() -> Result<Db, PersistenceError> {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .map_err(|e| sqlx::Error::Configuration(Box::new(e)))?
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await?;
    Ok(pool)
}

/// Connect to a SQLite database at the given URL.
pub async fn connect(database_url: &str) -> Result<Db, PersistenceError> {
    let opts = SqliteConnectOptions::from_str(database_url)
        .map_err(|e| sqlx::Error::Configuration(Box::new(e)))?
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await?;
    Ok(pool)
}

/// Run all pending migrations.
pub async fn migrate(db: &Db) -> Result<(), PersistenceError> {
    sqlx::migrate!("./migrations")
        .run(db)
        .await
        .map_err(|e| PersistenceError::Database(sqlx::Error::Configuration(Box::new(e))))?;
    Ok(())
}

fn normalize_project_path(path: &str) -> Result<String, PersistenceError> {
    let path = Path::new(path);
    if !path.exists() || !path.is_dir() {
        return Err(PersistenceError::InvalidPath(
            path.to_string_lossy().into_owned(),
        ));
    }

    let canonical_path = std::fs::canonicalize(path)
        .map_err(|_| PersistenceError::InvalidPath(path.to_string_lossy().into_owned()))?;

    if !canonical_path.is_dir() {
        return Err(PersistenceError::InvalidPath(
            path.to_string_lossy().into_owned(),
        ));
    }

    Ok(canonical_path.to_string_lossy().into_owned())
}

/// Create a new Project. Validates that the path exists and is a directory.
pub async fn create_project(
    db: &Db,
    request: &CreateProjectRequest,
) -> Result<ProjectResponse, PersistenceError> {
    let normalized_path = normalize_project_path(&request.path)?;

    let id = Uuid::new_v4().to_string();

    sqlx::query("INSERT INTO projects (id, path, trusted) VALUES (?, ?, 0)")
        .bind(&id)
        .bind(&normalized_path)
        .execute(db)
        .await
        .map_err(|e| match e {
            sqlx::Error::Database(ref db_err)
                if db_err.is_unique_violation() && db_err.code().as_deref() == Some("2067") =>
            {
                PersistenceError::Duplicate(normalized_path.clone())
            }
            other => PersistenceError::Database(other),
        })?;

    Ok(ProjectResponse {
        id: ProjectId(id),
        path: normalized_path,
        trusted: false,
        git_summary: None,
        enabled_issue_source: None,
    })
}

/// List all Projects.
pub async fn list_projects(db: &Db) -> Result<Vec<ProjectResponse>, PersistenceError> {
    let rows = sqlx::query_as::<_, (String, String, i64, Option<String>, Option<String>)>(
        r#"
        SELECT projects.id, projects.path, projects.trusted, project_issue_sources.kind, project_issue_sources.locator
        FROM projects
        LEFT JOIN project_issue_sources ON project_issue_sources.project_id = projects.id
        ORDER BY projects.path
        "#,
    )
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(id, path, trusted, kind, locator)| ProjectResponse {
            id: ProjectId(id),
            path,
            trusted: trusted != 0,
            git_summary: None,
            enabled_issue_source: issue_source_from_row(kind, locator),
        })
        .collect())
}

/// Get a single Project by ID.
pub async fn get_project(db: &Db, id: &str) -> Result<ProjectResponse, PersistenceError> {
    let row = sqlx::query_as::<_, (String, String, i64, Option<String>, Option<String>)>(
        r#"
        SELECT projects.id, projects.path, projects.trusted, project_issue_sources.kind, project_issue_sources.locator
        FROM projects
        LEFT JOIN project_issue_sources ON project_issue_sources.project_id = projects.id
        WHERE projects.id = ?
        "#,
    )
    .bind(id)
    .fetch_optional(db)
    .await?;

    match row {
        Some((id, path, trusted, kind, locator)) => Ok(ProjectResponse {
            id: ProjectId(id),
            path,
            trusted: trusted != 0,
            git_summary: None,
            enabled_issue_source: issue_source_from_row(kind, locator),
        }),
        None => Err(PersistenceError::NotFound(id.to_string())),
    }
}

/// Mark a Project as trusted for agent execution.
pub async fn trust_project(db: &Db, project_id: &str) -> Result<ProjectResponse, PersistenceError> {
    let result = sqlx::query("UPDATE projects SET trusted = 1 WHERE id = ?")
        .bind(project_id)
        .execute(db)
        .await?;

    if result.rows_affected() == 0 {
        return Err(PersistenceError::NotFound(project_id.to_string()));
    }

    get_project(db, project_id).await
}

/// Deliberately enable or switch the single Issue Source for a Project.
pub async fn enable_issue_source(
    db: &Db,
    project_id: &str,
    request: &EnableIssueSourceRequest,
) -> Result<ProjectResponse, PersistenceError> {
    validate_issue_source(request)?;
    get_project(db, project_id).await?;

    sqlx::query(
        r#"
        INSERT INTO project_issue_sources (project_id, kind, locator)
        VALUES (?, ?, ?)
        ON CONFLICT(project_id) DO UPDATE SET
            kind = excluded.kind,
            locator = excluded.locator
        "#,
    )
    .bind(project_id)
    .bind(&request.kind)
    .bind(&request.locator)
    .execute(db)
    .await?;

    get_project(db, project_id).await
}

pub async fn replace_planning_snapshot(
    db: &Db,
    project_id: &str,
    source: &IssueSource,
    issues: &[SourceIssueSnapshot],
    synced_at: &str,
) -> Result<IssueSourceSyncResponse, PersistenceError> {
    let mut tx = db.begin().await?;

    sqlx::query("DELETE FROM planning_snapshot_issues WHERE project_id = ?")
        .bind(project_id)
        .execute(&mut *tx)
        .await?;

    for issue in issues {
        let dependencies_json = serde_json::to_string(&issue.issue_dependencies)
            .map_err(|e| PersistenceError::Database(sqlx::Error::Decode(Box::new(e))))?;
        sqlx::query(
            r#"
            INSERT INTO planning_snapshot_issues (
                project_id,
                source_id,
                title,
                readiness,
                lifecycle_status,
                parent_issue,
                issue_dependencies_json,
                source_order,
                raw_text
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(project_id)
        .bind(&issue.source_id)
        .bind(&issue.title)
        .bind(&issue.readiness)
        .bind(&issue.lifecycle_status)
        .bind(&issue.parent_issue)
        .bind(dependencies_json)
        .bind(issue.source_order)
        .bind(&issue.raw_text)
        .execute(&mut *tx)
        .await?;
    }

    sqlx::query(
        r#"
        INSERT INTO issue_source_sync_status (
            project_id,
            source_kind,
            source_locator,
            last_successful_sync_at,
            last_failure
        )
        VALUES (?, ?, ?, ?, NULL)
        ON CONFLICT(project_id) DO UPDATE SET
            source_kind = excluded.source_kind,
            source_locator = excluded.source_locator,
            last_successful_sync_at = excluded.last_successful_sync_at,
            last_failure = NULL
        "#,
    )
    .bind(project_id)
    .bind(&source.kind)
    .bind(&source.locator)
    .bind(synced_at)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(IssueSourceSyncResponse {
        source: source.clone(),
        last_successful_sync_at: Some(synced_at.to_string()),
        last_failure: None,
    })
}

pub async fn record_sync_failure(
    db: &Db,
    project_id: &str,
    source: &IssueSource,
    failure: &str,
) -> Result<IssueSourceSyncResponse, PersistenceError> {
    sqlx::query(
        r#"
        INSERT INTO issue_source_sync_status (
            project_id,
            source_kind,
            source_locator,
            last_successful_sync_at,
            last_failure
        )
        VALUES (?, ?, ?, NULL, ?)
        ON CONFLICT(project_id) DO UPDATE SET
            source_kind = excluded.source_kind,
            source_locator = excluded.source_locator,
            last_failure = excluded.last_failure
        "#,
    )
    .bind(project_id)
    .bind(&source.kind)
    .bind(&source.locator)
    .bind(failure)
    .execute(db)
    .await?;

    let last_successful_sync_at = sqlx::query_as::<_, (Option<String>,)>(
        "SELECT last_successful_sync_at FROM issue_source_sync_status WHERE project_id = ?",
    )
    .bind(project_id)
    .fetch_optional(db)
    .await?
    .and_then(|row| row.0);

    Ok(IssueSourceSyncResponse {
        source: source.clone(),
        last_successful_sync_at,
        last_failure: Some(failure.to_string()),
    })
}

pub async fn get_issue_source_sync_status(
    db: &Db,
    project_id: &str,
) -> Result<IssueSourceSyncStatusResponse, PersistenceError> {
    let project = get_project(db, project_id).await?;
    let Some(source) = project.enabled_issue_source else {
        return Err(PersistenceError::InvalidIssueSource(
            "Project has no enabled Issue Source".to_string(),
        ));
    };

    let status = sqlx::query_as::<_, (Option<String>, Option<String>)>(
        r#"
        SELECT last_successful_sync_at, last_failure
        FROM issue_source_sync_status
        WHERE project_id = ?
        "#,
    )
    .bind(project_id)
    .fetch_optional(db)
    .await?;

    let (last_successful_sync_at, last_failure) = status.unwrap_or((None, None));

    Ok(IssueSourceSyncStatusResponse {
        source,
        last_successful_sync_at,
        last_failure,
    })
}

pub async fn get_planning_snapshot(
    db: &Db,
    project_id: &str,
) -> Result<PlanningSnapshotResponse, PersistenceError> {
    let status = sqlx::query_as::<_, (String, String, Option<String>, Option<String>)>(
        r#"
        SELECT source_kind, source_locator, last_successful_sync_at, last_failure
        FROM issue_source_sync_status
        WHERE project_id = ?
        "#,
    )
    .bind(project_id)
    .fetch_optional(db)
    .await?;

    let Some((kind, locator, last_successful_sync_at, last_failure)) = status else {
        get_project(db, project_id).await?;
        return Err(PersistenceError::SnapshotNotFound(project_id.to_string()));
    };

    let rows = sqlx::query_as::<_, (String, String, String, String, Option<String>, String, i64, String)>(
        r#"
        SELECT source_id, title, readiness, lifecycle_status, parent_issue, issue_dependencies_json, source_order, raw_text
        FROM planning_snapshot_issues
        WHERE project_id = ?
        ORDER BY source_order, source_id
        "#,
    )
    .bind(project_id)
    .fetch_all(db)
    .await?;

    let issues = rows
        .into_iter()
        .map(
            |(
                source_id,
                title,
                readiness,
                lifecycle_status,
                parent_issue,
                dependencies_json,
                source_order,
                raw_text,
            )| {
                let issue_dependencies =
                    serde_json::from_str(&dependencies_json).unwrap_or_default();
                SourceIssueSnapshot {
                    source_id,
                    title,
                    readiness,
                    lifecycle_status,
                    parent_issue,
                    issue_dependencies,
                    source_order,
                    raw_text,
                }
            },
        )
        .collect::<Vec<_>>();

    let ready_ids = issues
        .iter()
        .filter(|issue| issue.readiness == "ready")
        .map(|issue| issue.source_id.clone())
        .collect::<std::collections::HashSet<_>>();
    let mut non_ready = Vec::new();
    let mut blocked = Vec::new();
    let mut active = Vec::new();
    let mut completed = Vec::new();
    let mut eligible = Vec::new();

    for issue in issues {
        if issue.readiness != "ready" {
            non_ready.push(issue);
        } else if issue.lifecycle_status == "completed" {
            completed.push(issue);
        } else if matches!(
            issue.lifecycle_status.as_str(),
            "claimed" | "running" | "blocked"
        ) {
            active.push(issue);
        } else if issue
            .issue_dependencies
            .iter()
            .any(|dependency| ready_ids.contains(dependency))
        {
            blocked.push(issue);
        } else {
            eligible.push(issue);
        }
    }

    Ok(PlanningSnapshotResponse {
        source: IssueSource { kind, locator },
        last_successful_sync_at,
        last_failure,
        non_ready,
        blocked,
        active,
        completed,
        eligible,
    })
}

pub async fn create_issue_assignment(
    db: &Db,
    project_id: &str,
    source: &IssueSource,
    issue: &SourceIssueSnapshot,
    branch: &str,
) -> Result<IssueAssignmentResponse, PersistenceError> {
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
        INSERT INTO issue_assignments (
            id, project_id, source_kind, source_locator, source_id, source_title,
            source_raw_text, branch, status
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'provisional')
        "#,
    )
    .bind(&id)
    .bind(project_id)
    .bind(&source.kind)
    .bind(&source.locator)
    .bind(&issue.source_id)
    .bind(&issue.title)
    .bind(&issue.raw_text)
    .bind(branch)
    .execute(db)
    .await
    .map_err(|error| match &error {
        sqlx::Error::Database(db_error) if db_error.is_unique_violation() => {
            PersistenceError::ActiveAssignment(project_id.to_string())
        }
        _ => PersistenceError::Database(error),
    })?;

    get_issue_assignment(db, &id).await
}

pub async fn release_issue_assignment(
    db: &Db,
    assignment_id: &str,
) -> Result<(), PersistenceError> {
    sqlx::query("DELETE FROM issue_assignments WHERE id = ?")
        .bind(assignment_id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn set_assignment_worktree(
    db: &Db,
    assignment_id: &str,
    worktree_path: &str,
) -> Result<IssueAssignmentResponse, PersistenceError> {
    update_assignment(db, assignment_id, "claimed", None, Some(worktree_path)).await
}

pub async fn set_assignment_status(
    db: &Db,
    assignment_id: &str,
    status: &str,
    status_detail: Option<&str>,
) -> Result<IssueAssignmentResponse, PersistenceError> {
    update_assignment(db, assignment_id, status, status_detail, None).await
}

pub async fn set_assignment_change_proposal(
    db: &Db,
    assignment_id: &str,
    status: &str,
    url: &str,
) -> Result<IssueAssignmentResponse, PersistenceError> {
    let result = sqlx::query(
        "UPDATE issue_assignments SET change_proposal_status = ?, change_proposal_url = ? WHERE id = ?",
    )
    .bind(status)
    .bind(url)
    .bind(assignment_id)
    .execute(db)
    .await?;
    if result.rows_affected() == 0 {
        return Err(PersistenceError::AssignmentNotFound(
            assignment_id.to_string(),
        ));
    }
    get_issue_assignment(db, assignment_id).await
}

pub async fn record_initial_attempt(
    db: &Db,
    assignment_id: &str,
    process_id: Option<u32>,
    process_identity: Option<&str>,
    terminal_outcome: Option<&AssignmentTerminalOutcome>,
) -> Result<IssueAssignmentResponse, PersistenceError> {
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
        VALUES (?, ?, 'initial', ?, ?, ?)
        "#,
    )
    .bind(id)
    .bind(assignment_id)
    .bind(process_id.map(i64::from))
    .bind(process_identity)
    .bind(terminal_outcome_json)
    .execute(db)
    .await?;
    get_issue_assignment(db, assignment_id).await
}

pub async fn get_active_assignment(
    db: &Db,
    project_id: &str,
) -> Result<Option<IssueAssignmentResponse>, PersistenceError> {
    let id = sqlx::query_as::<_, (String,)>(
        "SELECT id FROM issue_assignments WHERE project_id = ? AND status != 'abandoned' LIMIT 1",
    )
    .bind(project_id)
    .fetch_optional(db)
    .await?
    .map(|row| row.0);
    match id {
        Some(id) => get_issue_assignment(db, &id).await.map(Some),
        None => Ok(None),
    }
}

async fn update_assignment(
    db: &Db,
    assignment_id: &str,
    status: &str,
    status_detail: Option<&str>,
    worktree_path: Option<&str>,
) -> Result<IssueAssignmentResponse, PersistenceError> {
    let result = if let Some(worktree_path) = worktree_path {
        sqlx::query(
            "UPDATE issue_assignments SET status = ?, status_detail = ?, worktree_path = ? WHERE id = ?",
        )
        .bind(status)
        .bind(status_detail)
        .bind(worktree_path)
        .bind(assignment_id)
        .execute(db)
        .await?
    } else {
        sqlx::query("UPDATE issue_assignments SET status = ?, status_detail = ? WHERE id = ?")
            .bind(status)
            .bind(status_detail)
            .bind(assignment_id)
            .execute(db)
            .await?
    };
    if result.rows_affected() == 0 {
        return Err(PersistenceError::AssignmentNotFound(
            assignment_id.to_string(),
        ));
    }
    get_issue_assignment(db, assignment_id).await
}

/// Look up an Issue Assignment by id, ignoring Project scope.
pub async fn get_issue_assignment_public(
    db: &Db,
    assignment_id: &str,
) -> Result<IssueAssignmentResponse, PersistenceError> {
    get_issue_assignment(db, assignment_id).await
}

/// Fetch the persisted raw text of the Source Issue an Issue Assignment was created
/// against. Used by recovery to build prompts from durable facts.
pub async fn get_assignment_source_raw_text(
    db: &Db,
    assignment_id: &str,
) -> Result<String, PersistenceError> {
    let row = sqlx::query_as::<_, (String,)>(
        "SELECT source_raw_text FROM issue_assignments WHERE id = ?",
    )
    .bind(assignment_id)
    .fetch_optional(db)
    .await?;
    row.map(|(raw,)| raw)
        .ok_or_else(|| PersistenceError::AssignmentNotFound(assignment_id.to_string()))
}

async fn get_issue_assignment(
    db: &Db,
    assignment_id: &str,
) -> Result<IssueAssignmentResponse, PersistenceError> {
    let row = sqlx::query_as::<_, (String, String, String, String, String, String, String)>(
        r#"
        SELECT id, project_id, source_id, source_title, branch, worktree_path, status
        FROM issue_assignments
        WHERE id = ?
        "#,
    )
    .bind(assignment_id)
    .fetch_optional(db)
    .await?;
    let Some((id, project_id, source_id, source_title, branch, worktree_path, status)) = row else {
        return Err(PersistenceError::AssignmentNotFound(
            assignment_id.to_string(),
        ));
    };
    let status_detail = sqlx::query_as::<_, (Option<String>,)>(
        "SELECT status_detail FROM issue_assignments WHERE id = ?",
    )
    .bind(assignment_id)
    .fetch_one(db)
    .await?
    .0;
    let (change_proposal_status, change_proposal_url) = sqlx::query_as::<
        _,
        (Option<String>, Option<String>),
    >(
        "SELECT change_proposal_status, change_proposal_url FROM issue_assignments WHERE id = ?",
    )
    .bind(assignment_id)
    .fetch_one(db)
    .await?;
    let change_proposal = change_proposal_status
        .zip(change_proposal_url)
        .map(|(status, url)| ChangeProposalResponse { status, url });
    let latest_attempt =
        sqlx::query_as::<_, (String, String, Option<i64>, Option<String>, Option<String>)>(
            r#"
        SELECT id, kind, process_id, process_identity, terminal_outcome_json
        FROM assignment_attempts
        WHERE assignment_id = ?
        ORDER BY rowid DESC
        LIMIT 1
        "#,
        )
        .bind(assignment_id)
        .fetch_optional(db)
        .await?
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
        );
    Ok(IssueAssignmentResponse {
        id,
        project_id: ProjectId(project_id),
        source_id,
        source_title,
        branch,
        worktree_path,
        status,
        status_detail,
        change_proposal,
        latest_attempt,
    })
}

fn validate_issue_source(request: &EnableIssueSourceRequest) -> Result<(), PersistenceError> {
    let supported_kind = matches!(request.kind.as_str(), "github" | "local_markdown");
    if !supported_kind || request.locator.trim().is_empty() {
        return Err(PersistenceError::InvalidIssueSource(format!(
            "{}:{}",
            request.kind, request.locator
        )));
    }

    Ok(())
}

fn issue_source_from_row(kind: Option<String>, locator: Option<String>) -> Option<IssueSource> {
    Some(IssueSource {
        kind: kind?,
        locator: locator?,
    })
}

/// Idempotently seed a development Project at the given path.
pub async fn seed_dev_project(
    db: &Db,
    dev_path: &str,
) -> Result<ProjectResponse, PersistenceError> {
    let normalized_path = normalize_project_path(dev_path)?;

    // Check if already seeded
    let existing = sqlx::query_as::<_, (String, String, i64)>(
        "SELECT id, path, trusted FROM projects WHERE path = ?",
    )
    .bind(&normalized_path)
    .fetch_optional(db)
    .await?;

    if let Some((id, path, trusted)) = existing {
        return Ok(ProjectResponse {
            id: ProjectId(id),
            path,
            trusted: trusted != 0,
            git_summary: None,
            enabled_issue_source: None,
        });
    }

    let id = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO projects (id, path, trusted) VALUES (?, ?, 0)")
        .bind(&id)
        .bind(&normalized_path)
        .execute(db)
        .await?;

    Ok(ProjectResponse {
        id: ProjectId(id),
        path: normalized_path,
        trusted: false,
        git_summary: None,
        enabled_issue_source: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_db() -> Db {
        let db = connect_in_memory().await.unwrap();
        migrate(&db).await.unwrap();
        db
    }

    #[tokio::test]
    async fn fresh_database_has_no_projects() {
        let db = setup_db().await;
        let projects = list_projects(&db).await.unwrap();
        assert!(projects.is_empty());
    }

    #[tokio::test]
    async fn create_project_returns_uuid_id() {
        let db = setup_db().await;
        let request = CreateProjectRequest {
            path: "/tmp".to_string(),
        };
        let project = create_project(&db, &request).await.unwrap();
        Uuid::parse_str(&project.id.0).expect("Project ID should be a valid UUID");
        assert_eq!(project.path, "/tmp");
        assert_eq!(project.trusted, false);
    }

    #[tokio::test]
    async fn create_project_rejects_nonexistent_path() {
        let db = setup_db().await;
        let request = CreateProjectRequest {
            path: "/nonexistent/path/that/does/not/exist".to_string(),
        };
        let result = create_project(&db, &request).await;
        assert!(matches!(result, Err(PersistenceError::InvalidPath(_))));
    }

    #[tokio::test]
    async fn create_project_rejects_file_path() {
        let db = setup_db().await;
        let request = CreateProjectRequest {
            path: "/etc/hostname".to_string(),
        };
        let result = create_project(&db, &request).await;
        assert!(matches!(result, Err(PersistenceError::InvalidPath(_))));
    }

    #[tokio::test]
    async fn create_project_rejects_duplicate_path() {
        let db = setup_db().await;
        let request = CreateProjectRequest {
            path: "/tmp".to_string(),
        };
        create_project(&db, &request).await.unwrap();
        let result = create_project(&db, &request).await;
        assert!(matches!(result, Err(PersistenceError::Duplicate(_))));
    }

    #[tokio::test]
    async fn list_projects_returns_created_projects() {
        let db = setup_db().await;
        let req = CreateProjectRequest {
            path: "/tmp".to_string(),
        };
        let created = create_project(&db, &req).await.unwrap();
        let projects = list_projects(&db).await.unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].id, created.id);
        assert_eq!(projects[0].path, "/tmp");
        assert_eq!(projects[0].trusted, false);
    }

    #[tokio::test]
    async fn get_project_by_id() {
        let db = setup_db().await;
        let req = CreateProjectRequest {
            path: "/tmp".to_string(),
        };
        let created = create_project(&db, &req).await.unwrap();
        let fetched = get_project(&db, &created.id.0).await.unwrap();
        assert_eq!(fetched, created);
    }

    #[tokio::test]
    async fn get_project_not_found() {
        let db = setup_db().await;
        let result = get_project(&db, "nonexistent-id").await;
        assert!(matches!(result, Err(PersistenceError::NotFound(_))));
    }

    #[tokio::test]
    async fn seed_dev_project_is_idempotent() {
        let db = setup_db().await;
        let first = seed_dev_project(&db, "/tmp").await.unwrap();
        let second = seed_dev_project(&db, "/tmp").await.unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(first.path, second.path);
        let projects = list_projects(&db).await.unwrap();
        assert_eq!(projects.len(), 1);
    }

    #[tokio::test]
    async fn seed_dev_project_rejects_invalid_path() {
        let db = setup_db().await;
        let result = seed_dev_project(&db, "/nonexistent/path").await;
        assert!(matches!(result, Err(PersistenceError::InvalidPath(_))));
    }

    #[tokio::test]
    async fn create_project_defaults_to_untrusted() {
        let db = setup_db().await;
        let req = CreateProjectRequest {
            path: "/tmp".to_string(),
        };
        let project = create_project(&db, &req).await.unwrap();
        assert_eq!(project.trusted, false);
        let fetched = get_project(&db, &project.id.0).await.unwrap();
        assert_eq!(fetched.trusted, false);
    }

    #[tokio::test]
    async fn trust_project_sets_trusted_true() {
        let db = setup_db().await;
        let req = CreateProjectRequest {
            path: "/tmp".to_string(),
        };
        let created = create_project(&db, &req).await.unwrap();
        assert_eq!(created.trusted, false);

        let updated = trust_project(&db, &created.id.0).await.unwrap();
        assert_eq!(updated.trusted, true);

        let fetched = get_project(&db, &created.id.0).await.unwrap();
        assert_eq!(fetched.trusted, true);
    }

    #[tokio::test]
    async fn trust_project_is_idempotent() {
        let db = setup_db().await;
        let req = CreateProjectRequest {
            path: "/tmp".to_string(),
        };
        let created = create_project(&db, &req).await.unwrap();
        trust_project(&db, &created.id.0).await.unwrap();
        let second = trust_project(&db, &created.id.0).await.unwrap();
        assert_eq!(second.trusted, true);
    }

    #[tokio::test]
    async fn trust_project_not_found() {
        let db = setup_db().await;
        let result = trust_project(&db, "nonexistent-id").await;
        assert!(matches!(result, Err(PersistenceError::NotFound(_))));
    }

    #[tokio::test]
    async fn get_planning_snapshot_lifecycle_status_precedes_dependencies() {
        let db = setup_db().await;
        let project = create_project(
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

        let issues = vec![SourceIssueSnapshot {
            source_id: "claimed-with-deps".to_string(),
            title: "claimed".to_string(),
            readiness: "ready".to_string(),
            lifecycle_status: "claimed".to_string(),
            parent_issue: None,
            issue_dependencies: vec!["some-dep".to_string()],
            source_order: 1,
            raw_text: "raw".to_string(),
        }];

        replace_planning_snapshot(&db, &project.id.0, &source, &issues, "unix:1")
            .await
            .unwrap();

        let snapshot = get_planning_snapshot(&db, &project.id.0).await.unwrap();
        assert_eq!(snapshot.active.len(), 1);
        assert_eq!(snapshot.active[0].source_id, "claimed-with-deps");
        assert!(snapshot.blocked.is_empty());
        assert!(snapshot.eligible.is_empty());
    }

    #[tokio::test]
    async fn get_planning_snapshot_buckets_by_lifecycle_status() {
        let db = setup_db().await;
        let project = create_project(
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

        let issues = vec![
            make_issue("ready-ready", "ready", "ready"),
            make_issue("ready-claimed", "ready", "claimed"),
            make_issue("ready-running", "ready", "running"),
            make_issue("ready-blocked", "ready", "blocked"),
            make_issue("ready-completed", "ready", "completed"),
            make_issue("not-ready", "not-ready", "ready"),
        ];

        replace_planning_snapshot(&db, &project.id.0, &source, &issues, "unix:1")
            .await
            .unwrap();

        let snapshot = get_planning_snapshot(&db, &project.id.0).await.unwrap();
        assert_eq!(snapshot.non_ready.len(), 1);
        assert_eq!(snapshot.non_ready[0].source_id, "not-ready");

        assert_eq!(snapshot.active.len(), 3);
        let active_ids: Vec<_> = snapshot
            .active
            .iter()
            .map(|i| i.source_id.clone())
            .collect();
        assert!(active_ids.contains(&"ready-claimed".to_string()));
        assert!(active_ids.contains(&"ready-running".to_string()));
        assert!(active_ids.contains(&"ready-blocked".to_string()));

        assert_eq!(snapshot.completed.len(), 1);
        assert_eq!(snapshot.completed[0].source_id, "ready-completed");

        assert_eq!(snapshot.eligible.len(), 1);
        assert_eq!(snapshot.eligible[0].source_id, "ready-ready");
    }

    fn make_issue(source_id: &str, readiness: &str, lifecycle_status: &str) -> SourceIssueSnapshot {
        SourceIssueSnapshot {
            source_id: source_id.to_string(),
            title: source_id.to_string(),
            readiness: readiness.to_string(),
            lifecycle_status: lifecycle_status.to_string(),
            parent_issue: None,
            issue_dependencies: vec![],
            source_order: 1,
            raw_text: "raw".to_string(),
        }
    }
}
