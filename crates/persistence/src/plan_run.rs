//! Persistence helpers for Project Execution Config and Plan Runs (ADR-0034).

use crate::{Db, PHASE_OUTPUT_BODY_MAX_BYTES, PersistenceError};
use agentic_afk_contracts::{
    PhaseOutputBody, PhaseOutputResponse, PlanRunResponse, ProjectExecutionConfigResponse,
    ProjectId, SetProjectExecutionConfigRequest,
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

/// Plan-Run-scoped Phase Output write seam (free-form body, legacy callers).
///
/// Wraps the raw `body_json` in the appropriate [`PhaseOutputBody`] stub so
/// the single typed write-seam below performs the outcome/body-pairing
/// validation and 64 KB truncation (ADR-0038).
pub async fn record_plan_run_phase_output(
    db: &Db,
    plan_run_id: &str,
    phase: &str,
    outcome: &str,
    body_json: &serde_json::Value,
) -> Result<PhaseOutputResponse, PersistenceError> {
    let body = wrap_legacy_body(phase, outcome, body_json);
    record_phase_output(db, plan_run_id, None, phase, outcome, &body).await
}

/// Plan-Run-scoped typed write seam. Validates outcome ↔ body variant
/// pairing and truncates the serialized body to 64 KB before persisting.
/// `phase` is the recording-phase column (planning / implementation / review
/// / merge / push) and may differ from the body's variant tag when the body
/// is `Failed` (the column stays as the originating phase per ADR-0038).
pub async fn record_plan_run_phase_output_typed(
    db: &Db,
    plan_run_id: &str,
    phase: &str,
    outcome: &str,
    body: &PhaseOutputBody,
) -> Result<PhaseOutputResponse, PersistenceError> {
    record_phase_output(db, plan_run_id, None, phase, outcome, body).await
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
    let body = wrap_legacy_body(phase, outcome, body_json);
    record_phase_output(db, plan_run_id, Some(assignment_id), phase, outcome, &body).await
}

/// Assignment-scoped typed write seam (see [`record_plan_run_phase_output_typed`]).
pub async fn record_assignment_phase_output_typed(
    db: &Db,
    plan_run_id: &str,
    assignment_id: &str,
    phase: &str,
    outcome: &str,
    body: &PhaseOutputBody,
) -> Result<PhaseOutputResponse, PersistenceError> {
    record_phase_output(db, plan_run_id, Some(assignment_id), phase, outcome, body).await
}

/// Wrap a free-form legacy body in the matching [`PhaseOutputBody`] variant.
/// `outcome="failed"` always routes through [`PhaseOutputBody::Failed`] so
/// the chokepoint validation can guard the pairing; other phases stay in
/// their stub variants until subsequent ADR-0038 slices tighten them.
fn wrap_legacy_body(
    phase: &str,
    outcome: &str,
    body_json: &serde_json::Value,
) -> PhaseOutputBody {
    // Push records structured bodies for both success and failure
    // outcomes (ADR-0038 push slice). Inject the `phase` discriminator
    // so serde can route the legacy free-form body into the typed
    // [`PhaseOutputBody::Push`] variant; fall back to a zero-value Push
    // shape when the body cannot be coerced so the row still lands.
    if phase == "push" {
        return inject_phase_and_deserialize_push(body_json);
    }
    if outcome == "failed" {
        let error = body_json
            .get("error")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| body_json.to_string());
        let problem_type = body_json
            .get("problem_type")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        return PhaseOutputBody::Failed {
            error,
            problem_type,
        };
    }
    match phase {
        "planning" => parse_planning_body(body_json),
        "implementation" => parse_implementation_body(body_json),
        "review" => parse_review_body(body_json),
        "merge" => parse_merge_body(body_json),
        // Unknown phase: route to Failed so the row still lands with a
        // typed body the Dashboard can render.
        _ => PhaseOutputBody::Failed {
            error: body_json.to_string(),
            problem_type: None,
        },
    }
}

