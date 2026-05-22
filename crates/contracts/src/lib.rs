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
    pub change_proposal: Option<ChangeProposalResponse>,
    pub latest_attempt: Option<AssignmentAttemptResponse>,
    #[serde(default)]
    pub repair_budget: Option<RepairBudgetResponse>,
}

/// Bounded GitHub Change Proposal Repair Loop budget for an Issue Assignment.
///
/// `attempt_count` is incremented only by `repair` Assignment Attempts; recovery
/// attempts never advance this budget. `window_started_at` stamps when the
/// elapsed window began (unix seconds, recorded on the first repair attempt).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct RepairBudgetResponse {
    pub attempt_count: i64,
    pub max_attempts: i64,
    pub window_seconds: i64,
    pub window_started_at: Option<i64>,
}

/// Failed required GitHub check fact carried into a repair Assignment Attempt.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct FailedCheckFact {
    pub name: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
}

/// Request body for starting a repair Assignment Attempt on a failed
/// Change Proposal.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct RepairAssignmentRequest {
    #[serde(default)]
    pub failed_checks: Vec<FailedCheckFact>,
    #[serde(default)]
    pub verified_worktree_facts: Option<String>,
}

/// Hosted code proposal created from an Issue Assignment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct ChangeProposalResponse {
    pub status: String,
    pub url: String,
}

/// Project detail Issue Assignment state for the first single-slot execution slice.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct ProjectAssignmentStateResponse {
    pub active_assignment: Option<IssueAssignmentResponse>,
    pub waiting_ready_issue_count: usize,
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
    pub assignment_state: ProjectAssignmentStateResponse,
    pub activity: Vec<ProjectActivityEntryResponse>,
    /// Advisory Issue Source candidates discovered from Project evidence.
    /// Bundled into the snapshot so the Dashboard does not need a
    /// separate fetch for the Issue Source panel.
    #[serde(default)]
    pub issue_source_candidates: Vec<IssueSourceCandidate>,
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
    /// The Change Proposal for an active assignment was re-read from the host.
    ChangeProposalRefreshed {
        assignment_id: String,
        change_proposal: ChangeProposalResponse,
    },
    /// The Change Proposal for an active assignment passed required checks.
    ChangeProposalVerified {
        assignment_id: String,
        change_proposal: ChangeProposalResponse,
    },
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
