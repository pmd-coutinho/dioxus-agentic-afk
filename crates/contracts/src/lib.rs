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
    /// Detected default branch from `refs/remotes/origin/HEAD`. Used to
    /// seed the per-Project Integration Branch the first time the
    /// developer configures execution. Falls back to `None` when origin
    /// HEAD is not configured locally.
    #[serde(default)]
    pub default_branch: Option<String>,
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
    pub dependency_blocked: Vec<SourceIssueSnapshot>,
    pub active: Vec<SourceIssueSnapshot>,
    pub completed: Vec<SourceIssueSnapshot>,
    pub eligible: Vec<SourceIssueSnapshot>,
    /// Source Issues an operator has flagged locally as Parent-Issue-style
    /// PRDs. PRD-marked rows are excluded from every active bucket above so
    /// no agent can pick them for direct implementation. Carried on the
    /// snapshot so the Dashboard can render the "N PRDs hidden" unmark
    /// affordance without a separate fetch. See CONTEXT.md → Parent Issue.
    #[serde(default)]
    pub prd_overrides: Vec<SourceIssueSnapshot>,
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

/// Fine-grained execution state of one **Issue Assignment** inside its
/// **Plan Run** (CONTEXT.md → Assignment Status). The wire encoding is the
/// lowercase discriminator string stored in `issue_assignments.status` and
/// surfaced on [`IssueAssignmentResponse::status`]. `MergeStaged` (ADR-0037)
/// sits between `Merging` and `Merged`: the Merge Phase has integrated
/// locally and verified but the Integration Branch push has not yet
/// succeeded. `Merged` always implies a pushed Integration Branch.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentStatusKind {
    Implementing,
    Implemented,
    Reviewed,
    Merging,
    MergeStaged,
    Merged,
    Blocked,
}

impl AssignmentStatusKind {
    /// Stable wire encoding used in `issue_assignments.status` and the
    /// `IssueAssignmentResponse::status` field surfaced over SSE / REST.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Implementing => "implementing",
            Self::Implemented => "implemented",
            Self::Reviewed => "reviewed",
            Self::Merging => "merging",
            Self::MergeStaged => "merge_staged",
            Self::Merged => "merged",
            Self::Blocked => "blocked",
        }
    }
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
    /// Plan Run that owns this assignment (ADR-0034). `None` for legacy
    /// rows from before the Plan Run flow landed.
    #[serde(default)]
    pub plan_run_id: Option<String>,
    /// Planner selection summary captured when the Planning Phase chose
    /// this Source Issue for the Plan Run.
    #[serde(default)]
    pub selection_summary: Option<String>,
    /// Implementation and review Phase Outputs recorded against this
    /// assignment, oldest first. Empty for `provisional` / freshly
    /// `claimed` assignments that have not yet run a phase.
    #[serde(default)]
    pub phase_outputs: Vec<PhaseOutputResponse>,
    /// Number of rejected Review Phases recorded against this assignment.
    /// Reset to zero when a human re-enables a blocked assignment.
    #[serde(default)]
    pub review_rejection_count: i64,
    /// Typed cause of a blocked **Issue Assignment** paired with optional
    /// freeform `detail` (ADR-0038). `None` for assignments that have never
    /// been blocked or have since been re-enabled.
    #[serde(default)]
    pub block_reason: Option<BlockReasonResponse>,
}

/// Typed cause of a blocked **Issue Assignment** (CONTEXT.md → Block Reason,
/// ADR-0038). The wire encoding is the lowercase snake-case discriminator
/// emitted by [`BlockReason::as_wire`].
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BlockReason {
    /// The Review Loop reached the per-Project Review Retry Limit without
    /// approval (see CONTEXT.md → Review Retry Limit).
    ReviewRetryLimitExhausted,
    /// The Merge Phase could not reach a successful merge (conflict the
    /// agent could not resolve, integration verification failure it could
    /// not fix, runner failure, or unparseable output).
    MergePhaseFailed,
    /// A push attempt for a `merge_staged` **Issue Assignment** was
    /// rejected because the **Integration Branch** has diverged
    /// (non-fast-forward). Recovery belongs in a new **Plan Run** with a
    /// refreshed baseline rather than another **Retry Push** (ADR-0037).
    PushNonFastForward,
    /// The operator chose **Abandon Staged** on a `merge_staged`
    /// **Issue Assignment** (ADR-0037 / issue #54). No push was attempted;
    /// the staged work will not land in the **Integration Branch**.
    AbandonedStaged,
}