/// Deserialize a free-form planning body into the typed
/// [`PhaseOutputBody::Planning`] variant. Tolerates legacy in-the-wild
/// planner-output rows that recorded `issues: [...]` (the raw planner
/// stdout shape) by mapping each issue into a [`PlanningSelection`].
/// Falls back to an empty Planning body when neither shape applies so
/// the row still lands.
fn parse_planning_body(body_json: &serde_json::Value) -> PhaseOutputBody {
    use agentic_afk_contracts::{PlanningSelection, RejectedPlanningCandidate};

    // Strict path: the body is already a typed Planning body.
    let mut value = body_json.clone();
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "phase".to_string(),
            serde_json::Value::String("planning".to_string()),
        );
    }
    if let Ok(typed @ PhaseOutputBody::Planning { .. }) =
        serde_json::from_value::<PhaseOutputBody>(value)
    {
        return typed;
    }

    // Legacy path: raw planner stdout body with `issues: [...]`.
    let selections = body_json
        .get("issues")
        .and_then(serde_json::Value::as_array)
        .map(|issues| {
            issues
                .iter()
                .filter_map(|issue| {
                    Some(PlanningSelection {
                        source_issue_id: issue
                            .get("source_issue_id")?
                            .as_str()?
                            .to_string(),
                        title: issue
                            .get("title")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        branch: issue
                            .get("branch")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        selection_summary: issue
                            .get("selection_summary")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let summary = body_json
        .get("summary")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let rejected_candidates = body_json
        .get("rejected_candidates")
        .and_then(serde_json::Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    Some(RejectedPlanningCandidate {
                        source_issue_id: row
                            .get("source_issue_id")?
                            .as_str()?
                            .to_string(),
                        reason: row
                            .get("reason")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    PhaseOutputBody::Planning {
        selections,
        summary,
        rejected_candidates,
    }
}

/// Deserialize a free-form implementation body into the typed
/// [`PhaseOutputBody::Implementation`] variant. Falls back to a Failed
/// row when the body lacks the required shape so legacy in-the-wild rows
/// still land somewhere the Dashboard can render.
fn parse_implementation_body(body_json: &serde_json::Value) -> PhaseOutputBody {
    inject_phase_and_deserialize(body_json, "implementation")
}

/// Deserialize a free-form review body into the typed
/// [`PhaseOutputBody::Review`] variant. Tolerant of legacy reviewer fakes
/// that emit `findings: ["msg"]` rather than `[{location, message}]` via
/// the [`agentic_afk_contracts::ReviewFinding`] custom deserializer.
fn parse_review_body(body_json: &serde_json::Value) -> PhaseOutputBody {
    inject_phase_and_deserialize(body_json, "review")
}

/// Deserialize a free-form merge body into the typed
/// [`PhaseOutputBody::Merge`] variant so legacy in-the-wild rows surface
/// `merged_source_ids` / `verification` / `summary` / `block_reason` on
/// the Dashboard rather than the opaque pretty-printed JSON fallback.
fn parse_merge_body(body_json: &serde_json::Value) -> PhaseOutputBody {
    inject_phase_and_deserialize(body_json, "merge")
}

/// Stamp the `phase` discriminator onto a free-form body and let serde
/// route it into the matching typed [`PhaseOutputBody`] variant. Falls
/// back to a Failed row when the body cannot be coerced so legacy
/// in-the-wild rows still land somewhere the Dashboard can render.
/// Coerce a free-form push body into the typed [`PhaseOutputBody::Push`]
/// variant. Falls back to a zero-valued Push (empty stderr,
/// `fast_forward=false`, attempt=0) when the body cannot be coerced so
/// the row still lands.
fn inject_phase_and_deserialize_push(body_json: &serde_json::Value) -> PhaseOutputBody {
    let mut value = body_json.clone();
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "phase".to_string(),
            serde_json::Value::String("push".to_string()),
        );
    }
    serde_json::from_value::<PhaseOutputBody>(value).unwrap_or(PhaseOutputBody::Push {
        stderr: String::new(),
        fast_forward: false,
        attempt: 0,
    })
}

fn inject_phase_and_deserialize(body_json: &serde_json::Value, phase_tag: &str) -> PhaseOutputBody {
    let mut value = body_json.clone();
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "phase".to_string(),
            serde_json::Value::String(phase_tag.to_string()),
        );
    }
    serde_json::from_value::<PhaseOutputBody>(value).unwrap_or_else(|_| PhaseOutputBody::Failed {
        error: body_json.to_string(),
        problem_type: None,
    })
}

/// Single chokepoint for Phase Output persistence (ADR-0038).
/// Validates that `outcome` and the body variant pair sensibly, serializes
/// the body to JSON, replaces it with a `truncated_at: <bytes>` marker when
/// the serialized form exceeds [`PHASE_OUTPUT_BODY_MAX_BYTES`], and writes
/// the row.
async fn record_phase_output(
    db: &Db,
    plan_run_id: &str,
    assignment_id: Option<&str>,
    phase: &str,
    outcome: &str,
    body: &PhaseOutputBody,
) -> Result<PhaseOutputResponse, PersistenceError> {
    validate_outcome_body(outcome, body)?;
    let stored_body = truncate_body_if_needed(body)?;
    let body_text = serde_json::to_string(&stored_body)
        .map_err(|e| PersistenceError::Database(sqlx::Error::Decode(Box::new(e))))?;
    let id = Uuid::new_v4().to_string();
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
        body_json: stored_body,
        recorded_at,
        assignment_id: assignment_id.map(str::to_owned),
    })
}

/// Reject outcome ↔ body pairings that would corrupt the audit log.
/// At this slice the validated pairings are:
/// * [`PhaseOutputBody::Failed`] requires `outcome == "failed"` — the
///   Failed variant carries an error string and has no other plausible
///   outcome.
/// * `outcome == "failed"` paired with a structured phase body
///   (Planning / Implementation / Review / Merge) is rejected: those
///   variants are stubs and surface failures through the Failed variant.
///   The Push variant is exempt because push records structured bodies
///   for both success and failure outcomes (ADR-0038 push slice).
fn validate_outcome_body(outcome: &str, body: &PhaseOutputBody) -> Result<(), PersistenceError> {
    let is_failed_body = matches!(body, PhaseOutputBody::Failed { .. });
    let is_failed_outcome = outcome == "failed";
    if is_failed_body && !is_failed_outcome {
        return Err(PersistenceError::PhaseOutputMismatch {
            body_phase: body.phase_tag(),
            outcome: outcome.to_string(),
        });
    }
    if is_failed_outcome && !is_failed_body && !matches!(body, PhaseOutputBody::Push { .. }) {
        return Err(PersistenceError::PhaseOutputMismatch {
            body_phase: body.phase_tag(),
            outcome: outcome.to_string(),
        });
    }
    // Push body pairs with `succeeded` (fast-forward accepted) or
    // `failed` (non-fast-forward / network / auth). Other outcomes would
    // corrupt the audit log (ADR-0038 push slice).
    if matches!(body, PhaseOutputBody::Push { .. })
        && !matches!(outcome, "succeeded" | "failed")
    {
        return Err(PersistenceError::PhaseOutputMismatch {
            body_phase: body.phase_tag(),
            outcome: outcome.to_string(),
        });
    }
    // Implementation body only pairs with `ready_for_review`. The
    // legitimate failure path uses the Failed variant.
    if matches!(body, PhaseOutputBody::Implementation { .. }) && outcome != "ready_for_review" {
        return Err(PersistenceError::PhaseOutputMismatch {
            body_phase: body.phase_tag(),
            outcome: outcome.to_string(),
        });
    }
    // Review body pairs with `approved` or `rejected`. Other outcomes
    // (e.g. `ready_for_review`, `merged`) would corrupt the audit log.
    if matches!(body, PhaseOutputBody::Review { .. })
        && !matches!(outcome, "approved" | "rejected")
    {
        return Err(PersistenceError::PhaseOutputMismatch {
            body_phase: body.phase_tag(),
            outcome: outcome.to_string(),
        });
    }
    // Planning body pairs with `succeeded` (non-empty selections) or
    // `succeeded_empty` (empty selections). Runner/parse failures land
    // as `Failed` with `outcome = "failed"` via the failure path above.
    if let PhaseOutputBody::Planning { selections, .. } = body {
        match outcome {
            "succeeded" if !selections.is_empty() => {}
            "succeeded_empty" if selections.is_empty() => {}
            _ => {
                return Err(PersistenceError::PhaseOutputMismatch {
                    body_phase: body.phase_tag(),
                    outcome: outcome.to_string(),
                });
            }
        }
    }
    // Merge body pairs with `merged` (clean local integration) or
    // `blocked` (merge could not finish safely; `block_reason` carries
    // the reason). Runner/parse failures land as `Failed` with
    // `outcome = "failed"` via the failure path above.
    if matches!(body, PhaseOutputBody::Merge { .. })
        && !matches!(outcome, "merged" | "blocked")
    {
        return Err(PersistenceError::PhaseOutputMismatch {
            body_phase: body.phase_tag(),
            outcome: outcome.to_string(),
        });
    }
    Ok(())
}

/// Serialize `body` and, if the JSON exceeds the 64 KB ceiling, replace it
/// with a marker object so the on-disk row stays bounded. Returns the
/// `serde_json::Value` that will land in the `body_json` column.
fn truncate_body_if_needed(
    body: &PhaseOutputBody,
) -> Result<serde_json::Value, PersistenceError> {
    let raw = serde_json::to_value(body)
        .map_err(|e| PersistenceError::Database(sqlx::Error::Decode(Box::new(e))))?;
    let serialized = serde_json::to_string(&raw)
        .map_err(|e| PersistenceError::Database(sqlx::Error::Decode(Box::new(e))))?;
    if serialized.len() <= PHASE_OUTPUT_BODY_MAX_BYTES {
        return Ok(raw);
    }
    // Preserve the `phase` tag so the truncated row still discriminates,
    // and append the marker so the Dashboard can render `[truncated]`.
    Ok(serde_json::json!({
        "phase": body.phase_tag(),
        "truncated_at": serialized.len(),
    }))
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
