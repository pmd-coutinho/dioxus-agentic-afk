//! Persistence helpers for Project Execution Config and Plan Runs (ADR-0034).

use crate::{Db, PersistenceError};
use agentic_afk_contracts::{
    PhaseOutputResponse, PlanRunResponse, ProjectExecutionConfigResponse, ProjectId,
    SetProjectExecutionConfigRequest,
};
use uuid::Uuid;

/// Upsert the Project Execution Config and return the persisted view.
pub async fn set_project_execution_config(
    db: &Db,
    project_id: &str,
    request: &SetProjectExecutionConfigRequest,
) -> Result<ProjectExecutionConfigResponse, PersistenceError> {
    crate::get_project(db, project_id).await?;
    sqlx::query(
        r#"
        INSERT INTO project_execution_configs (
            project_id, integration_branch, max_parallel_tasks, review_retry_limit
        )
        VALUES (?, ?, ?, ?)
        ON CONFLICT(project_id) DO UPDATE SET
            integration_branch = excluded.integration_branch,
            max_parallel_tasks = excluded.max_parallel_tasks,
            review_retry_limit = excluded.review_retry_limit
        "#,
    )
    .bind(project_id)
    .bind(&request.integration_branch)
    .bind(request.max_parallel_tasks)
    .bind(request.review_retry_limit)
    .execute(db)
    .await?;
    get_project_execution_config(db, project_id)
        .await
        .map(Option::unwrap)
}

pub async fn get_project_execution_config(
    db: &Db,
    project_id: &str,
) -> Result<Option<ProjectExecutionConfigResponse>, PersistenceError> {
    let row = sqlx::query_as::<_, (String, i64, i64)>(
        r#"
        SELECT integration_branch, max_parallel_tasks, review_retry_limit
        FROM project_execution_configs
        WHERE project_id = ?
        "#,
    )
    .bind(project_id)
    .fetch_optional(db)
    .await?;
    Ok(row.map(
        |(integration_branch, max_parallel_tasks, review_retry_limit)| {
            ProjectExecutionConfigResponse {
                integration_branch,
                max_parallel_tasks,
                review_retry_limit,
            }
        },
    ))
}

/// Insert a fresh Plan Run in `running` state and return it.
pub async fn create_plan_run(
    db: &Db,
    project_id: &str,
    integration_branch: &str,
    baseline_commit: &str,
) -> Result<PlanRunResponse, PersistenceError> {
    let id = Uuid::new_v4().to_string();
    let started_at = current_unix_timestamp();
    sqlx::query(
        r#"
        INSERT INTO plan_runs (
            id, project_id, integration_branch, baseline_commit, state, started_at
        )
        VALUES (?, ?, ?, ?, 'running', ?)
        "#,
    )
    .bind(&id)
    .bind(project_id)
    .bind(integration_branch)
    .bind(baseline_commit)
    .bind(&started_at)
    .execute(db)
    .await?;
    get_plan_run(db, &id).await
}

pub async fn record_plan_run_phase_output(
    db: &Db,
    plan_run_id: &str,
    phase: &str,
    outcome: &str,
    body_json: &serde_json::Value,
) -> Result<PhaseOutputResponse, PersistenceError> {
    record_phase_output(db, plan_run_id, None, phase, outcome, body_json).await
}

/// Record a Phase Output attributed to a particular Issue Assignment
/// (implementation / review). The plan_run_id is the assignment's owning
/// Plan Run.
pub async fn record_assignment_phase_output(
    db: &Db,
    plan_run_id: &str,
    assignment_id: &str,
    phase: &str,
    outcome: &str,
    body_json: &serde_json::Value,
) -> Result<PhaseOutputResponse, PersistenceError> {
    record_phase_output(
        db,
        plan_run_id,
        Some(assignment_id),
        phase,
        outcome,
        body_json,
    )
    .await
}

