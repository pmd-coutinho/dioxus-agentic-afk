//! Reactive Project store + mutation lifecycle. See ADR-0032.

use std::collections::HashMap;
use std::future::Future;

use agentic_afk_contracts::{
    IssueSourceCandidate, PlanningSnapshotResponse, ProblemDetail, ProjectActivityEntryResponse,
    ProjectAssignmentStateResponse, ProjectEvent, ProjectId, ProjectResponse, ProjectSnapshot,
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
    pub assignment_state: Option<ProjectAssignmentStateResponse>,
    pub planning_snapshot: Option<PlanningSnapshotResponse>,
    pub issue_source_candidates: Vec<IssueSourceCandidate>,
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
        self.assignment_state = Some(snapshot.assignment_state);
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
                let waiting = self
                    .assignment_state
                    .as_ref()
                    .map(|s| s.waiting_ready_issue_count)
                    .unwrap_or(0);
                // Terminal statuses ("abandoned", "completed") release the
                // Project assignment slot on the server, so the store clears
                // its active assignment to match. Any other status keeps the
                // assignment active in the slot.
                let active = if matches!(assignment.status.as_str(), "abandoned" | "completed") {
                    None
                } else {
                    Some(assignment)
                };
                self.assignment_state = Some(ProjectAssignmentStateResponse {
                    active_assignment: active,
                    waiting_ready_issue_count: waiting,
                });
            }
            ProjectEvent::AssignmentAttemptAdded {
                assignment_id,
                attempt,
            } => {
                if let Some(state) = self.assignment_state.as_mut() {
                    if let Some(active) = state.active_assignment.as_mut() {
                        if active.id == assignment_id {
                            active.latest_attempt = Some(attempt);
                        }
                    }
                }
            }
            ProjectEvent::ChangeProposalRefreshed {
                assignment_id,
                change_proposal,
            }
            | ProjectEvent::ChangeProposalVerified {
                assignment_id,
                change_proposal,
            } => {
                if let Some(state) = self.assignment_state.as_mut() {
                    if let Some(active) = state.active_assignment.as_mut() {
                        if active.id == assignment_id {
                            active.change_proposal = Some(change_proposal);
                        }
                    }
                }
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
    StartAssignment(ProjectId, SourceIssueId),
    AbandonAssignment(ProjectId, IssueAssignmentId),
    RecoverAssignment(ProjectId, IssueAssignmentId),
    RefreshProposalState(ProjectId, IssueAssignmentId),
    SyncIssueSource(ProjectId),
    EnableIssueSource(ProjectId, String, String),
}

/// Identifier for a Source Issue (the upstream issue tracker's issue id).
///
/// Distinct from `IssueAssignmentId` so the type system rejects mistakes
/// like keying a Start Assignment mutation by an assignment id.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SourceIssueId(pub String);

