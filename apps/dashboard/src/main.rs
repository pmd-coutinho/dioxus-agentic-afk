mod project_store;
mod sse_client;
mod ui;

use project_store::{
    IssueAssignmentId, MutationCategory, MutationKey, MutationState, ProjectStore, SourceIssueId,
};
use ui::{
    ActionButton, ButtonVariant, Card, CardBody, CardFoot, CardHead, EmptyState, EmptyStateAccent,
    ErrorState, HudToastRegion, KeyValueList, KeyValueRow, PillTone, SkeletonHeading, SkeletonLine,
    StatusPill,
};

use agentic_afk_contracts::{
    AppInfoResponse, EnableIssueSourceRequest, GitSummary, IssueSourceCandidate,
    IssueSourceSyncResponse, PlanningSnapshotResponse,
    ProjectActivityEntryResponse, ProjectAssignmentStateResponse, ProjectId, ProjectResponse,
    ProjectEvent, ProjectSnapshotResponse, SourceIssueSnapshot,
};
use dioxus::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

static TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    rsx! {
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }
        document::Link {
            rel: "stylesheet",
            href: "https://fonts.googleapis.com/css2?family=IBM+Plex+Sans+Condensed:wght@400;500;600;700&family=Inter+Tight:wght@400;500;600&family=JetBrains+Mono:wght@400;500&display=swap",
        }
        Router::<Route> {}
    }
}

#[rustfmt::skip]
#[derive(Routable, Clone, Debug, PartialEq)]
enum Route {
    #[layout(AppShell)]
        #[route("/")]
        Home {},
        #[route("/projects")]
        ProjectList {},
        #[nest("/projects/:id")]
            #[layout(ProjectLayout)]
                #[route("")]
                ProjectOverview { id: String },
                #[route("/planning")]
                ProjectPlanning { id: String },
                #[route("/assignment")]
                ProjectAssignment { id: String },
                #[route("/source")]
                ProjectIssueSource { id: String },
                #[route("/activity")]
                ProjectActivity { id: String },
            #[end_layout]
        #[end_nest]
        #[route("/settings")]
        Settings {},
        #[route("/design")]
        DesignSandbox {},
}

#[component]
fn AppShell() -> Element {
    use_context_provider(ProjectStore::new);
    rsx! {
        main { class: "min-h-screen bg-void font-body text-ink",
            div { class: "pointer-events-none fixed inset-0 -z-10",
                style: "background: radial-gradient(ellipse at 20% 0%, rgba(91,233,255,0.10), transparent 60%), radial-gradient(ellipse at 80% 100%, rgba(199,146,255,0.08), transparent 55%);",
            }
            section { class: "mx-auto flex w-full max-w-5xl flex-col gap-6 px-6 py-8",
                header { class: "flex items-end justify-between gap-4 border-b border-stroke pb-5",
                    div { class: "flex flex-col gap-1",
                        p { class: "font-mono text-[11px] uppercase tracking-[0.22em] text-cyan", "Local Control Plane" }
                        Link {
                            to: Route::Home {},
                            class: "w-fit",
                            h1 { class: "font-display text-3xl font-semibold uppercase tracking-[0.12em] text-ink",
                                "agentic-afk"
                            }
                        }
                    }
                    nav { class: "flex items-center gap-4 font-display text-[11px] uppercase tracking-[0.22em]",
                        Link { to: Route::Home {}, class: "text-ink-2 hover:text-cyan", "Home" }
                        Link { to: Route::ProjectList {}, class: "text-ink-2 hover:text-cyan", "Projects" }
                        Link { to: Route::Settings {}, class: "text-ink-2 hover:text-cyan", "Settings" }
                    }
                }
                HudToastRegion {}
                Outlet::<Route> {}
            }
        }
    }
}

#[component]
fn Home() -> Element {
    let app_info = use_resource(fetch_app_info);
    let projects = use_resource(fetch_projects);

    rsx! {
        div { class: "flex flex-col gap-6",
            match &*app_info.read_unchecked() {
                Some(Ok(info)) => rsx! {
                    div { class: "grid gap-6 md:grid-cols-[1.2fr_0.8fr]",
                        ApiConnectedCard { info: info.clone() }
                        SettingsCard { info: info.clone() }
                    }
                },
                Some(Err(error)) => rsx! {
                    ErrorState {
                        title: "API disconnected".to_string(),
                        detail: error.clone(),
                        problem_json: None,
                    }
                },
                None => rsx! { ApiLoadingCard {} },
            }
            ProjectsSection { projects: projects.read_unchecked().clone() }
        }
    }
}

#[component]
fn ProjectList() -> Element {
    let projects = use_resource(fetch_projects);
    rsx! {
        div { class: "flex flex-col gap-6",
            ProjectsSection { projects: projects.read_unchecked().clone() }
        }
    }
}

#[component]
fn Settings() -> Element {
    let app_info = use_resource(fetch_app_info);
    rsx! {
        div { class: "flex flex-col gap-6",
            match &*app_info.read_unchecked() {
                Some(Ok(info)) => rsx! { SettingsCard { info: info.clone() } },
                Some(Err(error)) => rsx! {
                    ErrorState {
                        title: "API disconnected".to_string(),
                        detail: error.clone(),
                        problem_json: None,
                    }
                },
                None => rsx! { SettingsLoadingCard {} },
            }
        }
    }
}

#[component]
fn ApiConnectedCard(info: AppInfoResponse) -> Element {
    rsx! {
        Card {
            CardHead {
                title: "API connected".to_string(),
                id_text: Some(info.version.clone()),
            }
            CardBody {
                div { class: "flex items-center justify-between gap-4",
                    div { class: "flex flex-col gap-1",
                        p { class: "font-display text-[11px] uppercase tracking-[0.22em] text-ink-dim", "Service" }
                        p { class: "font-mono text-[14px] text-ink", "{info.app_name}" }
                    }
                    StatusPill { tone: PillTone::Verified, label: "Connected".to_string() }
                }
            }
        }
    }
}

#[component]
fn ApiLoadingCard() -> Element {
    rsx! {
        Card {
            CardHead {
                title: "API connection".to_string(),
                id_text: Some("Loading\u{2026}".to_string()),
            }
            CardBody {
                SkeletonHeading {}
                SkeletonLine { width_percent: 70 }
                SkeletonLine { width_percent: 45 }
            }
        }
    }
}

#[component]
fn ProjectsSection(projects: Option<Result<Vec<ProjectResponse>, String>>) -> Element {
    rsx! {
        Card {
            CardHead {
                title: "Projects".to_string(),
                id_text: match &projects {
                    Some(Ok(list)) => Some(format!("{}", list.len())),
                    _ => None,
                },
            }
            CardBody {
                match projects {
                    Some(Ok(projects)) if projects.is_empty() => rsx! {
                        EmptyState {
                            title: "No Projects".to_string(),
                            body: "Register a Project path with the Local Control Plane to start.".to_string(),
                            accent: EmptyStateAccent::Cyan,
                        }
                    },
                    Some(Ok(projects)) => rsx! {
                        ul { class: "grid gap-4",
                            for project in projects {
                                ProjectRow { project }
                            }
                        }
                    },
                    Some(Err(error)) => rsx! {
                        ErrorState {
                            title: "Projects unavailable".to_string(),
                            detail: error.clone(),
                            problem_json: None,
                        }
                    },
                    None => rsx! {
                        div { class: "grid gap-2",
                            SkeletonHeading {}
                            SkeletonLine { width_percent: 78 }
                            SkeletonLine { width_percent: 64 }
                        }
                    },
                }
            }
        }
    }
}

