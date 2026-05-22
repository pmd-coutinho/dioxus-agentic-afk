use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct HealthResponse {
    pub status: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct AppInfoResponse {
    pub app_name: String,
    pub version: String,
    pub api_status: String,
    pub config: EffectiveConfig,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct EffectiveConfig {
    pub bind_address: String,
    pub dashboard_asset_dir: String,
    pub database_url: String,
}

// --- Project contracts ---

/// Stable identifier for a Project, independent of filesystem path.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, ToSchema)]
pub struct ProjectId(pub String);

/// Request body for creating a new Project.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct CreateProjectRequest {
    pub path: String,
}

/// Response body representing a Project.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct ProjectResponse {
    pub id: ProjectId,
    pub path: String,
    pub trusted: bool,
    pub git_summary: Option<GitSummary>,
    pub enabled_issue_source: Option<IssueSource>,
}

/// Read-only Git metadata derived from a Project path.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct GitSummary {
    pub branch: Option<String>,
    pub head: Option<String>,
    pub dirty: bool,
}

/// Persisted Issue Source selected for a Project.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct IssueSource {
    pub kind: String,
    pub locator: String,
}

/// Request body for deliberately enabling or switching a Project Issue Source.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct EnableIssueSourceRequest {
    pub kind: String,
    pub locator: String,
}

/// Advisory Issue Source candidate discovered from Project evidence.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct IssueSourceCandidate {
    pub kind: String,
    pub locator: String,
    pub enabled: bool,
}

/// Manual sync result for the enabled Issue Source.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct IssueSourceSyncResponse {
    pub source: IssueSource,
    pub last_successful_sync_at: Option<String>,
    pub last_failure: Option<String>,
}

/// Current sync status for the enabled Issue Source.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct IssueSourceSyncStatusResponse {
    pub source: IssueSource,
    pub last_successful_sync_at: Option<String>,
    pub last_failure: Option<String>,
}

/// Persisted cache of normalized Source Issues from an Issue Source.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct PlanningSnapshotResponse {
    pub source: IssueSource,
    pub last_successful_sync_at: Option<String>,
    pub last_failure: Option<String>,
    pub non_ready: Vec<SourceIssueSnapshot>,
    pub blocked: Vec<SourceIssueSnapshot>,
    pub active: Vec<SourceIssueSnapshot>,
    pub completed: Vec<SourceIssueSnapshot>,
    pub eligible: Vec<SourceIssueSnapshot>,
}

/// Normalized scheduling metadata plus preserved raw Source Issue text.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct SourceIssueSnapshot {
    pub source_id: String,
    pub title: String,
    pub readiness: String,
    pub lifecycle_status: String,
    pub parent_issue: Option<String>,
    pub issue_dependencies: Vec<String>,
    pub source_order: i64,
    pub raw_text: String,
}

/// Durable view of one Issue Assignment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct IssueAssignmentResponse {
    pub id: String,
    pub project_id: ProjectId,
    pub source_id: String,
    pub source_title: String,
    pub branch: String,
    pub worktree_path: String,
    pub status: String,
    pub status_detail: Option<String>,
    pub latest_attempt: Option<AssignmentAttemptResponse>,
}

// --- Plan Run contracts (ADR-0034) ---

/// Per-Project execution configuration consumed by Plan Runs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct ProjectExecutionConfigResponse {
    pub integration_branch: String,
    pub max_parallel_tasks: i64,
    pub review_retry_limit: i64,
}

/// Request body for setting `Project Execution Config`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct SetProjectExecutionConfigRequest {
    pub integration_branch: String,
    pub max_parallel_tasks: i64,
    pub review_retry_limit: i64,
}

/// One Plan Run: a manually started Planning Phase plus the parallel issue
/// work it selects through Review and Merge. This slice only exercises the
/// empty-selection planning outcome.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct PlanRunResponse {
    pub id: String,
    pub project_id: ProjectId,
    pub integration_branch: String,
    pub baseline_commit: String,
    pub state: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub phase_outputs: Vec<PhaseOutputResponse>,
}