async fn record_phase_output(
    db: &Db,
    plan_run_id: &str,
    assignment_id: Option<&str>,
    phase: &str,
    outcome: &str,
    body_json: &serde_json::Value,
) -> Result<PhaseOutputResponse, PersistenceError> {
    let id = Uuid::new_v4().to_string();
    let body_text = serde_json::to_string(body_json)
        .map_err(|e| PersistenceError::Database(sqlx::Error::Decode(Box::new(e))))?;
    let recorded_at = current_unix_timestamp();
    sqlx::query(
        r#"
        INSERT INTO plan_run_phase_outputs (
            id, plan_run_id, phase, outcome, body_json, recorded_at, assignment_id
        )
        VALUES (?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&id)
    .bind(plan_run_id)
    .bind(phase)
    .bind(outcome)
    .bind(&body_text)
    .bind(&recorded_at)
    .bind(assignment_id)
    .execute(db)
    .await?;
    Ok(PhaseOutputResponse {
        phase: phase.to_string(),
        outcome: outcome.to_string(),
        body_json: body_json.clone(),
        recorded_at,
        assignment_id: assignment_id.map(str::to_owned),
    })
}

/// List Phase Outputs attributed to one Issue Assignment, oldest first.
pub async fn list_assignment_phase_outputs(
    db: &Db,
    assignment_id: &str,
) -> Result<Vec<PhaseOutputResponse>, PersistenceError> {
    let rows = sqlx::query_as::<_, (String, String, String, String)>(
        r#"
        SELECT phase, outcome, body_json, recorded_at
        FROM plan_run_phase_outputs
        WHERE assignment_id = ?
        ORDER BY recorded_at ASC, rowid ASC
        "#,
    )
    .bind(assignment_id)
    .fetch_all(db)
    .await?;
    rows.into_iter()
        .map(|(phase, outcome, body_text, recorded_at)| {
            let body_json: serde_json::Value = serde_json::from_str(&body_text)
                .map_err(|e| PersistenceError::Database(sqlx::Error::Decode(Box::new(e))))?;
            Ok(PhaseOutputResponse {
                phase,
                outcome,
                body_json,
                recorded_at,
                assignment_id: Some(assignment_id.to_string()),
            })
        })
        .collect()
}

/// Transition a Plan Run to a terminal state and stamp `finished_at`.
pub async fn finish_plan_run(
    db: &Db,
    plan_run_id: &str,
    state: &str,
) -> Result<PlanRunResponse, PersistenceError> {
    let finished_at = current_unix_timestamp();
    sqlx::query("UPDATE plan_runs SET state = ?, finished_at = ? WHERE id = ?")
        .bind(state)
        .bind(&finished_at)
        .bind(plan_run_id)
        .execute(db)
        .await?;
    get_plan_run(db, plan_run_id).await
}

pub async fn get_plan_run(db: &Db, plan_run_id: &str) -> Result<PlanRunResponse, PersistenceError> {
    let row = sqlx::query_as::<_, (String, String, String, String, String, String, Option<String>)>(
        r#"
        SELECT id, project_id, integration_branch, baseline_commit, state, started_at, finished_at
        FROM plan_runs
        WHERE id = ?
        "#,
    )
    .bind(plan_run_id)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| PersistenceError::NotFound(plan_run_id.to_string()))?;
    let (id, project_id, integration_branch, baseline_commit, state, started_at, finished_at) = row;
    let phase_outputs = list_phase_outputs(db, &id).await?;
    let assignments = crate::list_plan_run_assignments(db, &id).await?;
    Ok(PlanRunResponse {
        id,
        project_id: ProjectId(project_id),
        integration_branch,
        baseline_commit,
        state,
        started_at,
        finished_at,
        phase_outputs,
        assignments,
    })
}

async fn list_phase_outputs(
    db: &Db,
    plan_run_id: &str,
) -> Result<Vec<PhaseOutputResponse>, PersistenceError> {
    let rows = sqlx::query_as::<_, (String, String, String, String, Option<String>)>(
        r#"
        SELECT phase, outcome, body_json, recorded_at, assignment_id
        FROM plan_run_phase_outputs
        WHERE plan_run_id = ?
        ORDER BY recorded_at ASC, rowid ASC
        "#,
    )
    .bind(plan_run_id)
    .fetch_all(db)
    .await?;
    rows.into_iter()
        .map(|(phase, outcome, body_text, recorded_at, assignment_id)| {
            let body_json: serde_json::Value = serde_json::from_str(&body_text)
                .map_err(|e| PersistenceError::Database(sqlx::Error::Decode(Box::new(e))))?;
            Ok(PhaseOutputResponse {
                phase,
                outcome,
                body_json,
                recorded_at,
                assignment_id,
            })
        })
        .collect()
}

/// The Project's active (non-terminal) Plan Run, if any.
pub async fn get_active_plan_run(
    db: &Db,
    project_id: &str,
) -> Result<Option<PlanRunResponse>, PersistenceError> {
    let id = sqlx::query_as::<_, (String,)>(
        r#"
        SELECT id FROM plan_runs
        WHERE project_id = ? AND state = 'running'
        ORDER BY started_at DESC, rowid DESC
        LIMIT 1
        "#,
    )
    .bind(project_id)
    .fetch_optional(db)
    .await?
    .map(|row| row.0);
    match id {
        Some(id) => get_plan_run(db, &id).await.map(Some),
        None => Ok(None),
    }
}

/// Recent finished Plan Runs for a Project (newest first).
pub async fn list_recent_plan_runs(
    db: &Db,
    project_id: &str,
    limit: i64,
) -> Result<Vec<PlanRunResponse>, PersistenceError> {
    let rows = sqlx::query_as::<_, (String,)>(
        r#"
        SELECT id FROM plan_runs
        WHERE project_id = ? AND state != 'running'
        ORDER BY started_at DESC, rowid DESC
        LIMIT ?
        "#,
    )
    .bind(project_id)
    .bind(limit)
    .fetch_all(db)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for (id,) in rows {
        out.push(get_plan_run(db, &id).await?);
    }
    Ok(out)
}

fn current_unix_timestamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("unix:{seconds}")
}