#[component]
fn ProjectLayout(id: String) -> Element {
    let project_resource_id = id.clone();
    let store = use_context::<ProjectStore>();
    let reload_counter = store.reload_counter();
    let project = use_resource(move || {
        let _ = reload_counter.read();
        fetch_project(project_resource_id.clone())
    });
    use_project_live_subscription(store, id.clone());

    rsx! {
        div { class: "flex flex-col gap-4",
            match &*project.read_unchecked() {
                Some(Ok(project)) => rsx! {
                    ProjectHeader { project: project.clone() }
                    ProjectSubNav { id: id.clone() }
                    Outlet::<Route> {}
                },
                Some(Err(error)) => rsx! {
                    StatusPanel {
                        title: "Project unavailable".to_string(),
                        detail: error.clone(),
                        tone: "border-red-700 bg-red-950/40 text-red-100".to_string(),
                    }
                },
                None => rsx! {
                    StatusPanel {
                        title: "Loading Project".to_string(),
                        detail: id.clone(),
                        tone: "border-zinc-700 bg-zinc-900 text-zinc-100".to_string(),
                    }
                },
            }
        }
    }
}

/// Hydrate the `ProjectStore` from `/snapshot`, then open an SSE subscription
/// for the duration of the layout's mount. Re-runs when the Project id in the
/// route changes. The `SseSubscription` is held by a hook-owned `RefCell`, so
/// `Drop` closes the `EventSource` automatically on unmount or id swap.
fn use_project_live_subscription(store: ProjectStore, project_id: String) {
    let subscription: Signal<Rc<RefCell<Option<sse_client::SseSubscription>>>> =
        use_hook(|| Signal::new(Rc::new(RefCell::new(None))));
    let last_project_id: Signal<Option<String>> = use_hook(|| Signal::new(None));

    let needs_rehydrate = store.needs_rehydrate();
    let project_changed = last_project_id.read().as_deref() != Some(project_id.as_str());

    if project_changed || needs_rehydrate {
        // Close any existing subscription before re-hydrating.
        subscription.read().borrow_mut().take();
        last_project_id
            .clone()
            .set(Some(project_id.clone()));
        store.clear_rehydrate_flag();

        let project_id_for_task = project_id.clone();
        let subscription_handle = subscription.read().clone();
        spawn(async move {
            let snapshot = match fetch_project_snapshot(project_id_for_task.clone()).await {
                Ok(snapshot) => snapshot,
                Err(_) => return,
            };
            let sequence = snapshot.sequence;
            store.hydrate(snapshot.snapshot, sequence);
            let on_event = move |seq: u64, event: ProjectEvent| {
                store.apply_event(seq, event);
            };
            let sub = sse_client::subscribe(&project_id_for_task, sequence, on_event);
            *subscription_handle.borrow_mut() = Some(sub);
        });
    }
}

#[component]
fn ProjectSubNav(id: String) -> Element {
    rsx! {
        nav { class: "flex flex-wrap gap-3 border-b border-zinc-800 pb-3 text-sm",
            Link {
                to: Route::ProjectOverview { id: id.clone() },
                class: "text-emerald-200 hover:text-emerald-100",
                "Overview"
            }
            Link {
                to: Route::ProjectPlanning { id: id.clone() },
                class: "text-emerald-200 hover:text-emerald-100",
                "Planning"
            }
            Link {
                to: Route::ProjectAssignment { id: id.clone() },
                class: "text-emerald-200 hover:text-emerald-100",
                "Assignment"
            }
            Link {
                to: Route::ProjectIssueSource { id: id.clone() },
                class: "text-emerald-200 hover:text-emerald-100",
                "Issue Source"
            }
            Link {
                to: Route::ProjectActivity { id: id.clone() },
                class: "text-emerald-200 hover:text-emerald-100",
                "Activity"
            }
        }
    }
}

#[component]
fn ProjectHeader(project: ProjectResponse) -> Element {
    let project_id = project.id.0.clone();
    rsx! {
        section { class: "rounded-lg border border-zinc-800 bg-zinc-900 p-5",
            Link {
                to: Route::ProjectList {},
                class: "text-sm text-emerald-200 hover:text-emerald-100",
                "Projects"
            }
            h2 { class: "mt-4 text-base font-semibold", "Project detail" }
            dl { class: "mt-4 grid gap-3 text-sm",
                SettingRow { label: "Project path".to_string(), value: project.path.clone() }
                SettingRow { label: "Project ID".to_string(), value: project.id.0.clone() }
                div { class: "grid gap-1 border-b border-zinc-800 pb-3 last:border-0 last:pb-0",
                    dt { class: "text-zinc-400", "Trust" }
                    dd { class: "break-words font-mono text-zinc-100",
                        if project.trusted {
                            span { class: "text-emerald-300", "Trusted for agent execution" }
                        } else {
                            TrustProjectButton { project_id: project_id.clone() }
                        }
                    }
                }
                match project.enabled_issue_source.clone() {
                    Some(source) => rsx! {
                        SettingRow {
                            label: "Issue Source".to_string(),
                            value: format!("{} {}", source.kind, source.locator),
                        }
                    },
                    None => rsx! {
                        SettingRow {
                            label: "Issue Source".to_string(),
                            value: "Not enabled".to_string(),
                        }
                    },
                }
            }
            div { class: "mt-4",
                match project.git_summary.clone() {
                    Some(summary) => rsx! { GitSummaryRow { summary } },
                    None => rsx! {
                        p { class: "text-sm text-zinc-500", "No Git Summary" }
                    },
                }
            }
        }
    }
}

#[component]
fn TrustProjectButton(project_id: String) -> Element {
    use project_store::{MutationCategory, MutationKey, MutationState};

    let store = use_context::<ProjectStore>();
    let key = MutationKey::TrustProject(agentic_afk_contracts::ProjectId(project_id.clone()));
    let pending = store.is_pending(&key);
    let inline_error = match store.state(&key) {
        Some(MutationState::Error {
            category: MutationCategory::Validation,
            title,
            detail,
        }) => Some((title, detail)),
        _ => None,
    };

    rsx! {
        div { class: "flex flex-col gap-1",
            div { class: "flex items-center gap-3",
                span { class: "text-zinc-400", "Not trusted" }
                button {
                    class: "rounded border border-emerald-700 px-3 py-1.5 text-xs font-medium text-emerald-100 hover:border-emerald-500 hover:bg-emerald-950/45 disabled:cursor-not-allowed disabled:opacity-50",
                    disabled: pending,
                    "data-testid": "trust-project-button",
                    "data-mutation-pending": if pending { "true" } else { "false" },
                    onclick: {
                        let project_id = project_id.clone();
                        let key = key.clone();
                        move |_| {
                            let project_id = project_id.clone();
                            let key = key.clone();
                            async move {
                                let result = store
                                    .mutate(key, trust_project_api(project_id.clone()))
                                    .await;
                                if result.is_ok() {
                                    store.push_success(
                                        "Project trusted",
                                        "Agent execution is now allowed",
                                    );
                                }
                            }
                        }
                    },
                    if pending { "Trusting…" } else { "Trust Project" }
                }
            }
            if let Some((title, detail)) = inline_error {
                p {
                    class: "text-xs text-red-300",
                    "data-trust-error": "true",
                    "{title}: {detail}"
                }
            }
        }
    }
}