/// One durable phase result (planning/implementation/review/merge) recorded
/// for a Plan Run or Assignment Attempt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct PhaseOutputResponse {
    pub phase: String,
    pub outcome: String,
    pub body_json: serde_json::Value,
    pub recorded_at: String,
}

/// One agent execution pass within an Issue Assignment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct AssignmentAttemptResponse {
    pub id: String,
    pub kind: String,
    pub process_id: Option<u32>,
    pub process_identity: Option<String>,
    pub terminal_outcome: Option<AssignmentTerminalOutcome>,
}

/// Structured terminal outcome reported by the Codex backend.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct AssignmentTerminalOutcome {
    pub outcome: String,
    pub summary: String,
}

/// One Project Activity entry. Activity is the chronological record of
/// noteworthy Control Plane lifecycle events for a Project. Detail is bounded
/// so full Codex output never lands here (ADR-0030).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct ProjectActivityEntryResponse {
    pub id: String,
    pub project_id: String,
    pub assignment_id: Option<String>,
    pub kind: String,
    pub detail: Option<String>,
    pub recorded_at: String,
}

/// Bundle of Project state served as the single hydration response for the
/// Dashboard's reactive store. Composed from the four panel-scoped GETs
/// (`project`, `planning-snapshot`, `assignment-state`, `activity`) so the
/// Dashboard does one round trip instead of four.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct ProjectSnapshot {
    pub project: ProjectResponse,
    pub planning_snapshot: Option<PlanningSnapshotResponse>,
    pub activity: Vec<ProjectActivityEntryResponse>,
    /// Advisory Issue Source candidates discovered from Project evidence.
    /// Bundled into the snapshot so the Dashboard does not need a
    /// separate fetch for the Issue Source panel.
    #[serde(default)]
    pub issue_source_candidates: Vec<IssueSourceCandidate>,
    /// Per-Project execution configuration that gates Plan Run execution.
    /// `None` until a developer sets it for the first time.
    #[serde(default)]
    pub execution_config: Option<ProjectExecutionConfigResponse>,
    /// The Project's active (in-progress) Plan Run, if any. At most one is
    /// active per Project (ADR-0034).
    #[serde(default)]
    pub active_plan_run: Option<PlanRunResponse>,
    /// Recent finished Plan Runs, newest first.
    #[serde(default)]
    pub recent_plan_runs: Vec<PlanRunResponse>,
}

/// HTTP response body for `GET /api/projects/{id}/snapshot`. Carries the
/// monotonic `sequence` so the client can resume the SSE stream from
/// `Last-Event-ID: <sequence>` and receive only deltas missed since.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct ProjectSnapshotResponse {
    pub snapshot: ProjectSnapshot,
    pub sequence: u64,
}