impl BlockReason {
    /// Stable wire encoding stored in `issue_assignments.block_reason_kind`
    /// and surfaced on [`BlockReasonResponse::kind`] over REST and SSE.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::ReviewRetryLimitExhausted => "review_retry_limit_exhausted",
            Self::MergePhaseFailed => "merge_phase_failed",
            Self::PushNonFastForward => "push_non_fast_forward",
            Self::AbandonedStaged => "abandoned_staged",
        }
    }

    /// Parse the wire encoding back into a typed variant. Returns `None`
    /// for unrecognized values (e.g. legacy rows whose `block_reason_kind`
    /// is `NULL` or a future variant the current binary does not know).
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "review_retry_limit_exhausted" => Some(Self::ReviewRetryLimitExhausted),
            "merge_phase_failed" => Some(Self::MergePhaseFailed),
            "push_non_fast_forward" => Some(Self::PushNonFastForward),
            "abandoned_staged" => Some(Self::AbandonedStaged),
            _ => None,
        }
    }
}

/// Structured Block Reason surfaced on the Issue Assignment API response
/// (ADR-0038). `kind` is the typed taxonomy that drives Dashboard
/// affordances; `detail` carries cause-specific freeform text (push stderr,
/// conflict file list, verification log tail) that has no fixed schema.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct BlockReasonResponse {
    pub kind: BlockReason,
    #[serde(default)]
    pub detail: Option<String>,
}

/// Result body of `POST /api/projects/{id}/assignments/{aid}/retry-push`
/// (issue #53, ADR-0037). `status` is the post-retry Assignment Status:
/// `merged` (push succeeded, Lifecycle `Completed` write-back done best-
/// effort), `merge_staged` (transient `Other` failure — retry remains
/// possible), or `blocked` (non-fast-forward divergence; recovery belongs
/// in a new Plan Run). `block_reason` is present iff `status == "blocked"`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct RetryPushResponse {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_reason: Option<BlockReasonResponse>,
}

/// Request body for
/// `POST /api/projects/{id}/assignments/{aid}/abandon-staged`
/// (issue #54, ADR-0037). The body is optional; an empty payload is
/// accepted and leaves `block_reason.detail = None`. When supplied,
/// `note` becomes the freeform `detail` on the resulting
/// `BlockReason::AbandonedStaged`.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct AbandonStagedRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Result body of
/// `POST /api/projects/{id}/assignments/{aid}/abandon-staged`
/// (issue #54, ADR-0037). The operator action always terminates the
/// **Issue Assignment** at `blocked` with
/// [`BlockReason::AbandonedStaged`]; the response shape mirrors the
/// `blocked` arm of [`RetryPushResponse`] so the Dashboard can update
/// without an additional fetch.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct AbandonStagedResponse {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_reason: Option<BlockReasonResponse>,
}

/// Wire shape of the upstream Lifecycle `Ready` write-back arm of
/// the Source-Issue-keyed re-enable response (issue #55, ADR-0038).
/// `ok == true` means the upstream **Issue Source** acknowledged the
/// write; `ok == false` carries the failure message so the Dashboard
/// can render a partial-success warning. The local clear is reported
/// separately in [`ReEnableSourceIssueResponse::local_cleared`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct WritebackOutcomeResponse {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Result body of
/// `POST /api/projects/{id}/source-issues/{sid}/re-enable`
/// (issue #55, ADR-0038). Surfaces both halves of the use case so the
/// Dashboard can render partial-success warnings without an additional
/// fetch. HTTP status is `200 OK` even when `writeback.ok == false`
/// because the local clear succeeded per ADR-0035.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct ReEnableSourceIssueResponse {
    pub local_cleared: bool,
    pub writeback: WritebackOutcomeResponse,
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
    /// Issue Assignments selected and claimed by the Plan Run's Planning
    /// Phase. Empty for `succeeded_empty` runs.
    #[serde(default)]
    pub assignments: Vec<IssueAssignmentResponse>,
}