/// Three lifecycle mutations on an Issue Assignment that share the same UI
/// shape (POST, disable-while-pending, inline validation error, pessimistic
/// refetch). Modelled as a closed enum so each variant carries the routing,
/// labels, and mutation key in one place.
#[derive(Clone, Copy, PartialEq)]
enum AssignmentLifecycle {
    RefreshProposal,
    Recover,
    Abandon,
}

impl AssignmentLifecycle {
    fn key(self, project_id: &str, assignment_id: &str) -> project_store::MutationKey {
        use project_store::{IssueAssignmentId, MutationKey};
        let pid = agentic_afk_contracts::ProjectId(project_id.to_string());
        let aid = IssueAssignmentId(assignment_id.to_string());
        match self {
            Self::RefreshProposal => MutationKey::RefreshProposalState(pid, aid),
            Self::Recover => MutationKey::RecoverAssignment(pid, aid),
            Self::Abandon => MutationKey::AbandonAssignment(pid, aid),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::RefreshProposal => "Refresh Proposal State",
            Self::Recover => "Recover Assignment",
            Self::Abandon => "Abandon Assignment",
        }
    }

    fn pending_label(self) -> &'static str {
        match self {
            Self::RefreshProposal => "Refreshing…",
            Self::Recover => "Recovering…",
            Self::Abandon => "Abandoning…",
        }
    }

    fn testid(self) -> &'static str {
        match self {
            Self::RefreshProposal => "refresh-proposal-state-button",
            Self::Recover => "recover-assignment-button",
            Self::Abandon => "abandon-assignment-button",
        }
    }

    /// Stable value for the `data-lifecycle-error` attribute on the inline
    /// error region, so Playwright can target each mutation distinctly.
    fn error_marker(self) -> &'static str {
        match self {
            Self::RefreshProposal => "refresh-proposal",
            Self::Recover => "recover-assignment",
            Self::Abandon => "abandon-assignment",
        }
    }

    fn button_class(self) -> &'static str {
        match self {
            Self::RefreshProposal => {
                "mt-2 w-fit rounded border border-emerald-700 px-2.5 py-1.5 text-left text-xs font-medium text-emerald-100 hover:border-emerald-500 hover:bg-emerald-950/45 disabled:cursor-not-allowed disabled:opacity-50"
            }
            Self::Recover => {
                "mt-2 w-fit rounded border border-amber-700 px-2.5 py-1.5 text-left text-xs font-medium text-amber-100 hover:border-amber-500 hover:bg-amber-950/45 disabled:cursor-not-allowed disabled:opacity-50"
            }
            Self::Abandon => {
                "mt-2 w-fit rounded border border-rose-700 px-2.5 py-1.5 text-left text-xs font-medium text-rose-100 hover:border-rose-500 hover:bg-rose-950/45 disabled:cursor-not-allowed disabled:opacity-50"
            }
        }
    }

    async fn invoke(
        self,
        project_id: String,
        assignment_id: String,
    ) -> Result<agentic_afk_contracts::IssueAssignmentResponse, project_store::MutationFailure>
    {
        match self {
            Self::RefreshProposal => refresh_proposal_state_api(project_id, assignment_id).await,
            Self::Recover => recover_assignment_api(project_id, assignment_id).await,
            Self::Abandon => abandon_assignment_api(project_id, assignment_id).await,
        }
    }
}

#[component]
fn RefreshProposalStateButton(project_id: String, assignment_id: String) -> Element {
    rsx! { AssignmentLifecycleButton { kind: AssignmentLifecycle::RefreshProposal, project_id, assignment_id } }
}

#[component]
fn RecoverAssignmentButton(project_id: String, assignment_id: String) -> Element {
    rsx! { AssignmentLifecycleButton { kind: AssignmentLifecycle::Recover, project_id, assignment_id } }
}

#[component]
fn AbandonAssignmentButton(project_id: String, assignment_id: String) -> Element {
    rsx! { AssignmentLifecycleButton { kind: AssignmentLifecycle::Abandon, project_id, assignment_id } }
}

#[component]
fn AssignmentLifecycleButton(
    kind: AssignmentLifecycle,
    project_id: String,
    assignment_id: String,
) -> Element {
    use project_store::{MutationCategory, MutationState};

    let store = use_context::<ProjectStore>();
    let key = kind.key(&project_id, &assignment_id);
    let pending = store.is_pending(&key);
    let inline_error = match store.state(&key) {
        Some(MutationState::Error {
            category: MutationCategory::Validation,
            title,
            detail,
        }) => Some((title, detail)),
        _ => None,
    };
    rsx! {
        div { class: "flex flex-col gap-1",
            button {
                class: kind.button_class(),
                disabled: pending,
                "data-testid": kind.testid(),
                "data-mutation-pending": if pending { "true" } else { "false" },
                onclick: {
                    let key = key.clone();
                    let project_id = project_id.clone();
                    let assignment_id = assignment_id.clone();
                    move |_| {
                        let key = key.clone();
                        let project_id = project_id.clone();
                        let assignment_id = assignment_id.clone();
                        async move {
                            let _ = store.mutate(key, kind.invoke(project_id, assignment_id)).await;
                        }
                    }
                },
                if pending { "{kind.pending_label()}" } else { "{kind.label()}" }
            }
            if let Some((title, detail)) = inline_error {
                p {
                    class: "text-xs text-red-300",
                    "data-lifecycle-error": kind.error_marker(),
                    "{title}: {detail}"
                }
            }
        }
    }
}

#[component]
fn ProjectOverview(id: String) -> Element {
    rsx! {
        IssueSourcePanels { id: id.clone() }
        AssignmentPanel { id: id.clone() }
        PlanningPanel { id: id.clone() }
        ActivitySection { id }
    }
}

#[component]
fn ProjectPlanning(id: String) -> Element {
    rsx! { PlanningPanel { id } }
}

#[component]
fn ProjectAssignment(id: String) -> Element {
    rsx! { AssignmentPanel { id } }
}

#[component]
fn ProjectIssueSource(id: String) -> Element {
    rsx! { IssueSourcePanels { id } }
}

#[component]
fn ProjectActivity(id: String) -> Element {
    rsx! { ActivitySection { id } }
}

#[component]
fn IssueSourcePanels(id: String) -> Element {
    let store_state = use_context::<ProjectStore>().state_signal();
    let s = store_state.read();
    // `enabled_issue_source` on the hydrated project may be stale after a
    // candidate is enabled mid-session (we don't emit a project-changed
    // event). The candidate list is updated via SSE on enable, so derive
    // the "source enabled" flag from any candidate marked enabled instead.
    let has_enabled_source = s
        .project
        .as_ref()
        .map(|p| p.enabled_issue_source.is_some())
        .unwrap_or(false)
        || s.issue_source_candidates.iter().any(|c| c.enabled);
    let candidates = s.issue_source_candidates.clone();
    drop(s);

    rsx! {
        if has_enabled_source {
            IssueSourceSyncStatus { project_id: id.clone() }
        }
        IssueSourceCandidates {
            project_id: id.clone(),
            candidates,
        }
    }
}

