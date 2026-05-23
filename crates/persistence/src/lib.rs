//! Persistence boundary for the Local Control Plane.
//!
//! Provides SQLite-backed storage for Projects.

use agentic_afk_contracts::{
    AssignmentAttemptResponse, AssignmentTerminalOutcome, BlockReason, BlockReasonResponse,
    CreateProjectRequest, EnableIssueSourceRequest, IssueAssignmentResponse, IssueSource,
    IssueSourceSyncResponse, IssueSourceSyncStatusResponse, ProjectId, ProjectResponse,
    SourceIssueSnapshot,
};
pub use agentic_afk_planning_snapshot::RawPlanningSnapshot;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Pool, Sqlite};
use std::path::Path;
use std::str::FromStr;
use uuid::Uuid;

mod activity;
mod plan_run;

pub use activity::{
    PROJECT_ACTIVITY_DETAIL_MAX_BYTES, ProjectActivityEntry, list_project_activity,
    record_project_activity,
};
pub use plan_run::{
    create_plan_run, finish_plan_run, get_active_plan_run, get_plan_run,
    get_project_execution_config, list_assignment_phase_outputs, list_recent_plan_runs,
    record_assignment_phase_output, record_assignment_phase_output_typed,
    record_plan_run_phase_output, record_plan_run_phase_output_typed,
    set_project_execution_config,
};

// Re-export new Plan Run assignment helpers at the crate root for
// convenience; defined further down in this module.

/// Public re-export of the internal assignment lookup, used by sibling modules and external callers.
pub async fn get_issue_assignment_public(
    db: &Db,
    assignment_id: &str,
) -> Result<IssueAssignmentResponse, PersistenceError> {
    get_issue_assignment(db, assignment_id).await
}

pub async fn get_project_assignment(
    db: &Db,
    project_id: &str,
    assignment_id: &str,
) -> Result<IssueAssignmentResponse, PersistenceError> {
    let assignment = get_issue_assignment_public(db, assignment_id).await?;
    if assignment.project_id.0 != project_id {
        return Err(PersistenceError::AssignmentNotFound(
            assignment_id.to_string(),
        ));
    }
    Ok(assignment)
}

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
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("Phase Output outcome {outcome} does not pair with {body_phase} body")]
    PhaseOutputMismatch {
        body_phase: &'static str,
        outcome: String,
    },
}

