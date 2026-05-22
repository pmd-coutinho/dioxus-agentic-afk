//! Reactive Project store + mutation lifecycle. See ADR-0032.

use std::collections::HashMap;
use std::future::Future;

use agentic_afk_contracts::{
    IssueSourceCandidate, PlanRunResponse, PlanningSnapshotResponse, ProblemDetail,
    ProjectActivityEntryResponse, ProjectEvent, ProjectExecutionConfigResponse, ProjectId,
    ProjectResponse, ProjectSnapshot,
};
use dioxus::prelude::*;

/// Outcome of applying one sequenced `ProjectEvent` to `ProjectStoreState`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplyOutcome {
    /// Event was sequential and merged into state.
    Merged,
    /// Event sequence is `<= last_seen_seq`; ignored.
    Stale,
    /// Sequence gap or explicit `Resync`; client must rehydrate via `/snapshot`.
    Rehydrate,
}

/// Pure store state hydrated from `/snapshot` then driven by SSE deltas.
/// Pure so tests can exercise hydrate-then-merge without a Dioxus runtime.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProjectStoreState {
    pub project: Option<ProjectResponse>,
    pub activity: Vec<ProjectActivityEntryResponse>,
    pub planning_snapshot: Option<PlanningSnapshotResponse>,
    pub issue_source_candidates: Vec<IssueSourceCandidate>,
    pub execution_config: Option<ProjectExecutionConfigResponse>,
    pub active_plan_run: Option<PlanRunResponse>,
    pub recent_plan_runs: Vec<PlanRunResponse>,
    pub sync_in_progress: bool,
    pub last_seen_seq: u64,
    pub needs_rehydrate: bool,
}