/// Identifier for an Issue Assignment (a single attempt to land a Source
/// Issue), distinct from `SourceIssueId`.
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
        AssignmentAttemptResponse, ChangeProposalResponse, IssueAssignmentResponse, IssueSource,
        IssueSourceCandidate, IssueSourceSyncResponse, PlanningSnapshotResponse,
        ProjectAssignmentStateResponse, ProjectResponse,
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
            blocked: vec![],
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
            change_proposal: None,
            latest_attempt: None,
            repair_budget: None,
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
            assignment_state: ProjectAssignmentStateResponse {
                active_assignment: None,
                waiting_ready_issue_count: 0,
            },
            activity,
            issue_source_candidates: vec![],
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

    #[test]
    fn assignment_created_event_sets_active_assignment() {
        let mut state = ProjectStoreState::new();
        state.last_seen_seq = 3;

        let outcome = state.apply_event(
            4,
            ProjectEvent::AssignmentCreated(assignment("assn-1", "running")),
        );

        assert_eq!(outcome, ApplyOutcome::Merged);
        assert_eq!(state.last_seen_seq, 4);
        let active = state
            .assignment_state
            .as_ref()
            .and_then(|s| s.active_assignment.as_ref())
            .expect("active assignment set");
        assert_eq!(active.id, "assn-1");
        assert_eq!(active.status, "running");
    }

    #[test]
    fn assignment_status_changed_replaces_active_assignment() {
        let mut state = ProjectStoreState::new();
        state.last_seen_seq = 4;
        state.assignment_state = Some(ProjectAssignmentStateResponse {
            active_assignment: Some(assignment("assn-1", "running")),
            waiting_ready_issue_count: 2,
        });

        let outcome = state.apply_event(
            5,
            ProjectEvent::AssignmentStatusChanged(assignment("assn-1", "proposal_pending")),
        );

        assert_eq!(outcome, ApplyOutcome::Merged);
        let s = state.assignment_state.as_ref().unwrap();
        assert_eq!(s.waiting_ready_issue_count, 2);
        assert_eq!(s.active_assignment.as_ref().unwrap().status, "proposal_pending");
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
    fn assignment_attempt_added_sets_latest_attempt_when_assignment_matches() {
        let mut state = ProjectStoreState::new();
        state.last_seen_seq = 4;
        state.assignment_state = Some(ProjectAssignmentStateResponse {
            active_assignment: Some(assignment("assn-1", "running")),
            waiting_ready_issue_count: 0,
        });

        let outcome = state.apply_event(
            5,
            ProjectEvent::AssignmentAttemptAdded {
                assignment_id: "assn-1".to_string(),
                attempt: attempt("att-1", "initial"),
            },
        );

        assert_eq!(outcome, ApplyOutcome::Merged);
        let active = state
            .assignment_state
            .as_ref()
            .and_then(|s| s.active_assignment.as_ref())
            .unwrap();
        assert_eq!(active.latest_attempt.as_ref().unwrap().id, "att-1");
    }

    #[test]
    fn change_proposal_refreshed_updates_proposal_when_assignment_matches() {
        let mut state = ProjectStoreState::new();
        state.last_seen_seq = 1;
        state.assignment_state = Some(ProjectAssignmentStateResponse {
            active_assignment: Some(assignment("assn-1", "proposal_pending")),
            waiting_ready_issue_count: 0,
        });

        let outcome = state.apply_event(
            2,
            ProjectEvent::ChangeProposalRefreshed {
                assignment_id: "assn-1".to_string(),
                change_proposal: ChangeProposalResponse {
                    status: "pending".to_string(),
                    url: "https://x/pr/1".to_string(),
                },
            },
        );

        assert_eq!(outcome, ApplyOutcome::Merged);
        let active = state
            .assignment_state
            .as_ref()
            .and_then(|s| s.active_assignment.as_ref())
            .unwrap();
        let proposal = active.change_proposal.as_ref().unwrap();
        assert_eq!(proposal.status, "pending");
        assert_eq!(proposal.url, "https://x/pr/1");
    }

    #[test]
    fn change_proposal_verified_updates_proposal_to_verified_status() {
        let mut state = ProjectStoreState::new();
        state.last_seen_seq = 1;
        let mut assn = assignment("assn-1", "proposal_pending");
        assn.change_proposal = Some(ChangeProposalResponse {
            status: "pending".to_string(),
            url: "https://x/pr/1".to_string(),
        });
        state.assignment_state = Some(ProjectAssignmentStateResponse {
            active_assignment: Some(assn),
            waiting_ready_issue_count: 0,
        });

        let outcome = state.apply_event(
            2,
            ProjectEvent::ChangeProposalVerified {
                assignment_id: "assn-1".to_string(),
                change_proposal: ChangeProposalResponse {
                    status: "verified".to_string(),
                    url: "https://x/pr/1".to_string(),
                },
            },
        );

        assert_eq!(outcome, ApplyOutcome::Merged);
        let proposal = state
            .assignment_state
            .as_ref()
            .and_then(|s| s.active_assignment.as_ref())
            .and_then(|a| a.change_proposal.as_ref())
            .unwrap();
        assert_eq!(proposal.status, "verified");
    }

    #[test]
    fn planning_snapshot_changed_replaces_planning_snapshot() {
        let mut state = ProjectStoreState::new();
        state.last_seen_seq = 0;
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
        state.last_seen_seq = 0;

        let outcome = state.apply_event(1, ProjectEvent::IssueSourceSyncStarted);

        assert_eq!(outcome, ApplyOutcome::Merged);
        assert!(state.sync_in_progress);
    }

    #[test]
    fn issue_source_sync_completed_clears_progress_and_updates_metadata() {
        let mut state = ProjectStoreState::new();
        state.last_seen_seq = 1;
        state.sync_in_progress = true;
        state.planning_snapshot = Some(planning_snapshot_with_source("acme/repo"));

        let outcome = state.apply_event(
            2,
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
        assert!(p.last_failure.is_none());
    }

    #[test]
    fn issue_source_sync_failed_clears_progress_and_sets_last_failure() {
        let mut state = ProjectStoreState::new();
        state.last_seen_seq = 1;
        state.sync_in_progress = true;
        state.planning_snapshot = Some(planning_snapshot_with_source("acme/repo"));

        let outcome = state.apply_event(
            2,
            ProjectEvent::IssueSourceSyncFailed {
                error: "401 Unauthorized".to_string(),
            },
        );

        assert_eq!(outcome, ApplyOutcome::Merged);
        assert!(!state.sync_in_progress);
        assert_eq!(
            state.planning_snapshot.as_ref().unwrap().last_failure.as_deref(),
            Some("401 Unauthorized")
        );
    }

    #[test]
    fn issue_source_candidates_changed_replaces_candidates() {
        let mut state = ProjectStoreState::new();
        state.last_seen_seq = 0;
        state.issue_source_candidates = vec![IssueSourceCandidate {
            kind: "github".into(),
            locator: "old/repo".into(),
            enabled: false,
        }];

        let outcome = state.apply_event(
            1,
            ProjectEvent::IssueSourceCandidatesChanged {
                candidates: vec![
                    IssueSourceCandidate {
                        kind: "github".into(),
                        locator: "acme/repo".into(),
                        enabled: true,
                    },
                    IssueSourceCandidate {
                        kind: "local_markdown".into(),
                        locator: ".scratch/issues".into(),
                        enabled: false,
                    },
                ],
            },
        );

        assert_eq!(outcome, ApplyOutcome::Merged);
        assert_eq!(state.issue_source_candidates.len(), 2);
        assert_eq!(state.issue_source_candidates[0].locator, "acme/repo");
        assert!(state.issue_source_candidates[0].enabled);
    }

    #[test]
    fn hydrate_populates_project_assignment_planning_and_candidates() {
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
            assignment_state: ProjectAssignmentStateResponse {
                active_assignment: Some(assignment("assn-1", "running")),
                waiting_ready_issue_count: 3,
            },
            activity: vec![activity_entry("a-1", "started")],
            issue_source_candidates: vec![IssueSourceCandidate {
                kind: "github".into(),
                locator: "acme/repo".into(),
                enabled: true,
            }],
        };

        state.hydrate(snapshot, 42);

        assert_eq!(state.last_seen_seq, 42);
        assert!(state.project.is_some());
        assert_eq!(state.project.as_ref().unwrap().path, "/p");
        assert_eq!(
            state.assignment_state.as_ref().unwrap().waiting_ready_issue_count,
            3
        );
        assert_eq!(
            state.planning_snapshot.as_ref().unwrap().source.locator,
            "acme/repo"
        );
        assert_eq!(state.activity.len(), 1);
        assert_eq!(state.issue_source_candidates.len(), 1);
    }

    #[test]
    fn assignment_status_changed_to_abandoned_clears_active_assignment() {
        let mut state = ProjectStoreState::new();
        state.last_seen_seq = 0;
        state.assignment_state = Some(ProjectAssignmentStateResponse {
            active_assignment: Some(assignment("assn-1", "running")),
            waiting_ready_issue_count: 0,
        });

        let outcome = state.apply_event(
            1,
            ProjectEvent::AssignmentStatusChanged(assignment("assn-1", "abandoned")),
        );

        assert_eq!(outcome, ApplyOutcome::Merged);
        assert!(
            state
                .assignment_state
                .as_ref()
                .unwrap()
                .active_assignment
                .is_none()
        );
    }

    #[test]
    fn hydrate_then_merge_produces_combined_state() {
        let mut state = ProjectStoreState::new();
        let snapshot_activity = vec![
            activity_entry("a-old-1", "started"),
            activity_entry("a-old-2", "started"),
        ];
        state.hydrate(snapshot_with_activity(snapshot_activity), 7);

        let _ = state.apply_event(8, ProjectEvent::Activity(activity_entry("a-new-1", "k")));
        let _ = state.apply_event(9, ProjectEvent::Activity(activity_entry("a-new-2", "k")));

        assert_eq!(state.last_seen_seq, 9);
        assert!(!state.needs_rehydrate);
        let ids: Vec<&str> = state.activity.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["a-new-2", "a-new-1", "a-old-1", "a-old-2"]);
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
            MutationState::Error {
                category,
                title,
                detail,
            } => {
                assert_eq!(category, MutationCategory::Transient);
                assert_eq!(title, "Request failed");
                assert_eq!(detail, "tcp closed");
            }
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[test]
    fn pending_state_recorded_after_starting_mutation() {
        let mut table = MutationsTable::new();
        let key = MutationKey::TrustProject(ProjectId("p1".to_string()));

        table.set_pending(key.clone());

        assert_eq!(table.get(&key), Some(&MutationState::Pending));
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
        assert!(!table.is_pending(&key));
    }

    #[test]
    fn assignment_keys_are_distinct_by_variant_and_id() {
        let project = ProjectId("p1".to_string());
        let source = SourceIssueId("issue-A".to_string());
        let assignment = IssueAssignmentId("assn-A".to_string());

        let mut table = MutationsTable::new();
        let start = MutationKey::StartAssignment(project.clone(), source.clone());
        let abandon = MutationKey::AbandonAssignment(project.clone(), assignment.clone());
        let recover = MutationKey::RecoverAssignment(project.clone(), assignment.clone());
        let refresh = MutationKey::RefreshProposalState(project.clone(), assignment.clone());

        table.set_pending(start.clone());

        assert!(table.is_pending(&start));
        assert!(!table.is_pending(&abandon));
        assert!(!table.is_pending(&recover));
        assert!(!table.is_pending(&refresh));

        // Same project + assignment id but different variant must not collide.
        table.set_pending(abandon.clone());
        assert!(table.is_pending(&abandon));
        assert!(!table.is_pending(&recover));
        assert!(!table.is_pending(&refresh));
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
    fn enable_issue_source_keys_distinguish_by_kind_and_locator() {
        let mut table = MutationsTable::new();
        let project = ProjectId("p1".to_string());
        let github = MutationKey::EnableIssueSource(
            project.clone(),
            "github".into(),
            "acme/repo".into(),
        );
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
    fn parallel_keys_tracked_independently() {
        let mut table = MutationsTable::new();
        let a = MutationKey::TrustProject(ProjectId("a".into()));
        let b = MutationKey::TrustProject(ProjectId("b".into()));

        table.set_pending(a.clone());
        table.set_pending(b.clone());
        table.set_done(a.clone());

        assert_eq!(table.get(&a), Some(&MutationState::Done));
        assert_eq!(table.get(&b), Some(&MutationState::Pending));
        assert!(table.is_pending(&b));
        assert!(!table.is_pending(&a));
    }
}