/// Typed Phase Output body recorded against a Plan Run or Issue Assignment
/// (ADR-0038). Tagged on the `phase` discriminator so every persisted body
/// carries its phase identity inline; the outer `outcome` column remains the
/// SQL-level filter for "failed phase outputs of Plan Run X" queries.
///
/// This slice ships the `Failed` variant fully. The remaining variants are
/// stubs that wrap the existing free-form body JSON so per-phase shapes can
/// be tightened in subsequent slices without breaking existing in-flight
/// callsites or in-the-wild rows.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum PhaseOutputBody {
    /// Planning Phase body — the structured product of one Planning Phase
    /// run. `selections` carries one entry per Planned Claim the planner
    /// chose (Source Issue identity + derived issue branch + selection
    /// summary); `summary` is the planner's high-level rationale text
    /// surfaced on the collapsed Dashboard row; `rejected_candidates`
    /// captures Source Issues the planner explicitly considered and
    /// declined (each with a reason). Validated against
    /// `outcome = "succeeded"` (non-empty `selections`) or
    /// `outcome = "succeeded_empty"` (empty `selections`) at the
    /// persistence write seam — runner/parse failures land as `Failed`
    /// with `outcome = "failed"`.
    Planning {
        #[serde(default)]
        selections: Vec<PlanningSelection>,
        #[serde(default)]
        summary: String,
        #[serde(default)]
        rejected_candidates: Vec<RejectedPlanningCandidate>,
    },
    /// Implementation Phase body — the structured product of one
    /// Implementation pass on an Issue Assignment. `commits` is the list
    /// of commit SHAs touched on the Issue Branch; `verification` carries
    /// the verification command transcript the agent ran (one entry per
    /// command); `gaps` is the agent's self-reported list of open items
    /// the next phase should know about; `summary` is a one-line human
    /// summary for the collapsed Dashboard row. Validated against
    /// `outcome = "ready_for_review"` at the persistence write seam.
    Implementation {
        #[serde(default)]
        commits: Vec<String>,
        #[serde(default)]
        verification: Vec<String>,
        #[serde(default)]
        gaps: Vec<String>,
        #[serde(default)]
        summary: String,
    },
    /// Review Phase body — the structured product of one Review pass on
    /// an Issue Assignment. `findings` is the ordered list of reviewer
    /// findings (each with optional `location` and required `message`);
    /// `verification` carries the verification command transcript the
    /// reviewer ran; `gaps` mirrors the implementation shape; `summary`
    /// is the one-line collapsed summary. Validated against
    /// `outcome = "approved"` or `outcome = "rejected"` at the
    /// persistence write seam.
    Review {
        #[serde(default)]
        findings: Vec<ReviewFinding>,
        #[serde(default)]
        verification: Vec<String>,
        #[serde(default)]
        gaps: Vec<String>,
        #[serde(default)]
        summary: String,
    },
    /// Merge Phase body — the structured product of one Merge pass on an
    /// Issue Assignment. `merged_source_ids` lists the Source Issue ids
    /// whose branches were integrated (empty when blocked);
    /// `verification` carries the verification command transcript the
    /// merger ran for the integrated result; `gaps` is the merger's
    /// self-reported list of verification gaps the next phase should
    /// know about; `summary` is the one-line collapsed Dashboard
    /// summary; `block_reason` is set only when the merge could not
    /// finish safely (paired with `outcome = "blocked"`). Validated
    /// against `outcome = "merged"` or `outcome = "blocked"` at the
    /// persistence write seam — runner/parse failures land as `Failed`
    /// with `outcome = "failed"`.
    Merge {
        #[serde(default)]
        merged_source_ids: Vec<String>,
        #[serde(default)]
        verification: Vec<String>,
        #[serde(default)]
        gaps: Vec<String>,
        #[serde(default)]
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        block_reason: Option<String>,
    },
    /// Integration Branch push body — the structured product of one
    /// `git push` attempt against the Integration Branch. `stderr` carries
    /// the upstream error text on failure (empty on success);
    /// `fast_forward` is `true` only when the upstream accepted the push
    /// as a fast-forward update; `attempt` is the 1-indexed attempt number
    /// for this Plan Run (one row per attempt — append-only, never
    /// mutating prior rows). Validated against `outcome = "succeeded"` or
    /// `outcome = "failed"` at the persistence write seam (ADR-0038).
    Push {
        #[serde(default)]
        stderr: String,
        #[serde(default)]
        fast_forward: bool,
        #[serde(default)]
        attempt: u32,
    },
    /// Failure body — carries the surfaced error and the RFC-7807
    /// problem-type URN of the originating CoordinatorError when known.
    /// Used by every phase failure path so Dashboard rendering and the
    /// write-seam validation share one shape.
    Failed {
        error: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        problem_type: Option<String>,
    },
}

