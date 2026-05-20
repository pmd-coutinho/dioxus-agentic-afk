//! Persistence boundary for the Local Control Plane.
//!
//! Provides SQLite-backed storage for Projects.

use agentic_afk_contracts::{CreateProjectRequest, ProjectId, ProjectResponse};
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
    })
}

/// List all Projects.
pub async fn list_projects(db: &Db) -> Result<Vec<ProjectResponse>, PersistenceError> {
    let rows = sqlx::query_as::<_, (String, String)>("SELECT id, path FROM projects ORDER BY path")
        .fetch_all(db)
        .await?;

    Ok(rows
        .into_iter()
        .map(|(id, path)| ProjectResponse {
            id: ProjectId(id),
            path,
        })
        .collect())
}

/// Get a single Project by ID.
pub async fn get_project(db: &Db, id: &str) -> Result<ProjectResponse, PersistenceError> {
    let row = sqlx::query_as::<_, (String, String)>("SELECT id, path FROM projects WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await?;

    match row {
        Some((id, path)) => Ok(ProjectResponse {
            id: ProjectId(id),
            path,
        }),
        None => Err(PersistenceError::NotFound(id.to_string())),
    }
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