#[component]
fn AssignmentPanel(id: String) -> Element {
    let state = use_context::<ProjectStore>().state_signal();
    let assignment_state = state.read().assignment_state.clone();
    rsx! {
        match assignment_state {
            Some(s) => rsx! { AssignmentState { project_id: id.clone(), state: s } },
            None => rsx! {
                StatusPanel {
                    title: "Issue Assignment".to_string(),
                    detail: "Loading".to_string(),
                    tone: "border-zinc-700 bg-zinc-900 text-zinc-100".to_string(),
                }
            },
        }
    }
}

#[component]
fn PlanningPanel(id: String) -> Element {
    let store_state = use_context::<ProjectStore>().state_signal();
    let s = store_state.read();
    let snapshot = s.planning_snapshot.clone();
    let trusted = s.project.as_ref().map(|p| p.trusted).unwrap_or(false);
    drop(s);

    rsx! {
        match snapshot {
            Some(snapshot) => rsx! {
                PlanningSnapshot {
                    project_id: id.clone(),
                    trusted,
                    snapshot,
                }
            },
            None => rsx! {
                StatusPanel {
                    title: "Loading planning snapshot".to_string(),
                    detail: id.clone(),
                    tone: "border-zinc-700 bg-zinc-900 text-zinc-100".to_string(),
                }
            },
        }
    }
}

#[component]
fn ActivitySection(id: String) -> Element {
    let _ = id;
    let state = use_context::<ProjectStore>().state_signal();
    let entries = state.read().activity.clone();
    rsx! { ActivityPanel { entries } }
}

#[component]
fn SettingsCard(info: AppInfoResponse) -> Element {
    rsx! {
        Card {
            CardHead {
                title: "Settings".to_string(),
                id_text: Some(info.version.clone()),
            }
            CardBody {
                KeyValueList {
                    KeyValueRow {
                        label: "Bind address".to_string(),
                        value: info.config.bind_address.clone(),
                    }
                    KeyValueRow {
                        label: "Dashboard assets".to_string(),
                        value: info.config.dashboard_asset_dir.clone(),
                    }
                    KeyValueRow {
                        label: "Database".to_string(),
                        value: info.config.database_url.clone(),
                    }
                }
            }
        }
    }
}

#[component]
fn SettingsLoadingCard() -> Element {
    rsx! {
        Card {
            CardHead {
                title: "Settings".to_string(),
                id_text: Some("Loading\u{2026}".to_string()),
            }
            CardBody {
                SkeletonHeading {}
                SkeletonLine { width_percent: 80 }
                SkeletonLine { width_percent: 60 }
                SkeletonLine { width_percent: 70 }
            }
        }
    }
}

/// Status panel still used by Project sub-routes (Assignment / Planning /
/// project-load fallbacks). Issue #35 recomposes those surfaces.
#[component]
fn StatusPanel(title: String, detail: String, tone: String) -> Element {
    rsx! {
        section { class: "rounded-lg border p-5 {tone}",
            h2 { class: "text-base font-semibold", "{title}" }
            p { class: "mt-2 text-sm opacity-85", "{detail}" }
        }
    }
}

#[component]
fn SettingRow(label: String, value: String) -> Element {
    rsx! {
        div { class: "grid gap-1 border-b border-zinc-800 pb-3 last:border-0 last:pb-0",
            dt { class: "text-zinc-400", "{label}" }
            dd { class: "break-words font-mono text-zinc-100", "{value}" }
        }
    }
}

#[component]
fn ProjectRow(project: ProjectResponse) -> Element {
    let (trust_tone, trust_label) = if project.trusted {
        (PillTone::Verified, "Trusted")
    } else {
        (PillTone::Stale, "Untrusted")
    };
    let (git_tone, git_label) = derive_git_pill(project.git_summary.as_ref());

    rsx! {
        li {
            Card {
                CardHead {
                    title: "Project".to_string(),
                    id_text: Some(short_project_id(&project.id.0)),
                }
                CardBody {
                    div { class: "flex flex-col gap-4",
                        Link {
                            to: Route::ProjectOverview { id: project.id.0.clone() },
                            class: "w-fit break-words font-mono text-[13px] text-cyan hover:text-ink",
                            "{project.path}"
                        }
                        div { class: "flex flex-wrap items-center gap-2",
                            StatusPill { tone: trust_tone, label: trust_label.to_string() }
                            StatusPill { tone: git_tone, label: git_label }
                        }
                        match project.git_summary.clone() {
                            Some(summary) => rsx! { GitSummaryRow { summary } },
                            None => rsx! {},
                        }
                    }
                }
            }
        }
    }
}

fn short_project_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn derive_git_pill(summary: Option<&GitSummary>) -> (PillTone, String) {
    match summary {
        None => (PillTone::Idle, "No Git".to_string()),
        Some(s) if s.dirty => (
            PillTone::Stale,
            s.branch.clone().unwrap_or_else(|| "dirty".to_string()),
        ),
        Some(s) => (
            PillTone::Verified,
            s.branch.clone().unwrap_or_else(|| "clean".to_string()),
        ),
    }
}

