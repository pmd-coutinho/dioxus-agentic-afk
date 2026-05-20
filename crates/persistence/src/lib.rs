//! Persistence boundary for the Local Control Plane.
//!
//! Provides SQLite-backed storage for Projects.

use agentic_afk_contracts::{
    CreateProjectRequest, EnableIssueSourceRequest, IssueSource, IssueSourceSyncResponse,
    PlanningSnapshotResponse, ProjectId, ProjectResponse, SourceIssueSnapshot,
};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Pool, Sqlite};
use std::path::Path;
use std::str::FromStr;
use uuid::Uuid;

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

    sqlx::query("INSERT INTO projects (id, path) VALUES (?, ?)")
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
        git_summary: None,
        enabled_issue_source: None,
    })
}

/// List all Projects.
pub async fn list_projects(db: &Db) -> Result<Vec<ProjectResponse>, PersistenceError> {
    let rows = sqlx::query_as::<_, (String, String, Option<String>, Option<String>)>(
        r#"
        SELECT projects.id, projects.path, project_issue_sources.kind, project_issue_sources.locator
        FROM projects
        LEFT JOIN project_issue_sources ON project_issue_sources.project_id = projects.id
        ORDER BY projects.path
        "#,
    )
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(id, path, kind, locator)| ProjectResponse {
            id: ProjectId(id),
            path,
            git_summary: None,
            enabled_issue_source: issue_source_from_row(kind, locator),
        })
        .collect())
}

/// Get a single Project by ID.
pub async fn get_project(db: &Db, id: &str) -> Result<ProjectResponse, PersistenceError> {
    let row = sqlx::query_as::<_, (String, String, Option<String>, Option<String>)>(
        r#"
        SELECT projects.id, projects.path, project_issue_sources.kind, project_issue_sources.locator
        FROM projects
        LEFT JOIN project_issue_sources ON project_issue_sources.project_id = projects.id
        WHERE projects.id = ?
        "#,
    )
    .bind(id)
    .fetch_optional(db)
    .await?;

    match row {
        Some((id, path, kind, locator)) => Ok(ProjectResponse {
            id: ProjectId(id),
            path,
            git_summary: None,
            enabled_issue_source: issue_source_from_row(kind, locator),
        }),
        None => Err(PersistenceError::NotFound(id.to_string())),
    }
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
                parent_issue,
                issue_dependencies_json,
                source_order,
                raw_text
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(project_id)
        .bind(&issue.source_id)
        .bind(&issue.title)
        .bind(&issue.readiness)
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

    let rows = sqlx::query_as::<_, (String, String, String, Option<String>, String, i64, String)>(
        r#"
        SELECT source_id, title, readiness, parent_issue, issue_dependencies_json, source_order, raw_text
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
    let mut eligible = Vec::new();

    for issue in issues {
        if issue.readiness != "ready" {
            non_ready.push(issue);
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
        eligible,
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
    let existing =
        sqlx::query_as::<_, (String, String)>("SELECT id, path FROM projects WHERE path = ?")
            .bind(&normalized_path)
            .fetch_optional(db)
            .await?;

    if let Some((id, path)) = existing {
        return Ok(ProjectResponse {
            id: ProjectId(id),
            path,
            git_summary: None,
            enabled_issue_source: None,
        });
    }

    let id = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO projects (id, path) VALUES (?, ?)")
        .bind(&id)
        .bind(&normalized_path)
        .execute(db)
        .await?;

    Ok(ProjectResponse {
        id: ProjectId(id),
        path: normalized_path,
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
}