/// One Planned Claim recorded inside a [`PhaseOutputBody::Planning`].
/// Pairs the planner's chosen Source Issue identity with the derived
/// **issue branch** name and the selection rationale shown on the
/// expanded Dashboard row.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct PlanningSelection {
    pub source_issue_id: String,
    pub title: String,
    pub branch: String,
    #[serde(default)]
    pub selection_summary: String,
}

/// One Source Issue the planner explicitly considered and rejected inside
/// a [`PhaseOutputBody::Planning`]. Pairs the candidate's identity with
/// the planner's rejection reason so the Dashboard can surface why a
/// nominally-eligible Source Issue did not make this Plan Run's batch.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct RejectedPlanningCandidate {
    pub source_issue_id: String,
    pub reason: String,
}

/// One reviewer finding inside a [`PhaseOutputBody::Review`]. `location`
/// names the source location (file path, function, or commit ref) the
/// finding is anchored to; `message` is the freeform reviewer text. Both
/// surface on the Dashboard expanded Review row.
///
/// Tolerant deserialization: a bare JSON string (legacy reviewer output
/// that only emitted message text) is accepted and becomes
/// `ReviewFinding { location: None, message }` so existing fakes and
/// in-the-wild rows keep working.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, ToSchema)]
pub struct ReviewFinding {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    pub message: String,
}

impl<'de> Deserialize<'de> for ReviewFinding {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Text(String),
            Structured {
                #[serde(default)]
                location: Option<String>,
                message: String,
            },
        }
        match Raw::deserialize(deserializer)? {
            Raw::Text(message) => Ok(ReviewFinding {
                location: None,
                message,
            }),
            Raw::Structured { location, message } => Ok(ReviewFinding { location, message }),
        }
    }
}

impl PhaseOutputBody {
    /// Variant discriminator as it appears in the serialized `phase` tag.
    /// Matches the existing `phase` column written by the persistence
    /// seam so legacy rows keep their column value.
    pub fn phase_tag(&self) -> &'static str {
        match self {
            Self::Planning { .. } => "planning",
            Self::Implementation { .. } => "implementation",
            Self::Review { .. } => "review",
            Self::Merge { .. } => "merge",
            Self::Push { .. } => "push",
            Self::Failed { .. } => "failed",
        }
    }

    /// Permissive deserialization for legacy in-the-wild Phase Output bodies
    /// recorded before ADR-0038 typing landed. Tries the strict tagged shape
    /// first; on failure, falls back to `Failed` carrying the legacy body's
    /// `error` field (or the whole body as a debug string) so the Dashboard
    /// can still render the row.
    pub fn from_legacy_value(value: serde_json::Value) -> Self {
        if let Ok(typed) = serde_json::from_value::<PhaseOutputBody>(value.clone()) {
            return typed;
        }
        let error = value
            .get("error")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| value.to_string());
        Self::Failed {
            error,
            problem_type: value
                .get("problem_type")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
        }
    }
}