#[component]
fn ActivityPanel(entries: Vec<ProjectActivityEntryResponse>) -> Element {
    rsx! {
        section { class: "rounded-lg border border-zinc-800 bg-zinc-900 p-5",
            h2 { class: "text-base font-semibold", "Activity" }
            if entries.is_empty() {
                p { class: "mt-3 text-sm text-zinc-500", "No Activity recorded yet." }
            } else {
                ul { class: "mt-4 grid gap-2",
                    for entry in entries {
                        li { class: "flex flex-col gap-1 border-b border-zinc-800 pb-2 text-sm last:border-0 last:pb-0",
                            div { class: "flex items-baseline justify-between gap-3",
                                span { class: "font-mono text-emerald-200", "{entry.kind}" }
                                span { class: "font-mono text-xs text-zinc-500", "{entry.recorded_at}" }
                            }
                            if let Some(detail) = entry.detail.clone() {
                                p { class: "break-words text-xs text-zinc-300", "{detail}" }
                            }
                            if let Some(assignment_id) = entry.assignment_id.clone() {
                                p { class: "font-mono text-[10px] text-zinc-500", "assignment {assignment_id}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn IssueSourceSyncStatus(project_id: String) -> Element {
    use project_store::{MutationCategory, MutationKey, MutationState};

    let store = use_context::<ProjectStore>();
    let store_state = store.state_signal();
    let s = store_state.read();
    let last_successful_sync_at = s
        .planning_snapshot
        .as_ref()
        .and_then(|p| p.last_successful_sync_at.clone());
    let last_failure = s
        .planning_snapshot
        .as_ref()
        .and_then(|p| p.last_failure.clone());
    let sync_in_progress = s.sync_in_progress;
    drop(s);
    let refresh_project_id = project_id.clone();

    let key = MutationKey::SyncIssueSource(agentic_afk_contracts::ProjectId(project_id.clone()));
    let pending = store.is_pending(&key) || sync_in_progress;
    let inline_error = match store.state(&key) {
        Some(MutationState::Error {
            category: MutationCategory::Validation,
            title,
            detail,
        }) => Some((title, detail)),
        _ => None,
    };

    rsx! {
        section { class: "rounded-lg border border-zinc-800 bg-zinc-900 p-5",
            div { class: "flex flex-col gap-3 md:flex-row md:items-start md:justify-between",
                div {
                    h2 { class: "text-base font-semibold", "Last sync status" }
                    p { class: "mt-2 font-mono text-sm text-zinc-300",
                        {last_successful_sync_at.clone().unwrap_or_else(|| "Never synced".to_string())}
                    }
                    if let Some(failure) = last_failure.clone() {
                        p { class: "mt-2 text-sm text-red-100", "{failure}" }
                    }
                }
                div { class: "flex flex-col items-end gap-1",
                    button {
                        class: if pending {
                            "rounded border border-emerald-700 px-3 py-2 text-sm font-medium text-emerald-100 cursor-not-allowed opacity-50"
                        } else {
                            "rounded border border-emerald-700 px-3 py-2 text-sm font-medium text-emerald-100 hover:border-emerald-500 hover:bg-emerald-950/45"
                        },
                        "aria-disabled": if pending { "true" } else { "false" },
                        "data-testid": "refresh-issue-source-button",
                        "data-mutation-pending": if pending { "true" } else { "false" },
                        onclick: {
                            let project_id = refresh_project_id.clone();
                            let key = key.clone();
                            move |_| {
                                let project_id = project_id.clone();
                                let key = key.clone();
                                let already_pending = pending;
                                async move {
                                    if already_pending {
                                        return;
                                    }
                                    let result = store
                                        .mutate(key, sync_issue_source(project_id.clone()))
                                        .await;
                                    if result.is_ok() {
                                        // The SSE `IssueSourceSyncCompleted` /
                                        // `PlanningSnapshotChanged` deltas
                                        // refresh the panel without a fetch.
                                        store.push_success("Issue Source synced", String::new());
                                    }
                                }
                            }
                        },
                        if pending { "Refreshing…" } else { "Refresh Issue Source" }
                    }
                    if let Some((title, detail)) = inline_error {
                        p {
                            class: "text-xs text-red-300",
                            "data-sync-error": "true",
                            "{title}: {detail}"
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn IssueSourceCandidates(project_id: String, candidates: Vec<IssueSourceCandidate>) -> Element {
    rsx! {
        section { class: "rounded-lg border border-zinc-800 bg-zinc-900 p-5",
            h2 { class: "text-base font-semibold", "Issue Source candidates" }
            if candidates.is_empty() {
                p { class: "mt-3 text-sm text-zinc-500", "None" }
            } else {
                ul { class: "mt-4 grid gap-3",
                    for candidate in candidates {
                        IssueSourceCandidateRow {
                            project_id: project_id.clone(),
                            candidate,
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn IssueSourceCandidateRow(project_id: String, candidate: IssueSourceCandidate) -> Element {
    use project_store::{MutationCategory, MutationKey, MutationState};

    let store = use_context::<ProjectStore>();
    let enable_project_id = project_id.clone();
    let enable_kind = candidate.kind.clone();
    let enable_locator = candidate.locator.clone();

    let key = MutationKey::EnableIssueSource(
        agentic_afk_contracts::ProjectId(project_id.clone()),
        candidate.kind.clone(),
        candidate.locator.clone(),
    );
    let pending = store.is_pending(&key);
    let inline_error = match store.state(&key) {
        Some(MutationState::Error {
            category: MutationCategory::Validation,
            title,
            detail,
        }) => Some((title, detail)),
        _ => None,
    };
    let testid = format!(
        "enable-issue-source-{}-{}",
        candidate.kind, candidate.locator
    );

    rsx! {
        li { class: "flex flex-col gap-2 border-b border-zinc-800 pb-3 text-sm text-zinc-100 last:border-0 last:pb-0 md:flex-row md:items-center md:justify-between",
            p { class: "break-words font-mono", "{candidate.kind} {candidate.locator}" }
            if candidate.enabled {
                p {
                    class: "text-xs text-emerald-200",
                    "data-candidate-enabled": "true",
                    "Enabled"
                }
            } else {
                div { class: "flex flex-col items-start gap-1 md:items-end",
                    button {
                        class: if pending {
                            "rounded border border-emerald-700 px-3 py-2 text-left text-xs font-medium text-emerald-100 cursor-not-allowed opacity-50"
                        } else {
                            "rounded border border-emerald-700 px-3 py-2 text-left text-xs font-medium text-emerald-100 hover:border-emerald-500 hover:bg-emerald-950/45"
                        },
                        "aria-disabled": if pending { "true" } else { "false" },
                        "data-testid": testid,
                        "data-mutation-pending": if pending { "true" } else { "false" },
                        onclick: {
                            let project_id = enable_project_id.clone();
                            let kind = enable_kind.clone();
                            let locator = enable_locator.clone();
                            let key = key.clone();
                            move |_| {
                                let project_id = project_id.clone();
                                let kind = kind.clone();
                                let locator = locator.clone();
                                let key = key.clone();
                                let already_pending = pending;
                                async move {
                                    if already_pending {
                                        return;
                                    }
                                    let _ = store
                                        .mutate(
                                            key,
                                            enable_issue_source(project_id, kind, locator),
                                        )
                                        .await;
                                }
                            }
                        },
                        if pending {
                            "Enabling…"
                        } else {
                            "Enable {candidate.kind} {candidate.locator}"
                        }
                    }
                    if let Some((title, detail)) = inline_error {
                        p {
                            class: "text-xs text-red-300",
                            "data-enable-error": "true",
                            "{title}: {detail}"
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PlanningSnapshot(
    project_id: String,
    trusted: bool,
    snapshot: PlanningSnapshotResponse,
) -> Element {
    let last_sync = snapshot
        .last_successful_sync_at
        .clone()
        .unwrap_or_else(|| "Never synced".to_string());

    rsx! {
        section { class: "rounded-lg border border-zinc-800 bg-zinc-900 p-5",
            div { class: "flex flex-col gap-2 border-b border-zinc-800 pb-4 md:flex-row md:items-start md:justify-between",
                div {
                    h2 { class: "text-base font-semibold", "Planning snapshot" }
                    p { class: "mt-1 font-mono text-xs text-zinc-500", "{snapshot.source.kind} {snapshot.source.locator}" }
                }
                p { class: "font-mono text-xs text-zinc-400", "{last_sync}" }
            }
            match snapshot.last_failure {
                Some(failure) => rsx! {
                    p { class: "mt-4 rounded border border-red-800 bg-red-950/35 px-3 py-2 text-sm text-red-100", "{failure}" }
                },
                None => rsx! {},
            }
            div { class: "mt-4 grid gap-4 lg:grid-cols-5",
                PlanningGroup {
                    project_id: project_id.clone(),
                    title: "Eligible Ready Issues".to_string(),
                    issues: snapshot.eligible,
                    can_start: trusted,
                }
                PlanningGroup {
                    project_id: project_id.clone(),
                    title: "Active Issues".to_string(),
                    issues: snapshot.active,
                    can_start: false,
                }
                PlanningGroup {
                    project_id: project_id.clone(),
                    title: "Blocked Ready Issues".to_string(),
                    issues: snapshot.blocked,
                    can_start: false,
                }
                PlanningGroup {
                    project_id: project_id.clone(),
                    title: "Completed Issues".to_string(),
                    issues: snapshot.completed,
                    can_start: false,
                }
                PlanningGroup {
                    project_id,
                    title: "Non-ready Source Issues".to_string(),
                    issues: snapshot.non_ready,
                    can_start: false,
                }
            }
        }
    }
}

#[component]
fn PlanningGroup(
    project_id: String,
    title: String,
    issues: Vec<SourceIssueSnapshot>,
    can_start: bool,
) -> Element {
    rsx! {
        section { class: "min-w-0 rounded border border-zinc-800 bg-zinc-950/40 p-4",
            h3 { class: "text-sm font-semibold text-zinc-100", "{title}" }
            if issues.is_empty() {
                p { class: "mt-3 text-sm text-zinc-500", "None" }
            } else {
                ul { class: "mt-3 grid gap-3",
                    for issue in issues {
                        PlanningIssue {
                            project_id: project_id.clone(),
                            issue,
                            can_start,
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PlanningIssue(project_id: String, issue: SourceIssueSnapshot, can_start: bool) -> Element {
    use project_store::{MutationCategory, MutationKey, MutationState, SourceIssueId};

    let dependencies = if issue.issue_dependencies.is_empty() {
        "No dependencies".to_string()
    } else {
        issue.issue_dependencies.join(", ")
    };
    let parent = issue
        .parent_issue
        .clone()
        .unwrap_or_else(|| "No parent".to_string());
    let store = use_context::<ProjectStore>();
    let key = MutationKey::StartAssignment(
        agentic_afk_contracts::ProjectId(project_id.clone()),
        SourceIssueId(issue.source_id.clone()),
    );
    let pending = store.is_pending(&key);
    let inline_error = match store.state(&key) {
        Some(MutationState::Error {
            category: MutationCategory::Validation,
            title,
            detail,
        }) => Some((title, detail)),
        _ => None,
    };
    let start_project_id = project_id.clone();
    let start_source_id = issue.source_id.clone();

    rsx! {
        li { class: "grid gap-1 border-b border-zinc-800 pb-3 last:border-0 last:pb-0",
            div { class: "flex items-baseline justify-between gap-3",
                p { class: "min-w-0 break-words text-sm font-medium text-zinc-100", "{issue.title}" }
                p { class: "shrink-0 font-mono text-xs text-zinc-500", "#{issue.source_order}" }
            }
            p { class: "break-words font-mono text-xs text-zinc-500", "{issue.source_id}" }
            p { class: "text-xs text-zinc-400", "Parent {parent}" }
            p { class: "text-xs text-zinc-400", "{dependencies}" }
            if can_start {
                button {
                    class: "mt-2 rounded border border-emerald-700 px-2.5 py-1.5 text-left text-xs font-medium text-emerald-100 hover:border-emerald-500 hover:bg-emerald-950/45 disabled:cursor-not-allowed disabled:opacity-50",
                    disabled: pending,
                    "data-testid": "start-assignment-button",
                    "data-mutation-pending": if pending { "true" } else { "false" },
                    onclick: {
                        let key = key.clone();
                        move |_| {
                            let project_id = start_project_id.clone();
                            let source_id = start_source_id.clone();
                            let key = key.clone();
                            async move {
                                let _ = store
                                    .mutate(key, start_assignment_api(project_id, source_id))
                                    .await;
                            }
                        }
                    },
                    if pending { "Starting…" } else { "Start Assignment" }
                }
                if let Some((title, detail)) = inline_error {
                    p {
                        class: "text-xs text-red-300",
                        "data-start-assignment-error": "true",
                        "{title}: {detail}"
                    }
                }
            }
        }
    }
}

#[component]
fn AssignmentState(project_id: String, state: ProjectAssignmentStateResponse) -> Element {
    rsx! {
        section { class: "rounded-lg border border-zinc-800 bg-zinc-900 p-5",
            h2 { class: "text-base font-semibold", "Issue Assignment" }
            match state.active_assignment {
                Some(assignment) => {
                    let lifecycle_label = match assignment.status.as_str() {
                        "proposal_pending" => "Change Proposal awaiting checks",
                        "proposal_verified" => "Verified — awaiting human merge",
                        "completed" => "Completed",
                        other => other,
                    };
                    let can_refresh_proposal = matches!(
                        assignment.status.as_str(),
                        "proposal_pending" | "proposal_verified"
                    );
                    let can_abandon = assignment.status == "blocked";
                    let can_recover = assignment.status == "blocked";
                    let assignment_id = assignment.id.clone();
                    rsx! {
                        div { class: "mt-4 grid gap-2 text-sm",
                            p { class: "font-medium text-zinc-100", "{assignment.source_title}" }
                            p { class: "font-mono text-xs text-zinc-400", "{assignment.source_id}" }
                            p { class: "text-zinc-300", "State {lifecycle_label}" }
                            p { class: "break-words font-mono text-xs text-zinc-500", "{assignment.branch}" }
                            if let Some(detail) = assignment.status_detail.clone() {
                                p { class: "text-zinc-300", "{detail}" }
                            }
                            if let Some(proposal) = assignment.change_proposal.clone() {
                                a {
                                    class: "w-fit text-emerald-300 underline decoration-emerald-800 underline-offset-4 hover:text-emerald-200",
                                    href: "{proposal.url}",
                                    target: "_blank",
                                    rel: "noreferrer",
                                    "Change Proposal {proposal.status}"
                                }
                            }
                            if can_refresh_proposal {
                                RefreshProposalStateButton {
                                    project_id: project_id.clone(),
                                    assignment_id: assignment_id.clone(),
                                }
                            }
                            if can_recover {
                                RecoverAssignmentButton {
                                    project_id: project_id.clone(),
                                    assignment_id: assignment_id.clone(),
                                }
                            }
                            if can_abandon {
                                AbandonAssignmentButton {
                                    project_id: project_id.clone(),
                                    assignment_id: assignment_id.clone(),
                                }
                            }
                            if let Some(budget) = assignment.repair_budget.clone() {
                                p { class: "text-xs text-zinc-400",
                                    "Repair Loop {budget.attempt_count} of {budget.max_attempts} attempts within {budget.window_seconds}s window"
                                }
                            }
                        }
                        if state.waiting_ready_issue_count > 0 {
                            p { class: "mt-4 border-t border-zinc-800 pt-3 text-sm text-zinc-300",
                                "{state.waiting_ready_issue_count} eligible Ready Issue waiting for the Project assignment slot."
                            }
                        }
                    }
                },
                None => rsx! {
                    p { class: "mt-3 text-sm text-zinc-500", "No active Issue Assignment" }
                },
            }
        }
    }
}

#[component]
fn GitSummaryRow(summary: GitSummary) -> Element {
    let branch = summary.branch.unwrap_or_else(|| "detached".to_string());
    let head = summary
        .head
        .map(|head| head.chars().take(12).collect::<String>())
        .unwrap_or_else(|| "unknown".to_string());
    let state = if summary.dirty { "dirty" } else { "clean" };

    rsx! {
        dl { class: "flex flex-wrap gap-x-4 gap-y-1 text-sm",
            GitSummaryTerm { label: "Branch".to_string(), value: branch }
            GitSummaryTerm { label: "Head".to_string(), value: head }
            GitSummaryTerm { label: "State".to_string(), value: state.to_string() }
        }
    }
}

#[component]
fn GitSummaryTerm(label: String, value: String) -> Element {
    rsx! {
        div { class: "flex gap-1",
            dt { class: "text-zinc-500", "{label}" }
            dd { class: "font-mono text-zinc-200", "{value}" }
        }
    }
}

async fn fetch_app_info() -> Result<AppInfoResponse, String> {
    gloo_net::http::Request::get("/api/app-info")
        .send()
        .await
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())
}

async fn fetch_projects() -> Result<Vec<ProjectResponse>, String> {
    gloo_net::http::Request::get("/api/projects")
        .send()
        .await
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())
}

async fn fetch_project(project_id: String) -> Result<ProjectResponse, String> {
    let projects = fetch_projects().await?;
    projects
        .into_iter()
        .find(|project| project.id.0 == project_id)
        .ok_or_else(|| format!("Project {project_id} not found"))
}

async fn fetch_project_snapshot(
    project_id: String,
) -> Result<ProjectSnapshotResponse, String> {
    gloo_net::http::Request::get(&format!("/api/projects/{project_id}/snapshot"))
        .send()
        .await
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())
}

async fn enable_issue_source(
    project_id: String,
    kind: String,
    locator: String,
) -> Result<ProjectResponse, project_store::MutationFailure> {
    let response = gloo_net::http::Request::put(&format!(
        "/api/projects/{project_id}/issue-source"
    ))
    .json(&EnableIssueSourceRequest { kind, locator })
    .map_err(|error| project_store::MutationFailure::network(error.to_string()))?
    .send()
    .await
    .map_err(|error| project_store::MutationFailure::network(error.to_string()))?;
    let status = response.status();
    if !(200..300).contains(&status) {
        let body = response.text().await.unwrap_or_default();
        return Err(project_store::MutationFailure::http(status, body));
    }
    response
        .json()
        .await
        .map_err(|error| project_store::MutationFailure::network(error.to_string()))
}

async fn trust_project_api(
    project_id: String,
) -> Result<ProjectResponse, project_store::MutationFailure> {
    let response = gloo_net::http::Request::put(&format!("/api/projects/{project_id}/trust"))
        .send()
        .await
        .map_err(|error| project_store::MutationFailure::network(error.to_string()))?;
    let status = response.status();
    if !(200..300).contains(&status) {
        let body = response.text().await.unwrap_or_default();
        return Err(project_store::MutationFailure::http(status, body));
    }
    response
        .json()
        .await
        .map_err(|error| project_store::MutationFailure::network(error.to_string()))
}

async fn start_assignment_api(
    project_id: String,
    source_id: String,
) -> Result<agentic_afk_contracts::IssueAssignmentResponse, project_store::MutationFailure> {
    post_assignment_mutation(format!(
        "/api/projects/{project_id}/source-issues/{source_id}/assignment"
    ))
    .await
}

async fn abandon_assignment_api(
    project_id: String,
    assignment_id: String,
) -> Result<agentic_afk_contracts::IssueAssignmentResponse, project_store::MutationFailure> {
    post_assignment_mutation(format!(
        "/api/projects/{project_id}/assignments/{assignment_id}/abandon"
    ))
    .await
}

async fn recover_assignment_api(
    project_id: String,
    assignment_id: String,
) -> Result<agentic_afk_contracts::IssueAssignmentResponse, project_store::MutationFailure> {
    post_assignment_mutation(format!(
        "/api/projects/{project_id}/assignments/{assignment_id}/recover"
    ))
    .await
}

async fn refresh_proposal_state_api(
    project_id: String,
    assignment_id: String,
) -> Result<agentic_afk_contracts::IssueAssignmentResponse, project_store::MutationFailure> {
    post_assignment_mutation(format!(
        "/api/projects/{project_id}/assignments/{assignment_id}/refresh-proposal-state"
    ))
    .await
}

/// Issue a POST against an Issue Assignment lifecycle endpoint and map the
/// outcome into a `MutationFailure` so the `ProjectStore` can categorize it.
async fn post_assignment_mutation(
    path: String,
) -> Result<agentic_afk_contracts::IssueAssignmentResponse, project_store::MutationFailure> {
    let response = gloo_net::http::Request::post(&path)
        .send()
        .await
        .map_err(|error| project_store::MutationFailure::network(error.to_string()))?;
    let status = response.status();
    if !(200..300).contains(&status) {
        let body = response.text().await.unwrap_or_default();
        return Err(project_store::MutationFailure::http(status, body));
    }
    response
        .json()
        .await
        .map_err(|error| project_store::MutationFailure::network(error.to_string()))
}

async fn sync_issue_source(
    project_id: String,
) -> Result<IssueSourceSyncResponse, project_store::MutationFailure> {
    let response = gloo_net::http::Request::post(&format!(
        "/api/projects/{project_id}/issue-source/sync"
    ))
    .send()
    .await
    .map_err(|error| project_store::MutationFailure::network(error.to_string()))?;
    let status = response.status();
    if !(200..300).contains(&status) {
        let body = response.text().await.unwrap_or_default();
        return Err(project_store::MutationFailure::http(status, body));
    }
    response
        .json()
        .await
        .map_err(|error| project_store::MutationFailure::network(error.to_string()))
}


#[component]
fn DesignSandbox() -> Element {
    let store = use_context::<ProjectStore>();

    use_hook(|| {
        let pending_key = MutationKey::SyncIssueSource(ProjectId("demo".into()));
        let err_key = MutationKey::StartAssignment(
            ProjectId("demo".into()),
            SourceIssueId("issue-A".into()),
        );
        store.force_state(pending_key, MutationState::Pending);
        store.force_state(
            err_key,
            MutationState::Error {
                category: MutationCategory::Validation,
                title: "project trust required".into(),
                detail: "trust the project from settings before booting an agent".into(),
            },
        );
    });

    let idle_key = MutationKey::AbandonAssignment(
        ProjectId("demo".into()),
        IssueAssignmentId("assn-A".into()),
    );
    let pending_key = MutationKey::SyncIssueSource(ProjectId("demo".into()));
    let err_key = MutationKey::StartAssignment(
        ProjectId("demo".into()),
        SourceIssueId("issue-A".into()),
    );

    rsx! {
        document::Link {
            rel: "stylesheet",
            href: "https://fonts.googleapis.com/css2?family=IBM+Plex+Sans+Condensed:wght@400;500;600;700&family=Inter+Tight:wght@400;500;600&family=JetBrains+Mono:wght@400;500&display=swap",
        }
        div { class: "min-h-screen bg-void text-ink font-body",
            div { class: "mx-auto max-w-[1240px] px-10 py-14",
                p { class: "font-mono text-[11px] uppercase tracking-[0.18em] text-ink-dim", "primitives sandbox" }
                h1 { class: "mt-3 font-display text-5xl font-bold uppercase tracking-wide", "HUD // Mission Control" }
                p { class: "mt-3 max-w-[64ch] text-ink-2",
                    "Every primitive in every state. Cyan signal, coral danger, mint verified, amber in-flight."
                }

                SandboxSection { heading: "StatusPill".to_string(),
                    div { class: "flex flex-wrap gap-4",
                        StatusPill { tone: PillTone::Idle, label: "Idle".to_string() }
                        StatusPill { tone: PillTone::Pending, label: "Pending".to_string() }
                        StatusPill { tone: PillTone::Running, label: "Running".to_string() }
                        StatusPill { tone: PillTone::Verified, label: "Verified".to_string() }
                        StatusPill { tone: PillTone::Stale, label: "Stale".to_string() }
                        StatusPill { tone: PillTone::Failed, label: "Failed".to_string() }
                    }
                }

                SandboxSection { heading: "ActionButton".to_string(),
                    p { class: "mb-4 text-ink-2 text-sm",
                        "Pending and error visuals come from "
                        code { class: "font-mono text-cyan", "ProjectStore" }
                        " automatically."
                    }
                    div { class: "grid gap-6 md:grid-cols-3",
                        div {
                            p { class: "mb-3 font-mono text-[10px] uppercase tracking-[0.22em] text-ink-dim", "Idle" }
                            ActionButton {
                                mutation_key: idle_key.clone(),
                                variant: ButtonVariant::Destructive,
                                on_press: move |_| {},
                                "Abandon Assignment"
                            }
                        }
                        div {
                            p { class: "mb-3 font-mono text-[10px] uppercase tracking-[0.22em] text-ink-dim", "Pending (store)" }
                            ActionButton {
                                mutation_key: pending_key.clone(),
                                variant: ButtonVariant::Default,
                                on_press: move |_| {},
                                "Syncing"
                            }
                        }
                        div {
                            p { class: "mb-3 font-mono text-[10px] uppercase tracking-[0.22em] text-ink-dim", "Validation error (store)" }
                            ActionButton {
                                mutation_key: err_key.clone(),
                                variant: ButtonVariant::Primary,
                                on_press: move |_| {},
                                "Start Assignment"
                            }
                        }
                    }
                }

                SandboxSection { heading: "Card".to_string(),
                    div { class: "grid gap-5 md:grid-cols-2",
                        Card {
                            CardHead { title: "Assignment".to_string(), id_text: Some("A3F2·1C".to_string()) }
                            CardBody {
                                dl { class: "grid grid-cols-2 gap-x-6 gap-y-3 font-mono",
                                    div {
                                        dt { class: "text-[10px] uppercase tracking-[0.14em] text-ink-dim", "Status" }
                                        dd { class: "mt-0.5",
                                            StatusPill { tone: PillTone::Running, label: "Running".to_string() }
                                        }
                                    }
                                    div {
                                        dt { class: "text-[10px] uppercase tracking-[0.14em] text-ink-dim", "Uptime" }
                                        dd { class: "mt-0.5 text-[22px] leading-none",
                                            "04:11"
                                            span { class: "ml-1 text-[11px] text-ink-dim", "m" }
                                        }
                                    }
                                }
                            }
                            CardFoot {
                                ActionButton {
                                    mutation_key: MutationKey::TrustProject(ProjectId("demo-card".into())),
                                    variant: ButtonVariant::Primary,
                                    on_press: move |_| {},
                                    "Open"
                                }
                            }
                        }
                        Card {
                            CardHead { title: "Loading".to_string(), id_text: Some("LOADING\u{2026}".to_string()) }
                            CardBody {
                                SkeletonHeading {}
                                SkeletonLine { width_percent: 86 }
                                SkeletonLine { width_percent: 62 }
                                SkeletonLine { width_percent: 74 }
                            }
                        }
                    }
                }

                SandboxSection { heading: "EmptyState".to_string(),
                    div { class: "grid gap-5 md:grid-cols-2",
                        EmptyState {
                            title: "No Assignments".to_string(),
                            body: "Pick a Ready Issue to boot an agent against this Project.".to_string(),
                            accent: EmptyStateAccent::Cyan,
                            ActionButton {
                                mutation_key: MutationKey::TrustProject(ProjectId("demo-empty".into())),
                                variant: ButtonVariant::Primary,
                                on_press: move |_| {},
                                "Pick an Issue"
                            }
                        }
                        EmptyState {
                            title: "Activity Feed Quiet".to_string(),
                            body: "Lifecycle events will surface here as the agent makes progress.".to_string(),
                            accent: EmptyStateAccent::Magenta,
                        }
                    }
                }

                SandboxSection { heading: "ErrorState".to_string(),
                    ErrorState {
                        title: "Control Plane Unavailable".to_string(),
                        detail: "The Project snapshot endpoint returned 503 three times in a row.".to_string(),
                        problem_json: Some("{\n  \"type\":   \"https://errors.afk.local/control-plane-unavailable\",\n  \"title\":  \"control plane unavailable\",\n  \"detail\": \"snapshot endpoint returned 503 after 3 retries\"\n}".to_string()),
                        ActionButton {
                            mutation_key: MutationKey::TrustProject(ProjectId("demo-err".into())),
                            variant: ButtonVariant::Default,
                            on_press: move |_| {},
                            "Retry"
                        }
                    }
                }

                SandboxSection { heading: "ToastRegion".to_string(),
                    p { class: "mb-4 text-ink-2 text-sm",
                        "Toasts come from "
                        code { class: "font-mono text-cyan", "ProjectStore" }
                        "."
                    }
                    div { class: "mb-4 flex gap-3",
                        button {
                            class: "hud-notch-btn font-display text-[12px] uppercase tracking-[0.18em] border border-stroke px-4 py-2 text-mint",
                            onclick: move |_| store.push_success("Sync \u{00B7} Complete", "Issue Source produced 4 new Ready Issues"),
                            "+ success toast"
                        }
                    }
                    HudToastRegion {}
                }
            }
        }
    }
}

#[component]
fn SandboxSection(heading: String, children: Element) -> Element {
    rsx! {
        section { class: "mt-16",
            h2 { class: "mb-1 flex items-center gap-3 font-display text-xs uppercase tracking-[0.22em] text-cyan",
                span { class: "h-[14px] w-[14px] rotate-45 border border-cyan bg-cyan/40 shadow-[0_0_12px_rgba(91,233,255,0.5)]" }
                "{heading}"
            }
            div { class: "mt-6", {children} }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_git_summary_maps_to_idle_pill() {
        let (tone, label) = derive_git_pill(None);
        assert_eq!(tone, PillTone::Idle);
        assert_eq!(label, "No Git");
    }

    #[test]
    fn clean_summary_with_branch_maps_to_verified_pill() {
        let summary = GitSummary {
            branch: Some("master".to_string()),
            head: None,
            dirty: false,
        };
        let (tone, label) = derive_git_pill(Some(&summary));
        assert_eq!(tone, PillTone::Verified);
        assert_eq!(label, "master");
    }

    #[test]
    fn dirty_summary_maps_to_stale_pill_with_branch_label() {
        let summary = GitSummary {
            branch: Some("feat/x".to_string()),
            head: None,
            dirty: true,
        };
        let (tone, label) = derive_git_pill(Some(&summary));
        assert_eq!(tone, PillTone::Stale);
        assert_eq!(label, "feat/x");
    }

    #[test]
    fn detached_clean_summary_falls_back_to_clean_label() {
        let summary = GitSummary {
            branch: None,
            head: None,
            dirty: false,
        };
        let (_tone, label) = derive_git_pill(Some(&summary));
        assert_eq!(label, "clean");
    }

    #[test]
    fn short_project_id_truncates_to_eight_chars() {
        assert_eq!(short_project_id("0123456789abcdef"), "01234567");
        assert_eq!(short_project_id("abc"), "abc");
    }
}