/// Typed delta pushed over SSE to the Dashboard. Variants mirror the
/// Activity envelope (`kind` + bounded `detail`) so the audit log and the
/// live wire format remain a single source of truth, with `Resync` added
/// for the ring-buffer-overflow recovery path (ADR-0032).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProjectEvent {
    /// One Project Activity entry was appended. Carries the same fields the
    /// `activity` REST endpoint exposes so the Dashboard can append it
    /// directly to its activity list without an additional fetch.
    Activity(ProjectActivityEntryResponse),
    /// A new Issue Assignment became active for the Project.
    AssignmentCreated(IssueAssignmentResponse),
    /// An existing Issue Assignment transitioned to a new lifecycle status.
    AssignmentStatusChanged(IssueAssignmentResponse),
    /// A new Assignment Attempt was recorded against an active assignment.
    AssignmentAttemptAdded {
        assignment_id: String,
        attempt: AssignmentAttemptResponse,
    },
    /// A Plan Run was started for the Project; carries the initial Plan Run
    /// shape so the Dashboard can render it without an additional fetch.
    PlanRunStarted(PlanRunResponse),
    /// A Plan Run phase recorded a durable Phase Output.
    PlanRunPhaseCompleted {
        plan_run_id: String,
        phase_output: PhaseOutputResponse,
    },
    /// A Plan Run reached a terminal state.
    PlanRunCompleted(PlanRunResponse),
    /// The Project Execution Config changed.
    ProjectExecutionConfigChanged(ProjectExecutionConfigResponse),
    /// The Project's Planning Snapshot was regenerated (e.g. after a sync
    /// or after enabling a new Issue Source). `snapshot` is `None` when the
    /// snapshot was cleared because no Issue Source is currently enabled.
    PlanningSnapshotChanged {
        snapshot: Option<PlanningSnapshotResponse>,
    },
    /// An Issue Source sync started; the Dashboard should reflect a
    /// transient "syncing" state until a matching Completed or Failed event
    /// arrives.
    IssueSourceSyncStarted,
    /// An Issue Source sync completed successfully; carries the new sync
    /// metadata so the Dashboard can stop showing the in-progress state.
    IssueSourceSyncCompleted(IssueSourceSyncResponse),
    /// An Issue Source sync failed; the Dashboard surfaces `error` as the
    /// last failure message and clears the in-progress state.
    IssueSourceSyncFailed { error: String },
    /// The set of advisory Issue Source candidates was recomputed (e.g.
    /// after a candidate was enabled or after re-scanning Project evidence).
    IssueSourceCandidatesChanged {
        candidates: Vec<IssueSourceCandidate>,
    },
    /// Top-level Project metadata changed (trusted flag, enabled Issue
    /// Source, etc.).
    ProjectChanged(ProjectResponse),
    /// The client's `Last-Event-ID` predates the per-Project ring buffer.
    /// The client must re-fetch `/snapshot` to recover authoritative state.
    Resync,
}

/// RFC 7807 problem+json error response.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct ProblemDetail {
    #[serde(rename = "type")]
    pub problem_type: String,
    pub title: String,
    pub status: u16,
    pub detail: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_event_variants_serialize_with_type_tag() {
        let candidates = ProjectEvent::IssueSourceCandidatesChanged {
            candidates: vec![],
        };
        let s = serde_json::to_string(&candidates).unwrap();
        assert!(s.contains("\"type\":\"issue_source_candidates_changed\""));
        let planning = ProjectEvent::PlanningSnapshotChanged { snapshot: None };
        let s = serde_json::to_string(&planning).unwrap();
        assert!(s.contains("\"type\":\"planning_snapshot_changed\""));
        let failed = ProjectEvent::IssueSourceSyncFailed {
            error: "x".into(),
        };
        let s = serde_json::to_string(&failed).unwrap();
        println!("Failed: {s}");
        let started = ProjectEvent::IssueSourceSyncStarted;
        let s = serde_json::to_string(&started).unwrap();
        println!("Started: {s}");
    }

    #[test]
    fn project_id_is_uuid_format() {
        let id = ProjectId("550e8400-e29b-41d4-a716-446655440000".to_string());
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, r#""550e8400-e29b-41d4-a716-446655440000""#);
        let deserialized: ProjectId = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, id);
    }

    #[test]
    fn create_project_request_serializes() {
        let req = CreateProjectRequest {
            path: "/home/user/my-project".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let deserialized: CreateProjectRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, req);
    }

    #[test]
    fn project_response_serializes() {
        let resp = ProjectResponse {
            id: ProjectId("550e8400-e29b-41d4-a716-446655440000".to_string()),
            path: "/home/user/my-project".to_string(),
            trusted: false,
            git_summary: None,
            enabled_issue_source: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let deserialized: ProjectResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, resp);
    }

    #[test]
    fn problem_detail_uses_rfc7807_field_names() {
        let problem = ProblemDetail {
            problem_type: "urn:agentic-afk:validation-error".to_string(),
            title: "Validation Error".to_string(),
            status: 422,
            detail: "Path does not exist".to_string(),
        };
        let json = serde_json::to_string(&problem).unwrap();
        // RFC 7807 uses "type" field name
        assert!(json.contains(r#""type":"urn:agentic-afk:validation-error""#));
        assert!(json.contains(r#""status":422"#));
        let deserialized: ProblemDetail = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, problem);
    }
}