/// One durable phase result (planning/implementation/review/merge) recorded
/// for a Plan Run or Assignment Attempt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct PhaseOutputResponse {
    pub phase: String,
    pub outcome: String,
    pub body_json: serde_json::Value,
    pub recorded_at: String,
    /// Owning Issue Assignment when this Phase Output came from an
    /// implementation or review pass. `None` for the planning Phase
    /// Output of a Plan Run.
    #[serde(default)]
    pub assignment_id: Option<String>,
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
        let candidates = ProjectEvent::IssueSourceCandidatesChanged { candidates: vec![] };
        let s = serde_json::to_string(&candidates).unwrap();
        assert!(s.contains("\"type\":\"issue_source_candidates_changed\""));
        let planning = ProjectEvent::PlanningSnapshotChanged { snapshot: None };
        let s = serde_json::to_string(&planning).unwrap();
        assert!(s.contains("\"type\":\"planning_snapshot_changed\""));
        let failed = ProjectEvent::IssueSourceSyncFailed { error: "x".into() };
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
    fn block_reason_round_trip_review_retry_limit_exhausted() {
        let reason = BlockReason::ReviewRetryLimitExhausted;
        assert_eq!(reason.as_wire(), "review_retry_limit_exhausted");
        let json = serde_json::to_string(&reason).unwrap();
        assert_eq!(json, "\"review_retry_limit_exhausted\"");
        let back: BlockReason = serde_json::from_str(&json).unwrap();
        assert_eq!(back, reason);
        assert_eq!(
            BlockReason::from_wire("review_retry_limit_exhausted"),
            Some(BlockReason::ReviewRetryLimitExhausted)
        );
    }

    #[test]
    fn block_reason_round_trip_merge_phase_failed() {
        let reason = BlockReason::MergePhaseFailed;
        assert_eq!(reason.as_wire(), "merge_phase_failed");
        let json = serde_json::to_string(&reason).unwrap();
        assert_eq!(json, "\"merge_phase_failed\"");
        let back: BlockReason = serde_json::from_str(&json).unwrap();
        assert_eq!(back, reason);
        assert_eq!(
            BlockReason::from_wire("merge_phase_failed"),
            Some(BlockReason::MergePhaseFailed)
        );
    }

    #[test]
    fn block_reason_round_trip_push_non_fast_forward() {
        let reason = BlockReason::PushNonFastForward;
        assert_eq!(reason.as_wire(), "push_non_fast_forward");
        let json = serde_json::to_string(&reason).unwrap();
        assert_eq!(json, "\"push_non_fast_forward\"");
        let back: BlockReason = serde_json::from_str(&json).unwrap();
        assert_eq!(back, reason);
        assert_eq!(
            BlockReason::from_wire("push_non_fast_forward"),
            Some(BlockReason::PushNonFastForward)
        );
    }

    #[test]
    fn block_reason_round_trip_abandoned_staged() {
        let reason = BlockReason::AbandonedStaged;
        assert_eq!(reason.as_wire(), "abandoned_staged");
        let json = serde_json::to_string(&reason).unwrap();
        assert_eq!(json, "\"abandoned_staged\"");
        let back: BlockReason = serde_json::from_str(&json).unwrap();
        assert_eq!(back, reason);
        assert_eq!(
            BlockReason::from_wire("abandoned_staged"),
            Some(BlockReason::AbandonedStaged)
        );
    }

    #[test]
    fn retry_push_response_serializes_status_and_optional_reason() {
        let success = RetryPushResponse {
            status: "merged".to_string(),
            block_reason: None,
        };
        let json = serde_json::to_string(&success).unwrap();
        assert!(json.contains("\"status\":\"merged\""));
        assert!(!json.contains("block_reason"));
        let blocked = RetryPushResponse {
            status: "blocked".to_string(),
            block_reason: Some(BlockReasonResponse {
                kind: BlockReason::PushNonFastForward,
                detail: Some("stderr: rejected".to_string()),
            }),
        };
        let json = serde_json::to_string(&blocked).unwrap();
        assert!(json.contains("\"block_reason\""));
        assert!(json.contains("\"kind\":\"push_non_fast_forward\""));
    }

    #[test]
    fn block_reason_from_wire_rejects_unknown() {
        assert_eq!(BlockReason::from_wire("nope"), None);
    }

    #[test]
    fn block_reason_response_serializes_as_kind_and_detail() {
        let response = BlockReasonResponse {
            kind: BlockReason::ReviewRetryLimitExhausted,
            detail: Some("Review Loop exhausted: 2 rejection(s)".to_string()),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(
            json.contains("\"kind\":\"review_retry_limit_exhausted\""),
            "{json}"
        );
        assert!(json.contains("\"detail\":\"Review Loop"), "{json}");
        let back: BlockReasonResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back, response);
    }

    #[test]
    fn block_reason_response_round_trips_without_detail() {
        let response = BlockReasonResponse {
            kind: BlockReason::MergePhaseFailed,
            detail: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        let back: BlockReasonResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back, response);
    }

    #[test]
    fn phase_output_body_failed_round_trips_with_phase_tag() {
        let body = PhaseOutputBody::Failed {
            error: "boom".to_string(),
            problem_type: Some("urn:agentic-afk:planning-phase-failed".to_string()),
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains("\"phase\":\"failed\""), "{json}");
        assert!(json.contains("\"error\":\"boom\""), "{json}");
        let back: PhaseOutputBody = serde_json::from_str(&json).unwrap();
        assert_eq!(back, body);
    }

    #[test]
    fn phase_output_body_legacy_shape_falls_back_to_failed() {
        // Existing in-the-wild rows pre-typed-body: just `{ "error": "..." }`
        // with no `phase` tag should deserialize as the Failed variant so the
        // Dashboard and write seam can still surface them.
        let legacy = serde_json::json!({ "error": "stderr: rejected" });
        let body = PhaseOutputBody::from_legacy_value(legacy);
        match body {
            PhaseOutputBody::Failed { error, problem_type } => {
                assert_eq!(error, "stderr: rejected");
                assert_eq!(problem_type, None);
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn phase_output_body_implementation_round_trips() {
        let body = PhaseOutputBody::Implementation {
            commits: vec!["abc123".to_string()],
            verification: vec!["cargo test --workspace".to_string()],
            gaps: vec!["no e2e yet".to_string()],
            summary: "shipped".to_string(),
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains("\"phase\":\"implementation\""), "{json}");
        assert!(json.contains("\"commits\":[\"abc123\"]"), "{json}");
        assert!(json.contains("\"summary\":\"shipped\""), "{json}");
        let back: PhaseOutputBody = serde_json::from_str(&json).unwrap();
        assert_eq!(back, body);
    }

    #[test]
    fn phase_output_body_review_round_trips_with_structured_finding() {
        let body = PhaseOutputBody::Review {
            findings: vec![ReviewFinding {
                location: Some("src/lib.rs:42".to_string()),
                message: "missing null check".to_string(),
            }],
            verification: vec!["cargo test".to_string()],
            gaps: vec![],
            summary: "needs more".to_string(),
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains("\"phase\":\"review\""), "{json}");
        assert!(json.contains("\"location\":\"src/lib.rs:42\""), "{json}");
        let back: PhaseOutputBody = serde_json::from_str(&json).unwrap();
        assert_eq!(back, body);
    }

    #[test]
    fn review_finding_accepts_bare_string_legacy_shape() {
        // Existing reviewer fakes emit `findings: ["missing tests"]` rather
        // than the structured `{location, message}` object. The tolerant
        // deserializer must accept the legacy shape so the Failed-style
        // permissive path on the Dashboard and persistence stay quiet.
        let json = r#"{"phase":"review","findings":["missing tests"],"summary":"x"}"#;
        let body: PhaseOutputBody = serde_json::from_str(json).unwrap();
        match body {
            PhaseOutputBody::Review { findings, summary, .. } => {
                assert_eq!(findings.len(), 1);
                assert_eq!(findings[0].location, None);
                assert_eq!(findings[0].message, "missing tests");
                assert_eq!(summary, "x");
            }
            other => panic!("expected Review, got {other:?}"),
        }
    }

    #[test]
    fn phase_output_body_merge_round_trips() {
        let body = PhaseOutputBody::Merge {
            merged_source_ids: vec!["42".to_string()],
            verification: vec!["cargo test --workspace".to_string()],
            gaps: vec![],
            summary: "integrated cleanly".to_string(),
            block_reason: None,
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains("\"phase\":\"merge\""), "{json}");
        assert!(json.contains("\"merged_source_ids\":[\"42\"]"), "{json}");
        assert!(json.contains("\"summary\":\"integrated cleanly\""), "{json}");
        let back: PhaseOutputBody = serde_json::from_str(&json).unwrap();
        assert_eq!(back, body);
    }

    #[test]
    fn phase_output_body_merge_blocked_carries_block_reason() {
        let body = PhaseOutputBody::Merge {
            merged_source_ids: vec![],
            verification: vec!["cargo test".to_string()],
            gaps: vec!["unresolved conflict in src/foo.rs".to_string()],
            summary: "conflict".to_string(),
            block_reason: Some("unresolvable merge conflict".to_string()),
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(
            json.contains("\"block_reason\":\"unresolvable merge conflict\""),
            "{json}"
        );
        let back: PhaseOutputBody = serde_json::from_str(&json).unwrap();
        assert_eq!(back, body);
    }

    #[test]
    fn phase_output_body_push_success_round_trips() {
        let body = PhaseOutputBody::Push {
            stderr: String::new(),
            fast_forward: true,
            attempt: 1,
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains("\"phase\":\"push\""), "{json}");
        assert!(json.contains("\"fast_forward\":true"), "{json}");
        assert!(json.contains("\"attempt\":1"), "{json}");
        let back: PhaseOutputBody = serde_json::from_str(&json).unwrap();
        assert_eq!(back, body);
    }

    #[test]
    fn phase_output_body_push_failure_carries_stderr() {
        let body = PhaseOutputBody::Push {
            stderr: "remote rejected: non-fast-forward".to_string(),
            fast_forward: false,
            attempt: 2,
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains("\"phase\":\"push\""), "{json}");
        assert!(json.contains("\"stderr\":\"remote rejected: non-fast-forward\""), "{json}");
        assert!(json.contains("\"fast_forward\":false"), "{json}");
        assert!(json.contains("\"attempt\":2"), "{json}");
        let back: PhaseOutputBody = serde_json::from_str(&json).unwrap();
        assert_eq!(back, body);
    }

    #[test]
    fn phase_output_body_planning_typed_round_trips_with_selection() {
        let body = PhaseOutputBody::Planning {
            selections: vec![PlanningSelection {
                source_issue_id: "62".to_string(),
                title: "Typed Planning variant".to_string(),
                branch: "agent/issue-62".to_string(),
                selection_summary: "baseline ready".to_string(),
            }],
            summary: "one ready issue".to_string(),
            rejected_candidates: vec![RejectedPlanningCandidate {
                source_issue_id: "63".to_string(),
                reason: "depends on #62".to_string(),
            }],
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains("\"phase\":\"planning\""), "{json}");
        assert!(json.contains("\"source_issue_id\":\"62\""), "{json}");
        assert!(json.contains("\"branch\":\"agent/issue-62\""), "{json}");
        assert!(json.contains("\"selection_summary\":\"baseline ready\""), "{json}");
        assert!(json.contains("\"reason\":\"depends on #62\""), "{json}");
        let back: PhaseOutputBody = serde_json::from_str(&json).unwrap();
        assert_eq!(back, body);
    }

    #[test]
    fn phase_output_body_planning_typed_round_trips_empty_selection() {
        let body = PhaseOutputBody::Planning {
            selections: vec![],
            summary: "no eligible work".to_string(),
            rejected_candidates: vec![],
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains("\"phase\":\"planning\""), "{json}");
        assert!(json.contains("\"selections\":[]"), "{json}");
        let back: PhaseOutputBody = serde_json::from_str(&json).unwrap();
        assert_eq!(back, body);
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