impl ProjectStoreState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace activity tail and reset sequence to the snapshot's value.
    pub fn hydrate(&mut self, snapshot: ProjectSnapshot, sequence: u64) {
        self.project = Some(snapshot.project);
        self.activity = snapshot.activity;
        self.execution_config = snapshot.execution_config;
        self.active_plan_run = snapshot.active_plan_run;
        self.recent_plan_runs = snapshot.recent_plan_runs;
        self.planning_snapshot = snapshot.planning_snapshot;
        self.issue_source_candidates = snapshot.issue_source_candidates;
        self.sync_in_progress = false;
        self.last_seen_seq = sequence;
        self.needs_rehydrate = false;
    }

    /// Apply one sequenced `ProjectEvent`. Enforces sequence monotonicity:
    /// stale events are ignored, sequential events merge into state, gaps and
    /// explicit `Resync` events flag the store as needing rehydrate.
    pub fn apply_event(&mut self, sequence: u64, event: ProjectEvent) -> ApplyOutcome {
        if matches!(event, ProjectEvent::Resync) {
            self.needs_rehydrate = true;
            return ApplyOutcome::Rehydrate;
        }
        if sequence <= self.last_seen_seq {
            return ApplyOutcome::Stale;
        }
        if sequence > self.last_seen_seq + 1 {
            self.needs_rehydrate = true;
            return ApplyOutcome::Rehydrate;
        }
        match event {
            ProjectEvent::Activity(entry) => {
                self.activity.insert(0, entry);
            }
            ProjectEvent::AssignmentCreated(assignment)
            | ProjectEvent::AssignmentStatusChanged(assignment) => {
                // Mirror the assignment into its owning Plan Run so the
                // Plan Run card shows the claimed Issue Assignment live
                // (issue #42).
                if let Some(plan_run_id) = assignment.plan_run_id.as_deref()
                    && let Some(active) = self.active_plan_run.as_mut()
                    && active.id == plan_run_id
                {
                    if let Some(slot) = active
                        .assignments
                        .iter_mut()
                        .find(|existing| existing.id == assignment.id)
                    {
                        *slot = assignment;
                    } else {
                        active.assignments.push(assignment);
                    }
                }
            }
            ProjectEvent::AssignmentAttemptAdded {
                assignment_id,
                attempt,
            } => {
                // Mirror onto the matching Plan Run assignment if any.
                if let Some(active) = self.active_plan_run.as_mut() {
                    if let Some(slot) = active
                        .assignments
                        .iter_mut()
                        .find(|existing| existing.id == assignment_id)
                    {
                        slot.latest_attempt = Some(attempt);
                    }
                }
            }
            ProjectEvent::PlanRunStarted(plan_run) => {
                self.active_plan_run = Some(plan_run);
            }
            ProjectEvent::PlanRunPhaseCompleted {
                plan_run_id,
                phase_output,
            } => {
                if let Some(active) = self.active_plan_run.as_mut()
                    && active.id == plan_run_id
                {
                    active.phase_outputs.push(phase_output);
                }
            }
            ProjectEvent::PlanRunCompleted(plan_run) => {
                if self
                    .active_plan_run
                    .as_ref()
                    .is_some_and(|active| active.id == plan_run.id)
                {
                    self.active_plan_run = None;
                }
                self.recent_plan_runs.insert(0, plan_run);
            }
            ProjectEvent::ProjectExecutionConfigChanged(config) => {
                self.execution_config = Some(config);
            }
            ProjectEvent::ProjectChanged(project) => {
                self.project = Some(project);
            }
            ProjectEvent::PlanningSnapshotChanged { snapshot } => {
                self.planning_snapshot = snapshot;
            }
            ProjectEvent::IssueSourceSyncStarted => {
                self.sync_in_progress = true;
            }
            ProjectEvent::IssueSourceSyncCompleted(sync) => {
                self.sync_in_progress = false;
                if let Some(planning) = self.planning_snapshot.as_mut() {
                    planning.last_successful_sync_at = sync.last_successful_sync_at;
                    planning.last_failure = sync.last_failure;
                }
            }
            ProjectEvent::IssueSourceSyncFailed { error } => {
                self.sync_in_progress = false;
                if let Some(planning) = self.planning_snapshot.as_mut() {
                    planning.last_failure = Some(error);
                }
            }
            ProjectEvent::IssueSourceCandidatesChanged { candidates } => {
                self.issue_source_candidates = candidates;
            }
            ProjectEvent::Resync => unreachable!("handled above"),
        }
        self.last_seen_seq = sequence;
        ApplyOutcome::Merged
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum MutationKey {
    TrustProject(ProjectId),
    SyncIssueSource(ProjectId),
    EnableIssueSource(ProjectId, String, String),
    SetExecutionConfig(ProjectId),
    StartPlanRun(ProjectId),
    ReEnableAssignment(ProjectId, IssueAssignmentId),
}

/// Identifier for an Issue Assignment (a single attempt to land a Source
/// Issue inside a Plan Run).
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct IssueAssignmentId(pub String);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutationCategory {
    Validation,
    Transient,
    System,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MutationState {
    Pending,
    Done,
    Error {
        category: MutationCategory,
        title: String,
        detail: String,
    },
}

impl MutationState {
    /// Categorize a failed HTTP response into a `MutationState::Error`.
    /// `status` is `None` for transport-level failures (network down, CORS, etc.).
    pub fn from_failure(status: Option<u16>, body: &str) -> Self {
        let problem = serde_json::from_str::<ProblemDetail>(body).ok();
        let (title, detail) = problem
            .map(|p| (p.title, p.detail))
            .unwrap_or_else(|| ("Request failed".to_string(), body.to_string()));
        let category = match status {
            Some(s) if (400..500).contains(&s) => MutationCategory::Validation,
            Some(s) if (500..600).contains(&s) => MutationCategory::Transient,
            None => MutationCategory::Transient,
            _ => MutationCategory::System,
        };
        MutationState::Error {
            category,
            title,
            detail,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct MutationsTable {
    entries: HashMap<MutationKey, MutationState>,
}

impl MutationsTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_pending(&mut self, key: MutationKey) {
        self.entries.insert(key, MutationState::Pending);
    }

    pub fn set_done(&mut self, key: MutationKey) {
        self.entries.insert(key, MutationState::Done);
    }

    pub fn set_error(
        &mut self,
        key: MutationKey,
        category: MutationCategory,
        title: String,
        detail: String,
    ) {
        self.entries.insert(
            key,
            MutationState::Error {
                category,
                title,
                detail,
            },
        );
    }

    pub fn get(&self, key: &MutationKey) -> Option<&MutationState> {
        self.entries.get(key)
    }

    pub fn is_pending(&self, key: &MutationKey) -> bool {
        matches!(self.entries.get(key), Some(MutationState::Pending))
    }
}

/// Raw failure carrying enough context for `MutationState::from_failure` to
/// categorize it. Returned by API call functions in place of `Result<_, String>`.
#[derive(Clone, Debug)]
pub struct MutationFailure {
    pub status: Option<u16>,
    pub body: String,
}

impl MutationFailure {
    pub fn network(message: impl Into<String>) -> Self {
        Self {
            status: None,
            body: message.into(),
        }
    }

    pub fn http(status: u16, body: impl Into<String>) -> Self {
        Self {
            status: Some(status),
            body: body.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToastKind {
    Success,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub kind: ToastKind,
    pub title: String,
    pub detail: String,
}

/// Reactive store wrapping `MutationsTable` and a toast queue.
///
/// Provided once on `AppShell` via `use_context_provider`. `Copy` because both
/// inner Signals are Copy.
#[derive(Clone, Copy, PartialEq)]
pub struct ProjectStore {
    mutations: Signal<MutationsTable>,
    toasts: Signal<Vec<Toast>>,
    next_toast_id: Signal<u64>,
    reload_counter: Signal<u64>,
    state: Signal<ProjectStoreState>,
}

impl ProjectStore {
    pub fn new() -> Self {
        Self {
            mutations: Signal::new(MutationsTable::new()),
            toasts: Signal::new(Vec::new()),
            next_toast_id: Signal::new(1),
            reload_counter: Signal::new(0),
            state: Signal::new(ProjectStoreState::new()),
        }
    }

    /// Read-side signal for the SSE-driven Project state. Components read
    /// `store.state().read().activity` etc.
    pub fn state_signal(&self) -> Signal<ProjectStoreState> {
        self.state
    }

    /// Replace activity tail and reset sequence to the snapshot's value.
    pub fn hydrate(&self, snapshot: ProjectSnapshot, sequence: u64) {
        self.state.clone().write().hydrate(snapshot, sequence);
    }

    /// Apply one sequenced `ProjectEvent` to the live state.
    pub fn apply_event(&self, sequence: u64, event: ProjectEvent) -> ApplyOutcome {
        self.state.clone().write().apply_event(sequence, event)
    }

    pub fn needs_rehydrate(&self) -> bool {
        self.state.read().needs_rehydrate
    }

    /// Clear the rehydrate flag once a rehydrate has been kicked off.
    pub fn clear_rehydrate_flag(&self) {
        self.state.clone().write().needs_rehydrate = false;
    }

    pub fn reload_counter(&self) -> Signal<u64> {
        self.reload_counter
    }

    pub fn bump_reload(&self) {
        let mut c = self.reload_counter;
        let next = *c.read() + 1;
        c.set(next);
    }

    pub fn toasts(&self) -> Signal<Vec<Toast>> {
        self.toasts
    }

    pub fn is_pending(&self, key: &MutationKey) -> bool {
        self.mutations.read().is_pending(key)
    }

    pub fn state(&self, key: &MutationKey) -> Option<MutationState> {
        self.mutations.read().get(key).cloned()
    }

    /// Force a mutation entry into a specific state. Used by the `/design`
    /// sandbox to demo `ActionButton`'s pending and error variants without
    /// firing a real API call.
    pub fn force_state(&self, key: MutationKey, state: MutationState) {
        match state {
            MutationState::Pending => self.mutations.clone().write().set_pending(key),
            MutationState::Done => self.mutations.clone().write().set_done(key),
            MutationState::Error {
                category,
                title,
                detail,
            } => self
                .mutations
                .clone()
                .write()
                .set_error(key, category, title, detail),
        }
    }

    /// Wrap an API call: record `Pending`, await, then record `Done` or
    /// categorized `Error`. Transient/System errors also push a toast.
    /// Returns the future's `Ok` value so callers can announce success.
    pub async fn mutate<F, T>(self, key: MutationKey, fut: F) -> Result<T, MutationFailure>
    where
        F: Future<Output = Result<T, MutationFailure>>,
    {
        self.mutations.clone().write().set_pending(key.clone());
        match fut.await {
            Ok(value) => {
                self.mutations.clone().write().set_done(key);
                self.bump_reload();
                Ok(value)
            }
            Err(failure) => {
                let state = MutationState::from_failure(failure.status, &failure.body);
                if let MutationState::Error {
                    category,
                    title,
                    detail,
                } = &state
                {
                    self.mutations.clone().write().set_error(
                        key,
                        *category,
                        title.clone(),
                        detail.clone(),
                    );
                    if !matches!(category, MutationCategory::Validation) {
                        self.push_toast(ToastKind::Error, title.clone(), detail.clone());
                    }
                }
                Err(failure)
            }
        }
    }

    pub fn push_success(&self, title: impl Into<String>, detail: impl Into<String>) {
        self.push_toast(ToastKind::Success, title.into(), detail.into());
    }

    fn push_toast(&self, kind: ToastKind, title: String, detail: String) {
        let mut id_signal = self.next_toast_id;
        let id = *id_signal.read();
        id_signal.set(id + 1);
        self.toasts.clone().write().push(Toast {
            id,
            kind,
            title,
            detail,
        });
    }

    pub fn dismiss_toast(&self, id: u64) {
        self.toasts.clone().write().retain(|t| t.id != id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_afk_contracts::{
        AssignmentAttemptResponse, IssueAssignmentResponse, IssueSource, IssueSourceCandidate,
        IssueSourceSyncResponse, PlanningSnapshotResponse, ProjectResponse,
    };

    fn planning_snapshot_with_source(locator: &str) -> PlanningSnapshotResponse {
        PlanningSnapshotResponse {
            source: IssueSource {
                kind: "github".to_string(),
                locator: locator.to_string(),
            },
            last_successful_sync_at: None,
            last_failure: None,
            non_ready: vec![],
            dependency_blocked: vec![],
            active: vec![],
            completed: vec![],
            eligible: vec![],
        }
    }

    fn assignment(id: &str, status: &str) -> IssueAssignmentResponse {
        IssueAssignmentResponse {
            id: id.to_string(),
            project_id: ProjectId("p".to_string()),
            source_id: "src".to_string(),
            source_title: "t".to_string(),
            branch: "b".to_string(),
            worktree_path: "/w".to_string(),
            status: status.to_string(),
            status_detail: None,
            latest_attempt: None,
            plan_run_id: None,
            selection_summary: None,
            phase_outputs: vec![],
            review_rejection_count: 0,
            block_reason: None,
        }
    }

    fn activity_entry(id: &str, kind: &str) -> ProjectActivityEntryResponse {
        ProjectActivityEntryResponse {
            id: id.to_string(),
            project_id: "p".to_string(),
            assignment_id: None,
            kind: kind.to_string(),
            detail: None,
            recorded_at: "0".to_string(),
        }
    }

    fn snapshot_with_activity(activity: Vec<ProjectActivityEntryResponse>) -> ProjectSnapshot {
        ProjectSnapshot {
            project: ProjectResponse {
                id: ProjectId("p".to_string()),
                path: String::new(),
                trusted: true,
                git_summary: None,
                enabled_issue_source: None,
            },
            planning_snapshot: None,
            activity,
            issue_source_candidates: vec![],
            execution_config: None,
            active_plan_run: None,
            recent_plan_runs: vec![],
        }
    }

    #[test]
    fn sequential_event_merges_and_bumps_seq() {
        let mut state = ProjectStoreState::new();
        state.last_seen_seq = 5;

        let outcome = state.apply_event(6, ProjectEvent::Activity(activity_entry("a1", "started")));

        assert_eq!(outcome, ApplyOutcome::Merged);
        assert_eq!(state.last_seen_seq, 6);
        assert_eq!(state.activity.len(), 1);
        assert_eq!(state.activity[0].id, "a1");
        assert!(!state.needs_rehydrate);
    }

    #[test]
    fn stale_event_is_ignored() {
        let mut state = ProjectStoreState::new();
        state.last_seen_seq = 5;
        state.activity.push(activity_entry("a-old", "k"));

        let outcome = state.apply_event(5, ProjectEvent::Activity(activity_entry("dup", "k")));

        assert_eq!(outcome, ApplyOutcome::Stale);
        assert_eq!(state.last_seen_seq, 5);
        assert_eq!(state.activity.len(), 1);
        assert_eq!(state.activity[0].id, "a-old");
        assert!(!state.needs_rehydrate);
    }

    #[test]
    fn sequence_gap_triggers_rehydrate_without_merge() {
        let mut state = ProjectStoreState::new();
        state.last_seen_seq = 5;

        let outcome = state.apply_event(8, ProjectEvent::Activity(activity_entry("a", "k")));

        assert_eq!(outcome, ApplyOutcome::Rehydrate);
        assert!(state.needs_rehydrate);
        assert_eq!(state.last_seen_seq, 5);
        assert!(state.activity.is_empty());
    }

    #[test]
    fn resync_event_triggers_rehydrate() {
        let mut state = ProjectStoreState::new();
        state.last_seen_seq = 10;

        let outcome = state.apply_event(99, ProjectEvent::Resync);

        assert_eq!(outcome, ApplyOutcome::Rehydrate);
        assert!(state.needs_rehydrate);
        assert_eq!(state.last_seen_seq, 10);
    }

    fn attempt(id: &str, kind: &str) -> AssignmentAttemptResponse {
        AssignmentAttemptResponse {
            id: id.to_string(),
            kind: kind.to_string(),
            process_id: None,
            process_identity: None,
            terminal_outcome: None,
        }
    }

    #[test]
    fn assignment_attempt_added_updates_latest_attempt_in_active_plan_run() {
        let mut state = ProjectStoreState::new();
        let mut plan_run = plan_run_response("pr1", "running");
        plan_run.assignments = vec![assignment("assn-1", "implementing")];
        state.active_plan_run = Some(plan_run);

        let outcome = state.apply_event(
            1,
            ProjectEvent::AssignmentAttemptAdded {
                assignment_id: "assn-1".to_string(),
                attempt: attempt("att-1", "initial"),
            },
        );

        assert_eq!(outcome, ApplyOutcome::Merged);
        let plan_run = state.active_plan_run.as_ref().unwrap();
        assert_eq!(
            plan_run.assignments[0].latest_attempt.as_ref().unwrap().id,
            "att-1"
        );
    }

    #[test]
    fn planning_snapshot_changed_replaces_planning_snapshot() {
        let mut state = ProjectStoreState::new();
        state.planning_snapshot = Some(planning_snapshot_with_source("old/repo"));

        let outcome = state.apply_event(
            1,
            ProjectEvent::PlanningSnapshotChanged {
                snapshot: Some(planning_snapshot_with_source("new/repo")),
            },
        );

        assert_eq!(outcome, ApplyOutcome::Merged);
        assert_eq!(
            state.planning_snapshot.as_ref().unwrap().source.locator,
            "new/repo"
        );
    }

    #[test]
    fn issue_source_sync_started_sets_sync_in_progress() {
        let mut state = ProjectStoreState::new();

        let outcome = state.apply_event(1, ProjectEvent::IssueSourceSyncStarted);

        assert_eq!(outcome, ApplyOutcome::Merged);
        assert!(state.sync_in_progress);
    }

    #[test]
    fn issue_source_sync_completed_clears_progress_and_updates_metadata() {
        let mut state = ProjectStoreState::new();
        state.sync_in_progress = true;
        state.planning_snapshot = Some(planning_snapshot_with_source("acme/repo"));

        let outcome = state.apply_event(
            1,
            ProjectEvent::IssueSourceSyncCompleted(IssueSourceSyncResponse {
                source: IssueSource {
                    kind: "github".to_string(),
                    locator: "acme/repo".to_string(),
                },
                last_successful_sync_at: Some("2026-05-22T10:00:00Z".to_string()),
                last_failure: None,
            }),
        );

        assert_eq!(outcome, ApplyOutcome::Merged);
        assert!(!state.sync_in_progress);
        let p = state.planning_snapshot.as_ref().unwrap();
        assert_eq!(
            p.last_successful_sync_at.as_deref(),
            Some("2026-05-22T10:00:00Z")
        );
    }

    #[test]
    fn issue_source_sync_failed_clears_progress_and_sets_last_failure() {
        let mut state = ProjectStoreState::new();
        state.sync_in_progress = true;
        state.planning_snapshot = Some(planning_snapshot_with_source("acme/repo"));

        let outcome = state.apply_event(
            1,
            ProjectEvent::IssueSourceSyncFailed {
                error: "401 Unauthorized".to_string(),
            },
        );

        assert_eq!(outcome, ApplyOutcome::Merged);
        assert!(!state.sync_in_progress);
        assert_eq!(
            state
                .planning_snapshot
                .as_ref()
                .unwrap()
                .last_failure
                .as_deref(),
            Some("401 Unauthorized")
        );
    }

    #[test]
    fn issue_source_candidates_changed_replaces_candidates() {
        let mut state = ProjectStoreState::new();
        state.issue_source_candidates = vec![IssueSourceCandidate {
            kind: "github".into(),
            locator: "old/repo".into(),
            enabled: false,
        }];

        let outcome = state.apply_event(
            1,
            ProjectEvent::IssueSourceCandidatesChanged {
                candidates: vec![IssueSourceCandidate {
                    kind: "github".into(),
                    locator: "acme/repo".into(),
                    enabled: true,
                }],
            },
        );

        assert_eq!(outcome, ApplyOutcome::Merged);
        assert_eq!(state.issue_source_candidates.len(), 1);
        assert_eq!(state.issue_source_candidates[0].locator, "acme/repo");
    }

    #[test]
    fn hydrate_populates_project_planning_and_candidates() {
        let mut state = ProjectStoreState::new();
        let snapshot = ProjectSnapshot {
            project: ProjectResponse {
                id: ProjectId("p".to_string()),
                path: "/p".to_string(),
                trusted: true,
                git_summary: None,
                enabled_issue_source: Some(IssueSource {
                    kind: "github".into(),
                    locator: "acme/repo".into(),
                }),
            },
            planning_snapshot: Some(planning_snapshot_with_source("acme/repo")),
            activity: vec![activity_entry("a-1", "started")],
            issue_source_candidates: vec![IssueSourceCandidate {
                kind: "github".into(),
                locator: "acme/repo".into(),
                enabled: true,
            }],
            execution_config: None,
            active_plan_run: None,
            recent_plan_runs: vec![],
        };

        state.hydrate(snapshot, 42);

        assert_eq!(state.last_seen_seq, 42);
        assert!(state.project.is_some());
        assert_eq!(state.activity.len(), 1);
        assert_eq!(state.issue_source_candidates.len(), 1);
    }

    #[test]
    fn hydrate_then_merge_produces_combined_state() {
        let mut state = ProjectStoreState::new();
        state.hydrate(
            snapshot_with_activity(vec![activity_entry("a-old-1", "started")]),
            7,
        );

        let _ = state.apply_event(8, ProjectEvent::Activity(activity_entry("a-new-1", "k")));

        assert_eq!(state.last_seen_seq, 8);
        let ids: Vec<&str> = state.activity.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["a-new-1", "a-old-1"]);
    }

    #[test]
    fn parses_validation_error_from_problem_json_4xx() {
        let body = r#"{"type":"about:blank","title":"Untrusted Project","status":422,"detail":"Project must be trusted before assignment"}"#;

        let state = MutationState::from_failure(Some(422), body);

        assert_eq!(
            state,
            MutationState::Error {
                category: MutationCategory::Validation,
                title: "Untrusted Project".to_string(),
                detail: "Project must be trusted before assignment".to_string(),
            }
        );
    }

    #[test]
    fn parses_transient_error_from_5xx() {
        let body = r#"{"type":"about:blank","title":"Internal error","status":500,"detail":"db unavailable"}"#;

        let state = MutationState::from_failure(Some(500), body);

        match state {
            MutationState::Error { category, .. } => {
                assert_eq!(category, MutationCategory::Transient);
            }
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[test]
    fn parses_transient_error_from_network_failure() {
        let state = MutationState::from_failure(None, "tcp closed");

        match state {
            MutationState::Error { category, .. } => {
                assert_eq!(category, MutationCategory::Transient);
            }
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[test]
    fn pending_then_done_transition() {
        let mut table = MutationsTable::new();
        let key = MutationKey::TrustProject(ProjectId("p1".to_string()));

        table.set_pending(key.clone());
        table.set_done(key.clone());

        assert_eq!(table.get(&key), Some(&MutationState::Done));
        assert!(!table.is_pending(&key));
    }

    #[test]
    fn pending_then_error_transition() {
        let mut table = MutationsTable::new();
        let key = MutationKey::TrustProject(ProjectId("p1".to_string()));

        table.set_pending(key.clone());
        table.set_error(
            key.clone(),
            MutationCategory::Validation,
            "bad".into(),
            "nope".into(),
        );

        assert!(matches!(
            table.get(&key),
            Some(MutationState::Error {
                category: MutationCategory::Validation,
                ..
            })
        ));
    }

    #[test]
    fn enable_issue_source_keys_distinguish_by_kind_and_locator() {
        let mut table = MutationsTable::new();
        let project = ProjectId("p1".to_string());
        let github =
            MutationKey::EnableIssueSource(project.clone(), "github".into(), "acme/repo".into());
        let local = MutationKey::EnableIssueSource(
            project,
            "local_markdown".into(),
            ".scratch/issues".into(),
        );

        table.set_pending(github.clone());

        assert!(table.is_pending(&github));
        assert!(!table.is_pending(&local));
    }

    #[test]
    fn sync_issue_source_key_tracked_independently_from_trust() {
        let mut table = MutationsTable::new();
        let project = ProjectId("p1".to_string());
        let trust = MutationKey::TrustProject(project.clone());
        let sync = MutationKey::SyncIssueSource(project);

        table.set_pending(sync.clone());

        assert!(table.is_pending(&sync));
        assert!(!table.is_pending(&trust));
    }

    #[test]
    fn assignment_keys_are_distinct_by_variant_and_id() {
        let project = ProjectId("p1".to_string());
        let assignment_a = IssueAssignmentId("assn-A".to_string());
        let assignment_b = IssueAssignmentId("assn-B".to_string());

        let mut table = MutationsTable::new();
        let re_enable_a = MutationKey::ReEnableAssignment(project.clone(), assignment_a.clone());
        let re_enable_b = MutationKey::ReEnableAssignment(project, assignment_b);

        table.set_pending(re_enable_a.clone());
        assert!(table.is_pending(&re_enable_a));
        assert!(!table.is_pending(&re_enable_b));
    }

    // --- Plan Run store behavior (issue #41) ---

    fn plan_run_response(id: &str, state: &str) -> agentic_afk_contracts::PlanRunResponse {
        agentic_afk_contracts::PlanRunResponse {
            id: id.to_string(),
            project_id: ProjectId("p".to_string()),
            integration_branch: "main".to_string(),
            baseline_commit: "abc".to_string(),
            state: state.to_string(),
            started_at: "0".to_string(),
            finished_at: None,
            phase_outputs: vec![],
            assignments: vec![],
        }
    }

    fn phase_output(phase: &str, outcome: &str) -> agentic_afk_contracts::PhaseOutputResponse {
        agentic_afk_contracts::PhaseOutputResponse {
            phase: phase.to_string(),
            outcome: outcome.to_string(),
            body_json: serde_json::json!({"issues":[]}),
            recorded_at: "0".to_string(),
            assignment_id: None,
        }
    }

    fn execution_config() -> agentic_afk_contracts::ProjectExecutionConfigResponse {
        agentic_afk_contracts::ProjectExecutionConfigResponse {
            integration_branch: "main".to_string(),
            max_parallel_tasks: 4,
            review_retry_limit: 2,
        }
    }

    #[test]
    fn hydrate_populates_plan_run_fields() {
        let mut state = ProjectStoreState::new();
        let mut snapshot = snapshot_with_activity(vec![]);
        snapshot.execution_config = Some(execution_config());
        snapshot.active_plan_run = Some(plan_run_response("pr1", "running"));
        snapshot.recent_plan_runs = vec![plan_run_response("pr0", "succeeded_empty")];
        state.hydrate(snapshot, 5);
        assert!(state.execution_config.is_some());
        assert_eq!(state.active_plan_run.as_ref().unwrap().id, "pr1");
        assert_eq!(state.recent_plan_runs.len(), 1);
        assert_eq!(state.recent_plan_runs[0].id, "pr0");
    }

    #[test]
    fn plan_run_started_sets_active_plan_run() {
        let mut state = ProjectStoreState::new();
        state.hydrate(snapshot_with_activity(vec![]), 0);
        let outcome = state.apply_event(
            1,
            ProjectEvent::PlanRunStarted(plan_run_response("pr1", "running")),
        );
        assert_eq!(outcome, ApplyOutcome::Merged);
        assert_eq!(state.active_plan_run.as_ref().unwrap().id, "pr1");
    }

    #[test]
    fn plan_run_phase_completed_appends_phase_output_to_matching_active_plan_run() {
        let mut state = ProjectStoreState::new();
        state.hydrate(snapshot_with_activity(vec![]), 0);
        state.apply_event(
            1,
            ProjectEvent::PlanRunStarted(plan_run_response("pr1", "running")),
        );
        let outcome = state.apply_event(
            2,
            ProjectEvent::PlanRunPhaseCompleted {
                plan_run_id: "pr1".to_string(),
                phase_output: phase_output("planning", "succeeded_empty"),
            },
        );
        assert_eq!(outcome, ApplyOutcome::Merged);
        let active = state.active_plan_run.as_ref().unwrap();
        assert_eq!(active.phase_outputs.len(), 1);
        assert_eq!(active.phase_outputs[0].phase, "planning");
    }

    #[test]
    fn plan_run_completed_moves_active_into_recent_history() {
        let mut state = ProjectStoreState::new();
        state.hydrate(snapshot_with_activity(vec![]), 0);
        state.apply_event(
            1,
            ProjectEvent::PlanRunStarted(plan_run_response("pr1", "running")),
        );
        let mut completed = plan_run_response("pr1", "succeeded_empty");
        completed.finished_at = Some("1".to_string());
        state.apply_event(2, ProjectEvent::PlanRunCompleted(completed));
        assert!(state.active_plan_run.is_none());
        assert_eq!(state.recent_plan_runs.len(), 1);
        assert_eq!(state.recent_plan_runs[0].state, "succeeded_empty");
    }

    #[test]
    fn assignment_created_with_plan_run_id_mirrors_into_active_plan_run() {
        let mut state = ProjectStoreState::new();
        state.hydrate(snapshot_with_activity(vec![]), 0);
        state.apply_event(
            1,
            ProjectEvent::PlanRunStarted(plan_run_response("pr1", "running")),
        );
        let mut assignment = assignment("a1", "claimed");
        assignment.plan_run_id = Some("pr1".to_string());
        assignment.selection_summary = Some("baseline ready".to_string());
        let outcome = state.apply_event(2, ProjectEvent::AssignmentCreated(assignment));
        assert_eq!(outcome, ApplyOutcome::Merged);
        let active = state.active_plan_run.as_ref().unwrap();
        assert_eq!(active.assignments.len(), 1);
        assert_eq!(active.assignments[0].id, "a1");
        assert_eq!(
            active.assignments[0].selection_summary.as_deref(),
            Some("baseline ready")
        );
    }

    #[test]
    fn assignment_status_changed_mirrors_phase_outputs_into_active_plan_run() {
        let mut state = ProjectStoreState::new();
        state.hydrate(snapshot_with_activity(vec![]), 0);
        state.apply_event(
            1,
            ProjectEvent::PlanRunStarted(plan_run_response("pr1", "running")),
        );
        let mut created = assignment("a1", "claimed");
        created.plan_run_id = Some("pr1".to_string());
        state.apply_event(2, ProjectEvent::AssignmentCreated(created));

        // Status change carries an updated assignment with new phase outputs;
        // the store must mirror it into the active Plan Run.
        let mut reviewed = assignment("a1", "reviewed");
        reviewed.plan_run_id = Some("pr1".to_string());
        reviewed.phase_outputs = vec![
            agentic_afk_contracts::PhaseOutputResponse {
                phase: "implementation".to_string(),
                outcome: "ready_for_review".to_string(),
                body_json: serde_json::json!({}),
                recorded_at: "0".to_string(),
                assignment_id: Some("a1".to_string()),
            },
            agentic_afk_contracts::PhaseOutputResponse {
                phase: "review".to_string(),
                outcome: "approved".to_string(),
                body_json: serde_json::json!({}),
                recorded_at: "0".to_string(),
                assignment_id: Some("a1".to_string()),
            },
        ];
        state.apply_event(3, ProjectEvent::AssignmentStatusChanged(reviewed));
        let active = state.active_plan_run.as_ref().unwrap();
        assert_eq!(active.assignments.len(), 1);
        assert_eq!(active.assignments[0].status, "reviewed");
        assert_eq!(active.assignments[0].phase_outputs.len(), 2);
        assert_eq!(active.assignments[0].phase_outputs[1].outcome, "approved");
    }

    #[test]
    fn execution_config_changed_updates_state() {
        let mut state = ProjectStoreState::new();
        state.hydrate(snapshot_with_activity(vec![]), 0);
        let outcome = state.apply_event(
            1,
            ProjectEvent::ProjectExecutionConfigChanged(execution_config()),
        );
        assert_eq!(outcome, ApplyOutcome::Merged);
        let cfg = state.execution_config.as_ref().unwrap();
        assert_eq!(cfg.integration_branch, "main");
        assert_eq!(cfg.max_parallel_tasks, 4);
        assert_eq!(cfg.review_retry_limit, 2);
    }
}