/// Hard ceiling on the serialized JSON size of a single Phase Output body.
/// Bodies that serialize larger are replaced at the write seam with a
/// `truncated_at: <bytes>` marker (ADR-0038) so push stderr or verification
/// log dumps cannot push runaway rows through to the Dashboard.
pub const PHASE_OUTPUT_BODY_MAX_BYTES: usize = 64 * 1024;

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
) -> Result<RawPlanningSnapshot, PersistenceError> {
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

    Ok(RawPlanningSnapshot {
        source: IssueSource { kind, locator },
        last_successful_sync_at,
        last_failure,
        issues,
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

/// Public accessor for fetching a single Issue Assignment by id.
pub async fn get_assignment(
    db: &Db,
    assignment_id: &str,
) -> Result<IssueAssignmentResponse, PersistenceError> {
    get_issue_assignment(db, assignment_id).await
}

/// Fetch the persisted Source Issue raw text for an assignment, used when
/// building Plan Run implementation prompts that need the original brief verbatim.
pub async fn get_assignment_source_raw_text(
    db: &Db,
    assignment_id: &str,
) -> Result<String, PersistenceError> {
    let row = sqlx::query_as::<_, (String,)>(
        "SELECT source_raw_text FROM issue_assignments WHERE id = ?",
    )
    .bind(assignment_id)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| PersistenceError::AssignmentNotFound(assignment_id.to_string()))?;
    Ok(row.0)
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

pub(crate) async fn get_issue_assignment(
    db: &Db,
    assignment_id: &str,
) -> Result<IssueAssignmentResponse, PersistenceError> {
    let row = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
            Option<String>,
            Option<String>,
        ),
    >(
        r#"
        SELECT id, project_id, source_id, source_title, branch, worktree_path, status,
               status_detail, plan_run_id, selection_summary,
               review_rejection_count, block_reason, block_reason_kind
        FROM issue_assignments
        WHERE id = ?
        "#,
    )
    .bind(assignment_id)
    .fetch_optional(db)
    .await?;
    let Some((
        id,
        project_id,
        source_id,
        source_title,
        branch,
        worktree_path,
        status,
        status_detail,
        plan_run_id,
        selection_summary,
        review_rejection_count,
        block_reason_detail,
        block_reason_kind,
    )) = row
    else {
        return Err(PersistenceError::AssignmentNotFound(
            assignment_id.to_string(),
        ));
    };
    let block_reason = block_reason_kind
        .as_deref()
        .and_then(BlockReason::from_wire)
        .map(|kind| BlockReasonResponse {
            kind,
            detail: block_reason_detail.clone(),
        });
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
    let phase_outputs = list_assignment_phase_outputs(db, &id).await?;
    Ok(IssueAssignmentResponse {
        id,
        project_id: ProjectId(project_id),
        source_id,
        source_title,
        branch,
        worktree_path,
        status,
        status_detail,
        latest_attempt,
        plan_run_id,
        selection_summary,
        phase_outputs,
        review_rejection_count,
        block_reason,
    })
}

/// Increment the review rejection counter for one Issue Assignment, then
/// return the updated count.
pub async fn increment_review_rejection(
    db: &Db,
    assignment_id: &str,
) -> Result<i64, PersistenceError> {
    let result =
        sqlx::query("UPDATE issue_assignments SET review_rejection_count = review_rejection_count + 1 WHERE id = ?")
            .bind(assignment_id)
            .execute(db)
            .await?;
    if result.rows_affected() == 0 {
        return Err(PersistenceError::AssignmentNotFound(
            assignment_id.to_string(),
        ));
    }
    let row = sqlx::query_as::<_, (i64,)>(
        "SELECT review_rejection_count FROM issue_assignments WHERE id = ?",
    )
    .bind(assignment_id)
    .fetch_one(db)
    .await?;
    Ok(row.0)
}

/// Move an Issue Assignment into the coarse blocked lifecycle state with a
/// typed **Block Reason** (ADR-0038). `kind` populates
/// `block_reason_kind` for taxonomy-driven Dashboard affordances; `detail`
/// keeps the existing freeform text used as `status_detail` and as the
/// human-readable specifics under the typed badge. Freeform-only blocking
/// is no longer supported.
pub async fn record_blocked_with_kind(
    db: &Db,
    assignment_id: &str,
    kind: BlockReason,
    detail: Option<&str>,
) -> Result<IssueAssignmentResponse, PersistenceError> {
    let result = sqlx::query(
        "UPDATE issue_assignments SET status = 'blocked', status_detail = ?, block_reason = ?, block_reason_kind = ? WHERE id = ?",
    )
    .bind(detail)
    .bind(detail)
    .bind(kind.as_wire())
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

/// Look up the most recently created blocked **Issue Assignment** for
/// one `(project, source_id)` pair, if any. Returns `None` when no
/// blocked row remains (Plan Run cleanup may have deleted the dead
/// Issue Assignment row, or the operator may be re-enabling a Source
/// Issue that never blocked). Used by the Source-Issue-keyed re-enable
/// use case (ADR-0038) which must locate the latest blocked row, not
/// the operator-supplied Assignment id.
pub async fn latest_blocked_assignment_for_source(
    db: &Db,
    project_id: &str,
    source_id: &str,
) -> Result<Option<IssueAssignmentResponse>, PersistenceError> {
    let row = sqlx::query_as::<_, (String,)>(
        r#"
        SELECT id
        FROM issue_assignments
        WHERE project_id = ? AND source_id = ? AND status = 'blocked'
        ORDER BY rowid DESC
        LIMIT 1
        "#,
    )
    .bind(project_id)
    .bind(source_id)
    .fetch_optional(db)
    .await?;
    match row {
        Some((id,)) => Ok(Some(get_issue_assignment(db, &id).await?)),
        None => Ok(None),
    }
}

/// Update the Lifecycle Status of one Source Issue row inside the local
/// **Planning Snapshot** mirror. Used by the Source-Issue-keyed
/// re-enable use case (ADR-0038) to flip a `blocked` mirror row to
/// `ready` so the next **Plan Run** Planning Snapshot buckets the
/// Source Issue as `eligible` rather than `active` without waiting for
/// a fresh sync. A no-op when no snapshot row exists.
pub async fn set_planning_snapshot_lifecycle(
    db: &Db,
    project_id: &str,
    source_id: &str,
    lifecycle_status: &str,
) -> Result<(), PersistenceError> {
    sqlx::query(
        "UPDATE planning_snapshot_issues SET lifecycle_status = ? WHERE project_id = ? AND source_id = ?",
    )
    .bind(lifecycle_status)
    .bind(project_id)
    .bind(source_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Clear the blocked lifecycle of an Issue Assignment without redefining
/// `ready-for-agent` readiness. Resets the review rejection counter so a
/// later Plan Run may pick up the Source Issue again. Returns the updated
/// assignment.
pub async fn re_enable_blocked_assignment(
    db: &Db,
    assignment_id: &str,
) -> Result<IssueAssignmentResponse, PersistenceError> {
    let current = get_issue_assignment(db, assignment_id).await?;
    if current.status != "blocked" {
        return Err(PersistenceError::InvalidIssueSource(format!(
            "Issue Assignment {assignment_id} is not blocked (status={})",
            current.status
        )));
    }
    let result = sqlx::query(
        "UPDATE issue_assignments SET status = 're_enabled', status_detail = NULL, block_reason = NULL, block_reason_kind = NULL, review_rejection_count = 0 WHERE id = ?",
    )
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

/// Insert a provisional Issue Assignment nested under a Plan Run.
///
/// The assignment starts in `provisional` status with no worktree path; the
/// caller transitions it to `claimed` after the Assignment Worktree is
/// ready (ADR-0028).
pub async fn create_plan_run_assignment(
    db: &Db,
    plan_run_id: &str,
    project_id: &str,
    source: &IssueSource,
    issue: &SourceIssueSnapshot,
    branch: &str,
    selection_summary: &str,
) -> Result<IssueAssignmentResponse, PersistenceError> {
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
        INSERT INTO issue_assignments (
            id, project_id, source_kind, source_locator, source_id, source_title,
            source_raw_text, branch, status, plan_run_id, selection_summary
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'provisional', ?, ?)
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
    .bind(plan_run_id)
    .bind(selection_summary)
    .execute(db)
    .await
    .map_err(|error| match &error {
        sqlx::Error::Database(db_error) if db_error.is_unique_violation() => {
            PersistenceError::ActiveAssignment(plan_run_id.to_string())
        }
        _ => PersistenceError::Database(error),
    })?;

    get_issue_assignment(db, &id).await
}

/// List Issue Assignments nested under a Plan Run, oldest first.
pub async fn list_plan_run_assignments(
    db: &Db,
    plan_run_id: &str,
) -> Result<Vec<IssueAssignmentResponse>, PersistenceError> {
    let rows = sqlx::query_as::<_, (String,)>(
        r#"
        SELECT id FROM issue_assignments
        WHERE plan_run_id = ?
        ORDER BY rowid ASC
        "#,
    )
    .bind(plan_run_id)
    .fetch_all(db)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for (id,) in rows {
        out.push(get_issue_assignment(db, &id).await?);
    }
    Ok(out)
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
        assert_eq!(snapshot.issues.len(), 1);
        assert_eq!(snapshot.issues[0].source_id, "claimed-with-deps");
        assert_eq!(snapshot.issues[0].lifecycle_status, "claimed");
        assert_eq!(snapshot.issues[0].issue_dependencies, vec!["some-dep"]);
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
        // Persistence returns raw issues exactly as written; bucketing is the
        // job of `agentic_afk_planning_snapshot::normalize` and is covered by
        // that crate's tests.
        let ids: Vec<_> = snapshot
            .issues
            .iter()
            .map(|i| i.source_id.clone())
            .collect();
        assert_eq!(ids.len(), 6);
        assert!(ids.contains(&"ready-ready".to_string()));
        assert!(ids.contains(&"ready-claimed".to_string()));
        assert!(ids.contains(&"ready-running".to_string()));
        assert!(ids.contains(&"ready-blocked".to_string()));
        assert!(ids.contains(&"ready-completed".to_string()));
        assert!(ids.contains(&"not-ready".to_string()));
    }

    #[tokio::test]
    async fn record_blocked_with_kind_persists_typed_block_reason() {
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
            kind: "github".into(),
            locator: "owner/repo".into(),
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
        let issue = make_issue("42", "ready", "ready");
        let assignment = create_issue_assignment(&db, &project.id.0, &source, &issue, "agent/42")
            .await
            .unwrap();

        let updated = record_blocked_with_kind(
            &db,
            &assignment.id,
            BlockReason::ReviewRetryLimitExhausted,
            Some("Review Loop exhausted: 3 rejection(s)"),
        )
        .await
        .unwrap();

        assert_eq!(updated.status, "blocked");
        let reason = updated
            .block_reason
            .as_ref()
            .expect("typed block reason recorded");
        assert_eq!(reason.kind, BlockReason::ReviewRetryLimitExhausted);
        assert_eq!(
            reason.detail.as_deref(),
            Some("Review Loop exhausted: 3 rejection(s)")
        );

        // Re-enable clears both kind and detail.
        let cleared = re_enable_blocked_assignment(&db, &assignment.id)
            .await
            .unwrap();
        assert!(cleared.block_reason.is_none());
    }

    #[tokio::test]
    async fn record_blocked_with_kind_supports_merge_phase_failed() {
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
            kind: "github".into(),
            locator: "owner/repo".into(),
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
        let issue = make_issue("99", "ready", "ready");
        let assignment = create_issue_assignment(&db, &project.id.0, &source, &issue, "agent/99")
            .await
            .unwrap();

        let updated = record_blocked_with_kind(
            &db,
            &assignment.id,
            BlockReason::MergePhaseFailed,
            Some("unresolvable merge conflict requires human review"),
        )
        .await
        .unwrap();
        let reason = updated
            .block_reason
            .as_ref()
            .expect("typed block reason recorded");
        assert_eq!(reason.kind, BlockReason::MergePhaseFailed);
        assert!(
            reason
                .detail
                .as_deref()
                .is_some_and(|d| d.contains("merge conflict"))
        );
    }

    #[tokio::test]
    async fn write_seam_rejects_failed_outcome_with_non_failed_body() {
        use agentic_afk_contracts::PhaseOutputBody;
        let db = setup_db().await;
        let project = create_project(
            &db,
            &CreateProjectRequest {
                path: "/tmp".to_string(),
            },
        )
        .await
        .unwrap();
        let plan_run = create_plan_run(&db, &project.id.0, "main", "deadbeef")
            .await
            .unwrap();
        // Implementation body paired with outcome="failed" must be rejected
        // at the single write-seam chokepoint.
        let body = PhaseOutputBody::Implementation {
            commits: vec![],
            verification: vec![],
            gaps: vec![],
            summary: String::new(),
        };
        let result =
            record_plan_run_phase_output_typed(&db, &plan_run.id, "planning", "failed", &body).await;
        assert!(
            matches!(result, Err(PersistenceError::PhaseOutputMismatch { .. })),
            "expected PhaseOutputMismatch, got {result:?}"
        );
    }

    #[tokio::test]
    async fn write_seam_rejects_failed_body_with_non_failed_outcome() {
        use agentic_afk_contracts::PhaseOutputBody;
        let db = setup_db().await;
        let project = create_project(
            &db,
            &CreateProjectRequest {
                path: "/tmp".to_string(),
            },
        )
        .await
        .unwrap();
        let plan_run = create_plan_run(&db, &project.id.0, "main", "deadbeef")
            .await
            .unwrap();
        let body = PhaseOutputBody::Failed {
            error: "boom".to_string(),
            problem_type: None,
        };
        let result =
            record_plan_run_phase_output_typed(&db, &plan_run.id, "review", "approved", &body).await;
        assert!(
            matches!(result, Err(PersistenceError::PhaseOutputMismatch { .. })),
            "expected PhaseOutputMismatch, got {result:?}"
        );
    }

    #[tokio::test]
    async fn write_seam_truncates_body_over_64kb() {
        use agentic_afk_contracts::PhaseOutputBody;
        let db = setup_db().await;
        let project = create_project(
            &db,
            &CreateProjectRequest {
                path: "/tmp".to_string(),
            },
        )
        .await
        .unwrap();
        let plan_run = create_plan_run(&db, &project.id.0, "main", "deadbeef")
            .await
            .unwrap();
        // Build a Failed body whose serialized JSON exceeds 64 KB.
        let huge_error: String = "x".repeat(65 * 1024);
        let body = PhaseOutputBody::Failed {
            error: huge_error.clone(),
            problem_type: None,
        };
        let stored = record_plan_run_phase_output_typed(&db, &plan_run.id, "planning", "failed", &body)
            .await
            .unwrap();
        // The stored body must carry a `truncated_at: <bytes>` marker.
        let marker = stored
            .body_json
            .get("truncated_at")
            .and_then(serde_json::Value::as_u64);
        assert!(marker.is_some(), "body_json missing truncated_at marker: {stored:?}");
        let bytes = marker.unwrap();
        // The marker records the original serialized size (the byte count
        // that triggered truncation), which must exceed the ceiling.
        assert!(bytes > PHASE_OUTPUT_BODY_MAX_BYTES as u64, "marker bytes {bytes}");
        // Round-trip via list to verify it persists on disk truncated.
        let reloaded = get_plan_run(&db, &plan_run.id).await.unwrap();
        let row = reloaded
            .phase_outputs
            .iter()
            .find(|p| p.outcome == "failed")
            .expect("failed row");
        assert_eq!(
            row.body_json.get("truncated_at"),
            Some(&serde_json::Value::from(bytes))
        );
    }

    #[tokio::test]
    async fn write_seam_accepts_failed_failed_pair() {
        use agentic_afk_contracts::PhaseOutputBody;
        let db = setup_db().await;
        let project = create_project(
            &db,
            &CreateProjectRequest {
                path: "/tmp".to_string(),
            },
        )
        .await
        .unwrap();
        let plan_run = create_plan_run(&db, &project.id.0, "main", "deadbeef")
            .await
            .unwrap();
        let body = PhaseOutputBody::Failed {
            error: "planner unparseable".to_string(),
            problem_type: Some("urn:agentic-afk:planning-output-unparseable".to_string()),
        };
        let stored = record_plan_run_phase_output_typed(&db, &plan_run.id, "planning", "failed", &body)
            .await
            .unwrap();
        // The SQL `phase` column tracks the originating phase identity
        // (per ADR-0038 the outer `phase` column stays as `planning` /
        // `implementation` / `review` / `merge` / `push`). The body JSON's
        // `phase` tag carries the typed body variant ("failed" here).
        assert_eq!(stored.phase, "planning");
        assert_eq!(stored.outcome, "failed");
        assert_eq!(
            stored.body_json.get("phase").and_then(|v| v.as_str()),
            Some("failed")
        );
        assert_eq!(
            stored.body_json.get("error").and_then(|v| v.as_str()),
            Some("planner unparseable")
        );
    }

    #[tokio::test]
    async fn write_seam_accepts_implementation_with_ready_for_review() {
        use agentic_afk_contracts::PhaseOutputBody;
        let db = setup_db().await;
        let project = create_project(
            &db,
            &CreateProjectRequest {
                path: "/tmp".to_string(),
            },
        )
        .await
        .unwrap();
        let plan_run = create_plan_run(&db, &project.id.0, "main", "deadbeef")
            .await
            .unwrap();
        let body = PhaseOutputBody::Implementation {
            commits: vec!["abc".into()],
            verification: vec!["cargo test".into()],
            gaps: vec![],
            summary: "shipped".into(),
        };
        let stored = record_plan_run_phase_output_typed(
            &db,
            &plan_run.id,
            "implementation",
            "ready_for_review",
            &body,
        )
        .await
        .unwrap();
        assert_eq!(stored.phase, "implementation");
        assert_eq!(stored.outcome, "ready_for_review");
        assert_eq!(
            stored.body_json.get("phase").and_then(|v| v.as_str()),
            Some("implementation")
        );
        assert_eq!(
            stored.body_json.get("summary").and_then(|v| v.as_str()),
            Some("shipped")
        );
    }

    #[tokio::test]
    async fn write_seam_rejects_implementation_with_non_ready_outcome() {
        use agentic_afk_contracts::PhaseOutputBody;
        let db = setup_db().await;
        let project = create_project(
            &db,
            &CreateProjectRequest {
                path: "/tmp".to_string(),
            },
        )
        .await
        .unwrap();
        let plan_run = create_plan_run(&db, &project.id.0, "main", "deadbeef")
            .await
            .unwrap();
        let body = PhaseOutputBody::Implementation {
            commits: vec![],
            verification: vec![],
            gaps: vec![],
            summary: "x".into(),
        };
        // Implementation body must only pair with `ready_for_review`. The
        // legitimate failure path uses the Failed variant.
        let result = record_plan_run_phase_output_typed(
            &db,
            &plan_run.id,
            "implementation",
            "approved",
            &body,
        )
        .await;
        assert!(
            matches!(result, Err(PersistenceError::PhaseOutputMismatch { .. })),
            "expected PhaseOutputMismatch, got {result:?}"
        );
    }

    #[tokio::test]
    async fn write_seam_accepts_review_with_approved_and_rejected() {
        use agentic_afk_contracts::PhaseOutputBody;
        let db = setup_db().await;
        let project = create_project(
            &db,
            &CreateProjectRequest {
                path: "/tmp".to_string(),
            },
        )
        .await
        .unwrap();
        let plan_run = create_plan_run(&db, &project.id.0, "main", "deadbeef")
            .await
            .unwrap();
        let body = PhaseOutputBody::Review {
            findings: vec![],
            verification: vec![],
            gaps: vec![],
            summary: "lgtm".into(),
        };
        let stored =
            record_plan_run_phase_output_typed(&db, &plan_run.id, "review", "approved", &body)
                .await
                .unwrap();
        assert_eq!(stored.outcome, "approved");

        let body = PhaseOutputBody::Review {
            findings: vec![agentic_afk_contracts::ReviewFinding {
                location: Some("src/x.rs:1".into()),
                message: "missing test".into(),
            }],
            verification: vec![],
            gaps: vec![],
            summary: "needs more".into(),
        };
        let stored =
            record_plan_run_phase_output_typed(&db, &plan_run.id, "review", "rejected", &body)
                .await
                .unwrap();
        assert_eq!(stored.outcome, "rejected");
        let findings = stored.body_json.get("findings").and_then(|v| v.as_array());
        assert!(findings.is_some_and(|arr| arr.len() == 1));
    }

    #[tokio::test]
    async fn write_seam_rejects_review_with_non_review_outcome() {
        use agentic_afk_contracts::PhaseOutputBody;
        let db = setup_db().await;
        let project = create_project(
            &db,
            &CreateProjectRequest {
                path: "/tmp".to_string(),
            },
        )
        .await
        .unwrap();
        let plan_run = create_plan_run(&db, &project.id.0, "main", "deadbeef")
            .await
            .unwrap();
        let body = PhaseOutputBody::Review {
            findings: vec![],
            verification: vec![],
            gaps: vec![],
            summary: "x".into(),
        };
        let result =
            record_plan_run_phase_output_typed(&db, &plan_run.id, "review", "ready_for_review", &body)
                .await;
        assert!(
            matches!(result, Err(PersistenceError::PhaseOutputMismatch { .. })),
            "expected PhaseOutputMismatch, got {result:?}"
        );
    }

    #[tokio::test]
    async fn write_seam_accepts_merge_with_merged_and_blocked() {
        use agentic_afk_contracts::PhaseOutputBody;
        let db = setup_db().await;
        let project = create_project(
            &db,
            &CreateProjectRequest {
                path: "/tmp".to_string(),
            },
        )
        .await
        .unwrap();
        let plan_run = create_plan_run(&db, &project.id.0, "main", "deadbeef")
            .await
            .unwrap();
        let body = PhaseOutputBody::Merge {
            merged_source_ids: vec!["42".into()],
            verification: vec!["cargo test".into()],
            gaps: vec![],
            summary: "integrated cleanly".into(),
            block_reason: None,
        };
        let stored =
            record_plan_run_phase_output_typed(&db, &plan_run.id, "merge", "merged", &body)
                .await
                .unwrap();
        assert_eq!(stored.outcome, "merged");
        assert_eq!(
            stored
                .body_json
                .get("merged_source_ids")
                .and_then(|v| v.as_array())
                .map(|a| a.len()),
            Some(1)
        );

        let body = PhaseOutputBody::Merge {
            merged_source_ids: vec![],
            verification: vec![],
            gaps: vec![],
            summary: "conflict".into(),
            block_reason: Some("unresolvable conflict".into()),
        };
        let stored =
            record_plan_run_phase_output_typed(&db, &plan_run.id, "merge", "blocked", &body)
                .await
                .unwrap();
        assert_eq!(stored.outcome, "blocked");
    }

    #[tokio::test]
    async fn write_seam_rejects_merge_with_non_merge_outcome() {
        use agentic_afk_contracts::PhaseOutputBody;
        let db = setup_db().await;
        let project = create_project(
            &db,
            &CreateProjectRequest {
                path: "/tmp".to_string(),
            },
        )
        .await
        .unwrap();
        let plan_run = create_plan_run(&db, &project.id.0, "main", "deadbeef")
            .await
            .unwrap();
        let body = PhaseOutputBody::Merge {
            merged_source_ids: vec![],
            verification: vec![],
            gaps: vec![],
            summary: "x".into(),
            block_reason: None,
        };
        // `approved` belongs to the Review pairing — Merge bodies must
        // never land with a non-merge outcome.
        let result =
            record_plan_run_phase_output_typed(&db, &plan_run.id, "merge", "approved", &body)
                .await;
        assert!(
            matches!(result, Err(PersistenceError::PhaseOutputMismatch { .. })),
            "expected PhaseOutputMismatch, got {result:?}"
        );
    }

    #[tokio::test]
    async fn write_seam_accepts_push_with_succeeded_and_failed() {
        use agentic_afk_contracts::PhaseOutputBody;
        let db = setup_db().await;
        let project = create_project(
            &db,
            &CreateProjectRequest {
                path: "/tmp".to_string(),
            },
        )
        .await
        .unwrap();
        let plan_run = create_plan_run(&db, &project.id.0, "main", "deadbeef")
            .await
            .unwrap();
        // Push body paired with `succeeded` is the happy-path Plan-Run-scoped
        // push row (ADR-0038 push slice). `assignment_id` stays `None`.
        let succ = PhaseOutputBody::Push {
            stderr: String::new(),
            fast_forward: true,
            attempt: 1,
        };
        let stored = record_plan_run_phase_output_typed(&db, &plan_run.id, "push", "succeeded", &succ)
            .await
            .unwrap();
        assert_eq!(stored.phase, "push");
        assert_eq!(stored.outcome, "succeeded");
        assert!(stored.assignment_id.is_none(),
            "push Phase Output must be Plan-Run-scoped (assignment_id = None)");

        // Push body paired with `failed` carries the upstream stderr.
        let failed = PhaseOutputBody::Push {
            stderr: "remote rejected: non-fast-forward".to_string(),
            fast_forward: false,
            attempt: 2,
        };
        let stored = record_plan_run_phase_output_typed(&db, &plan_run.id, "push", "failed", &failed)
            .await
            .unwrap();
        assert_eq!(stored.outcome, "failed");
        assert!(stored.assignment_id.is_none());
        assert_eq!(
            stored.body_json["stderr"].as_str(),
            Some("remote rejected: non-fast-forward")
        );
        assert_eq!(stored.body_json["fast_forward"].as_bool(), Some(false));
        assert_eq!(stored.body_json["attempt"].as_u64(), Some(2));
    }

    #[tokio::test]
    async fn write_seam_rejects_push_with_non_push_outcome() {
        use agentic_afk_contracts::PhaseOutputBody;
        let db = setup_db().await;
        let project = create_project(
            &db,
            &CreateProjectRequest {
                path: "/tmp".to_string(),
            },
        )
        .await
        .unwrap();
        let plan_run = create_plan_run(&db, &project.id.0, "main", "deadbeef")
            .await
            .unwrap();
        let body = PhaseOutputBody::Push {
            stderr: String::new(),
            fast_forward: true,
            attempt: 1,
        };
        // `merged` belongs to the Merge pairing — a Push body paired with
        // a non-push outcome must be rejected so the audit log stays sound.
        let result =
            record_plan_run_phase_output_typed(&db, &plan_run.id, "push", "merged", &body).await;
        assert!(
            matches!(result, Err(PersistenceError::PhaseOutputMismatch { .. })),
            "expected PhaseOutputMismatch, got {result:?}"
        );
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
