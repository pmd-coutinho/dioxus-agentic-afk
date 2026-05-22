//! Reactive Project store + mutation lifecycle. See ADR-0032.

use std::collections::HashMap;
use std::future::Future;

use agentic_afk_contracts::{ProblemDetail, ProjectId};
use dioxus::prelude::*;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum MutationKey {
    TrustProject(ProjectId),
    StartAssignment(ProjectId, SourceIssueId),
    AbandonAssignment(ProjectId, IssueAssignmentId),
    RecoverAssignment(ProjectId, IssueAssignmentId),
    RefreshProposalState(ProjectId, IssueAssignmentId),
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
}

impl ProjectStore {
    pub fn new() -> Self {
        Self {
            mutations: Signal::new(MutationsTable::new()),
            toasts: Signal::new(Vec::new()),
            next_toast_id: Signal::new(1),
            reload_counter: Signal::new(0),
        }
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
