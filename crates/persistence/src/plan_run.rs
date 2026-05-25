//! Persistence helpers for Project Execution Config and Plan Runs (ADR-0034).

use crate::{Db, PHASE_OUTPUT_BODY_MAX_BYTES, PersistenceError};
use agentic_afk_contracts::{
    PhaseOutputBody, PhaseOutputResponse, PlanRunResponse, PlanRunState,
    ProjectExecutionConfigResponse, ProjectId, SetProjectExecutionConfigRequest,
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
        VALUES (?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&id)
    .bind(project_id)
    .bind(integration_branch)
    .bind(baseline_commit)
    .bind(PlanRunState::Running.as_wire())
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
fn wrap_legacy_body(phase: &str, outcome: &str, body_json: &serde_json::Value) -> PhaseOutputBody {
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
                        source_issue_id: issue.get("source_issue_id")?.as_str()?.to_string(),
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
                        source_issue_id: row.get("source_issue_id")?.as_str()?.to_string(),
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
    if matches!(body, PhaseOutputBody::Push { .. }) && !matches!(outcome, "succeeded" | "failed") {
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
    if matches!(body, PhaseOutputBody::Review { .. }) && !matches!(outcome, "approved" | "rejected")
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
    if matches!(body, PhaseOutputBody::Merge { .. }) && !matches!(outcome, "merged" | "blocked") {
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
fn truncate_body_if_needed(body: &PhaseOutputBody) -> Result<serde_json::Value, PersistenceError> {
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
    let rows = sqlx::query_as::<_, (String, String, Option<String>, String)>(
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
            let body_json = parse_optional_body_json(body_text.as_deref())?;
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

/// In-flight rows carry `body_json = NULL` (ADR-0042). Decode the column
/// as `Null` JSON so the rest of the read path (Dashboard renderers,
/// SSE deltas) does not need to special-case the missing-body shape.
fn parse_optional_body_json(
    body_text: Option<&str>,
) -> Result<serde_json::Value, PersistenceError> {
    let Some(text) = body_text else {
        return Ok(serde_json::Value::Null);
    };
    serde_json::from_str(text)
        .map_err(|e| PersistenceError::Database(sqlx::Error::Decode(Box::new(e))))
}

/// Transition a Plan Run to a terminal state and stamp `finished_at`.
pub async fn finish_plan_run(
    db: &Db,
    plan_run_id: &str,
    state: PlanRunState,
) -> Result<PlanRunResponse, PersistenceError> {
    let finished_at = current_unix_timestamp();
    sqlx::query("UPDATE plan_runs SET state = ?, finished_at = ? WHERE id = ?")
        .bind(state.as_wire())
        .bind(&finished_at)
        .bind(plan_run_id)
        .execute(db)
        .await?;
    get_plan_run(db, plan_run_id).await
}

pub async fn get_plan_run(db: &Db, plan_run_id: &str) -> Result<PlanRunResponse, PersistenceError> {
    let row = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            String,
            String,
            String,
            Option<String>,
        ),
    >(
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
    let state =
        PlanRunState::from_wire(&state).ok_or_else(|| PersistenceError::InvalidPlanRunState {
            plan_run_id: id.clone(),
            value: state.clone(),
        })?;
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
    let rows = sqlx::query_as::<_, (String, String, Option<String>, String, Option<String>)>(
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
            let body_json = parse_optional_body_json(body_text.as_deref())?;
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
        WHERE project_id = ? AND state = ?
        ORDER BY started_at DESC, rowid DESC
        LIMIT 1
        "#,
    )
    .bind(project_id)
    .bind(PlanRunState::Running.as_wire())
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
        WHERE project_id = ? AND state != ?
        ORDER BY started_at DESC, rowid DESC
        LIMIT ?
        "#,
    )
    .bind(project_id)
    .bind(PlanRunState::Running.as_wire())
    .bind(limit)
    .fetch_all(db)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for (id,) in rows {
        out.push(get_plan_run(db, &id).await?);
    }
    Ok(out)
}

/// Insert an `in_flight` phase output row *before* the Codex Sandbox is
/// launched (ADR-0042). The row carries `body_json = NULL` and no captured
/// PID yet; the orchestrator calls [`record_in_flight_phase_process`] right
/// after spawn and [`complete_in_flight_phase_output`] on terminal outcome.
/// Returns the row id so the orchestrator can target the subsequent UPDATEs
/// at the same row without scanning.
pub async fn insert_in_flight_phase_output(
    db: &Db,
    plan_run_id: &str,
    assignment_id: Option<&str>,
    phase: &str,
) -> Result<String, PersistenceError> {
    let id = Uuid::new_v4().to_string();
    let recorded_at = current_unix_timestamp();
    sqlx::query(
        r#"
        INSERT INTO plan_run_phase_outputs (
            id, plan_run_id, phase, outcome, body_json, recorded_at, assignment_id
        )
        VALUES (?, ?, ?, 'in_flight', NULL, ?, ?)
        "#,
    )
    .bind(&id)
    .bind(plan_run_id)
    .bind(phase)
    .bind(&recorded_at)
    .bind(assignment_id)
    .execute(db)
    .await?;
    Ok(id)
}

/// Stamp the captured child PID + spawn timestamp onto an in-flight row
/// (ADR-0042). Called by the orchestrator immediately after the Codex child
/// is spawned, before any await that blocks on the child's exit, so the
/// ShutdownCoordinator can SIGTERM the captured PID.
pub async fn record_in_flight_phase_process(
    db: &Db,
    row_id: &str,
    process_id: u32,
    process_started_at: &str,
) -> Result<(), PersistenceError> {
    sqlx::query(
        r#"
        UPDATE plan_run_phase_outputs
        SET process_id = ?, process_started_at = ?
        WHERE id = ?
        "#,
    )
    .bind(process_id as i64)
    .bind(process_started_at)
    .bind(row_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Replace an in-flight row's `outcome` + `body_json` with the terminal
/// result (ADR-0042). Validates the outcome ↔ body pairing through the
/// existing chokepoint and applies the same 64 KB truncation marker.
pub async fn complete_in_flight_phase_output(
    db: &Db,
    row_id: &str,
    outcome: &str,
    body: &PhaseOutputBody,
) -> Result<(), PersistenceError> {
    validate_outcome_body(outcome, body)?;
    let stored_body = truncate_body_if_needed(body)?;
    let body_text = serde_json::to_string(&stored_body)
        .map_err(|e| PersistenceError::Database(sqlx::Error::Decode(Box::new(e))))?;
    sqlx::query(
        r#"
        UPDATE plan_run_phase_outputs
        SET outcome = ?, body_json = ?
        WHERE id = ?
        "#,
    )
    .bind(outcome)
    .bind(&body_text)
    .bind(row_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Mark a single `in_flight` row as `failed` with a stable leak-guard
/// problem-type URN in the body. Called from `TrackedPhase`'s Drop impl
/// when the handle is abandoned without `complete` or `fail`, so leaked
/// rows self-heal instead of stuck-forever blocking the dashboard. The
/// `outcome='in_flight'` guard makes the update a no-op if the row was
/// already finalised by another path (defensive — the Drop impl already
/// checks its `finalized` flag).
pub async fn mark_phase_row_leaked(db: &Db, row_id: &str) -> Result<(), PersistenceError> {
    let body_text = r#"{"type":"Failed","error":"phase handle dropped without finalization (TrackedPhase leak guard)","problem_type":"urn:agentic-afk:phase-handle-leaked"}"#;
    sqlx::query(
        r#"
        UPDATE plan_run_phase_outputs
        SET outcome = 'failed', body_json = ?
        WHERE id = ? AND outcome = 'in_flight'
        "#,
    )
    .bind(body_text)
    .bind(row_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Summary of one `in_flight` or `interrupted` row, returned by
/// [`mark_in_flight_rows_interrupted`] and [`list_in_flight_phase_rows`]
/// so callers (ShutdownCoordinator / BootRecoveryScanner) can SIGTERM
/// captured PIDs and block the owning Issue Assignments without re-loading
/// the row themselves.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InFlightPhaseRowSummary {
    pub id: String,
    pub plan_run_id: String,
    pub assignment_id: Option<String>,
    pub phase: String,
    pub outcome: String,
    pub process_id: Option<u32>,
}

/// Return every non-terminal phase output row (`outcome IN ('in_flight',
/// 'interrupted')`). Used by both the ShutdownCoordinator (to find rows it
/// must mark on the way out) and the BootRecoveryScanner (to find rows it
/// must recover on the way in).
pub async fn list_in_flight_phase_rows(
    db: &Db,
) -> Result<Vec<InFlightPhaseRowSummary>, PersistenceError> {
    let rows = sqlx::query_as::<_, (String, String, Option<String>, String, String, Option<i64>)>(
        r#"
        SELECT id, plan_run_id, assignment_id, phase, outcome, process_id
        FROM plan_run_phase_outputs
        WHERE outcome IN ('in_flight', 'interrupted')
        ORDER BY recorded_at ASC, rowid ASC
        "#,
    )
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, plan_run_id, assignment_id, phase, outcome, process_id)| {
                InFlightPhaseRowSummary {
                    id,
                    plan_run_id,
                    assignment_id,
                    phase,
                    outcome,
                    process_id: process_id.and_then(|pid| u32::try_from(pid).ok()),
                }
            },
        )
        .collect())
}

/// Mark one specific phase output row as `interrupted` by row id. Used
/// by the [`crate`]-internal BootRecoveryScanner caller path so per-row
/// transitions (block-this-assignment, leave-this-merge-staged, etc.)
/// can land row-by-row rather than via the batch
/// [`mark_in_flight_rows_interrupted`] sweep.
pub async fn mark_phase_row_interrupted(db: &Db, row_id: &str) -> Result<(), PersistenceError> {
    sqlx::query(
        r#"
        UPDATE plan_run_phase_outputs
        SET outcome = 'interrupted'
        WHERE id = ? AND outcome = 'in_flight'
        "#,
    )
    .bind(row_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Flip every `in_flight` row to `interrupted` and return the affected
/// rows so the caller (ShutdownCoordinator) can SIGTERM the captured PIDs.
/// Idempotent: rows already at `interrupted` are left alone but still
/// returned so a second call returns the same set without double-killing.
pub async fn mark_in_flight_rows_interrupted(
    db: &Db,
) -> Result<Vec<InFlightPhaseRowSummary>, PersistenceError> {
    // Snapshot first so the caller can SIGTERM the captured PIDs even when
    // a concurrent completion flips a row to a terminal outcome between
    // the SELECT and the UPDATE.
    let affected = list_in_flight_phase_rows(db).await?;
    sqlx::query(
        r#"
        UPDATE plan_run_phase_outputs
        SET outcome = 'interrupted'
        WHERE outcome = 'in_flight'
        "#,
    )
    .execute(db)
    .await?;
    Ok(affected)
}

fn current_unix_timestamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("unix:{seconds}")
}
