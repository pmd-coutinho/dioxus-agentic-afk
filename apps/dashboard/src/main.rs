mod project_store;

use project_store::{ProjectStore, Toast, ToastKind};

use agentic_afk_contracts::{
    AppInfoResponse, EnableIssueSourceRequest, GitSummary, IssueSourceCandidate,
    IssueSourceSyncResponse, IssueSourceSyncStatusResponse, PlanningSnapshotResponse,
    ProjectActivityEntryResponse, ProjectAssignmentStateResponse, ProjectResponse,
    SourceIssueSnapshot,
};
use dioxus::prelude::*;

static TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    rsx! {
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }
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
}

#[component]
fn AppShell() -> Element {
    use_context_provider(ProjectStore::new);
    rsx! {
        main { class: "min-h-screen bg-zinc-950 text-zinc-100",
            section { class: "mx-auto flex w-full max-w-5xl flex-col gap-6 px-6 py-8",
                header { class: "flex flex-col gap-1 border-b border-zinc-800 pb-5",
                    p { class: "text-sm font-medium uppercase tracking-wide text-emerald-300", "Local Control Plane" }
                    Link {
                        to: Route::Home {},
                        class: "w-fit",
                        h1 { class: "text-3xl font-semibold", "agentic-afk" }
                    }
                }
                ToastRegion {}
                Outlet::<Route> {}
            }
        }
    }
}

#[component]
fn ToastRegion() -> Element {
    let store = use_context::<ProjectStore>();
    let toasts = store.toasts();
    rsx! {
        div {
            class: "flex flex-col gap-2",
            role: "status",
            "aria-live": "polite",
            for toast in toasts.read().clone().into_iter() {
                ToastView { store, toast }
            }
        }
    }
}

#[component]
fn ToastView(store: ProjectStore, toast: Toast) -> Element {
    let tone = match toast.kind {
        ToastKind::Success => "border-emerald-700 bg-emerald-950/40 text-emerald-100",
        ToastKind::Error => "border-red-700 bg-red-950/40 text-red-100",
    };
    let id = toast.id;
    rsx! {
        div {
            class: format!("flex items-start justify-between gap-3 rounded border px-4 py-3 text-sm {tone}"),
            "data-toast-kind": match toast.kind {
                ToastKind::Success => "success",
                ToastKind::Error => "error",
            },
            div { class: "flex flex-col gap-0.5",
                p { class: "font-medium", "{toast.title}" }
                if !toast.detail.is_empty() {
                    p { class: "text-zinc-200/80", "{toast.detail}" }
                }
            }
            button {
                class: "text-xs text-zinc-300 hover:text-zinc-100",
                onclick: move |_| store.dismiss_toast(id),
                "Dismiss"
            }
        }
    }
}

#[component]
fn Home() -> Element {
    let app_info = use_resource(fetch_app_info);
    let projects = use_resource(fetch_projects);

    rsx! {
        match &*app_info.read_unchecked() {
            Some(Ok(info)) => rsx! {
                div { class: "flex flex-col gap-4",
                    div { class: "grid gap-4 md:grid-cols-[1.2fr_0.8fr]",
                        StatusPanel {
                            title: "API connected".to_string(),
                            detail: format!("{} {}", info.app_name, info.version),
                            tone: "border-emerald-700 bg-emerald-950/35 text-emerald-50".to_string(),
                        }
                        SettingsPanel { info: info.clone() }
                    }
                    ProjectsSection { projects: projects.read_unchecked().clone() }
                }
            },
            Some(Err(error)) => rsx! {
                StatusPanel {
                    title: "API disconnected".to_string(),
                    detail: error.clone(),
                    tone: "border-red-700 bg-red-950/40 text-red-100".to_string(),
                }
            },
            None => rsx! {
                StatusPanel {
                    title: "Checking API connection".to_string(),
                    detail: "Waiting for /api/app-info".to_string(),
                    tone: "border-zinc-700 bg-zinc-900 text-zinc-100".to_string(),
                }
            },
        }
    }
}

#[component]
fn ProjectList() -> Element {
    let projects = use_resource(fetch_projects);
    rsx! {
        div { class: "flex flex-col gap-4",
            ProjectsSection { projects: projects.read_unchecked().clone() }
        }
    }
}

#[component]
fn Settings() -> Element {
    let app_info = use_resource(fetch_app_info);
    rsx! {
        div { class: "flex flex-col gap-4",
            match &*app_info.read_unchecked() {
                Some(Ok(info)) => rsx! { SettingsPanel { info: info.clone() } },
                Some(Err(error)) => rsx! {
                    StatusPanel {
                        title: "API disconnected".to_string(),
                        detail: error.clone(),
                        tone: "border-red-700 bg-red-950/40 text-red-100".to_string(),
                    }
                },
                None => rsx! {
                    StatusPanel {
                        title: "Loading Settings".to_string(),
                        detail: "Waiting for /api/app-info".to_string(),
                        tone: "border-zinc-700 bg-zinc-900 text-zinc-100".to_string(),
                    }
                },
            }
        }
    }
}

#[component]
fn ProjectsSection(projects: Option<Result<Vec<ProjectResponse>, String>>) -> Element {
    rsx! {
        section { class: "rounded-lg border border-zinc-800 bg-zinc-900 p-5",
            h2 { class: "mb-4 text-base font-semibold", "Projects" }
            match projects {
                Some(Ok(projects)) if projects.is_empty() => rsx! {
                    p { class: "text-sm text-zinc-400", "No Projects" }
                },
                Some(Ok(projects)) => rsx! {
                    ul { class: "grid gap-3",
                        for project in projects {
                            ProjectRow { project }
                        }
                    }
                },
                Some(Err(error)) => rsx! {
                    p { class: "text-sm text-red-200", "{error}" }
                },
                None => rsx! {
                    p { class: "text-sm text-zinc-400", "Loading Projects" }
                },
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
    let reload = use_context::<ProjectStore>().reload_counter();
    let project_resource_id = id.clone();
    let project = use_resource(move || {
        let _ = reload.read();
        fetch_project(project_resource_id.clone())
    });
    let candidate_project_id = id.clone();
    let issue_source_candidates = use_resource(move || {
        let _ = reload.read();
        fetch_issue_source_candidates(candidate_project_id.clone())
    });

    rsx! {
        match &*project.read_unchecked() {
            Some(Ok(project)) if project.enabled_issue_source.is_some() => rsx! {
                IssueSourceSyncStatus { project_id: id.clone() }
            },
            _ => rsx! {},
        }
        match &*issue_source_candidates.read_unchecked() {
            Some(Ok(candidates)) => rsx! {
                IssueSourceCandidates {
                    project_id: id.clone(),
                    candidates: candidates.clone(),
                }
            },
            Some(Err(error)) => rsx! {
                StatusPanel {
                    title: "Issue Source candidates unavailable".to_string(),
                    detail: error.clone(),
                    tone: "border-red-700 bg-red-950/40 text-red-100".to_string(),
                }
            },
            None => rsx! {
                StatusPanel {
                    title: "Loading Issue Source candidates".to_string(),
                    detail: id.clone(),
                    tone: "border-zinc-700 bg-zinc-900 text-zinc-100".to_string(),
                }
            },
        }
    }
}

#[component]
fn AssignmentPanel(id: String) -> Element {
    let reload = use_context::<ProjectStore>().reload_counter();
    let assignment_project_id = id.clone();
    let assignment_state = use_resource(move || {
        let _ = reload.read();
        fetch_assignment_state(assignment_project_id.clone())
    });
    rsx! {
        match &*assignment_state.read_unchecked() {
            Some(Ok(state)) => rsx! { AssignmentState { project_id: id.clone(), state: state.clone() } },
            Some(Err(error)) => rsx! {
                StatusPanel {
                    title: "Issue Assignment state unavailable".to_string(),
                    detail: error.clone(),
                    tone: "border-zinc-700 bg-zinc-900 text-zinc-100".to_string(),
                }
            },
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
    let reload = use_context::<ProjectStore>().reload_counter();
    let planning_project_id = id.clone();
    let planning_snapshot = use_resource(move || {
        let _ = reload.read();
        fetch_planning_snapshot(planning_project_id.clone())
    });
    let project_resource_id = id.clone();
    let project = use_resource(move || {
        let _ = reload.read();
        fetch_project(project_resource_id.clone())
    });

    rsx! {
        match (&*planning_snapshot.read_unchecked(), &*project.read_unchecked()) {
            (Some(Ok(snapshot)), Some(Ok(project))) => rsx! {
                PlanningSnapshot {
                    project_id: id.clone(),
                    trusted: project.trusted,
                    snapshot: snapshot.clone(),
                }
            },
            (Some(Err(error)), _) => rsx! {
                StatusPanel {
                    title: "Planning snapshot unavailable".to_string(),
                    detail: error.clone(),
                    tone: "border-zinc-700 bg-zinc-900 text-zinc-100".to_string(),
                }
            },
            _ => rsx! {
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
    let reload = use_context::<ProjectStore>().reload_counter();
    let activity_project_id = id.clone();
    let activity = use_resource(move || {
        let _ = reload.read();
        fetch_project_activity(activity_project_id.clone())
    });
    rsx! {
        match &*activity.read_unchecked() {
            Some(Ok(entries)) => rsx! { ActivityPanel { entries: entries.clone() } },
            Some(Err(error)) => rsx! {
                StatusPanel {
                    title: "Activity unavailable".to_string(),
                    detail: error.clone(),
                    tone: "border-zinc-700 bg-zinc-900 text-zinc-100".to_string(),
                }
            },
            None => rsx! {
                ActivityPanel { entries: Vec::<ProjectActivityEntryResponse>::new() }
            },
        }
    }
}

#[component]
fn SettingsPanel(info: AppInfoResponse) -> Element {
    rsx! {
        section { class: "rounded-lg border border-zinc-800 bg-zinc-900 p-5",
            h2 { class: "mb-4 text-base font-semibold", "Settings" }
            dl { class: "grid gap-3 text-sm",
                SettingRow { label: "Bind address".to_string(), value: info.config.bind_address }
                SettingRow { label: "Dashboard assets".to_string(), value: info.config.dashboard_asset_dir }
                SettingRow { label: "Database".to_string(), value: info.config.database_url }
            }
        }
    }
}

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
    rsx! {
        li { class: "grid gap-2 border-b border-zinc-800 pb-3 last:border-0 last:pb-0",
            div { class: "flex flex-col gap-1 md:flex-row md:items-baseline md:justify-between",
                div { class: "flex items-center gap-2",
                    Link {
                        to: Route::ProjectOverview { id: project.id.0.clone() },
                        class: "break-words font-mono text-sm text-emerald-200 hover:text-emerald-100",
                        "{project.path}"
                    }
                    if project.trusted {
                        span { class: "rounded bg-emerald-900/40 px-1.5 py-0.5 text-xs text-emerald-200", "Trusted" }
                    }
                }
                p { class: "font-mono text-xs text-zinc-500", "{project.id.0}" }
            }
            match project.git_summary {
                Some(summary) => rsx! { GitSummaryRow { summary } },
                None => rsx! {
                    p { class: "text-sm text-zinc-500", "No Git Summary" }
                },
            }
        }
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
    let reload = use_context::<ProjectStore>().reload_counter();
    let sync_project_id = project_id.clone();
    let sync_status = use_resource(move || {
        let _ = reload.read();
        fetch_issue_source_sync_status(sync_project_id.clone())
    });
    let refresh_project_id = project_id.clone();

    rsx! {
        section { class: "rounded-lg border border-zinc-800 bg-zinc-900 p-5",
            div { class: "flex flex-col gap-3 md:flex-row md:items-start md:justify-between",
                div {
                    h2 { class: "text-base font-semibold", "Last sync status" }
                    match &*sync_status.read_unchecked() {
                        Some(Ok(status)) => rsx! {
                            p { class: "mt-2 font-mono text-sm text-zinc-300",
                                {status.last_successful_sync_at.clone().unwrap_or_else(|| "Never synced".to_string())}
                            }
                            if let Some(failure) = status.last_failure.clone() {
                                p { class: "mt-2 text-sm text-red-100", "{failure}" }
                            }
                        },
                        Some(Err(error)) => rsx! {
                            p { class: "mt-2 text-sm text-red-100", "{error}" }
                        },
                        None => rsx! {
                            p { class: "mt-2 text-sm text-zinc-500", "Loading" }
                        },
                    }
                }
                button {
                    class: "rounded border border-emerald-700 px-3 py-2 text-sm font-medium text-emerald-100 hover:border-emerald-500 hover:bg-emerald-950/45",
                    onclick: move |_| {
                        let project_id = refresh_project_id.clone();
                        async move {
                            let _ = sync_issue_source(project_id).await;
                            reload_dashboard();
                        }
                    },
                    "Refresh Issue Source"
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
    let enable_project_id = project_id.clone();
    let enable_kind = candidate.kind.clone();
    let enable_locator = candidate.locator.clone();

    rsx! {
        li { class: "flex flex-col gap-2 border-b border-zinc-800 pb-3 text-sm text-zinc-100 last:border-0 last:pb-0 md:flex-row md:items-center md:justify-between",
            p { class: "break-words font-mono", "{candidate.kind} {candidate.locator}" }
            if candidate.enabled {
                p { class: "text-xs text-emerald-200", "Enabled" }
            } else {
                button {
                    class: "rounded border border-emerald-700 px-3 py-2 text-left text-xs font-medium text-emerald-100 hover:border-emerald-500 hover:bg-emerald-950/45",
                    onclick: move |_| {
                        let project_id = enable_project_id.clone();
                        let kind = enable_kind.clone();
                        let locator = enable_locator.clone();
                        async move {
                            if enable_issue_source(project_id, kind, locator).await.is_ok() {
                                reload_dashboard();
                            }
                        }
                    },
                    "Enable {candidate.kind} {candidate.locator}"
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
    let dependencies = if issue.issue_dependencies.is_empty() {
        "No dependencies".to_string()
    } else {
        issue.issue_dependencies.join(", ")
    };
    let parent = issue
        .parent_issue
        .clone()
        .unwrap_or_else(|| "No parent".to_string());
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
                    class: "mt-2 rounded border border-emerald-700 px-2.5 py-1.5 text-left text-xs font-medium text-emerald-100 hover:border-emerald-500 hover:bg-emerald-950/45",
                    onclick: move |_| {
                        let project_id = start_project_id.clone();
                        let source_id = start_source_id.clone();
                        async move {
                            let _ = start_assignment(project_id, source_id).await;
                            reload_dashboard();
                        }
                    },
                    "Start Assignment"
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
                    let refresh_project_id = assignment.project_id.0.clone();
                    let refresh_assignment_id = assignment.id.clone();
                    let abandon_project_id = project_id.clone();
                    let abandon_assignment_id = assignment.id.clone();
                    let recover_project_id = project_id.clone();
                    let recover_assignment_id = assignment.id.clone();
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
                                button {
                                    class: "mt-2 w-fit rounded border border-emerald-700 px-2.5 py-1.5 text-left text-xs font-medium text-emerald-100 hover:border-emerald-500 hover:bg-emerald-950/45",
                                    onclick: move |_| {
                                        let project_id = refresh_project_id.clone();
                                        let assignment_id = refresh_assignment_id.clone();
                                        async move {
                                            let _ = refresh_proposal_state(project_id, assignment_id).await;
                                            reload_dashboard();
                                        }
                                    },
                                    "Refresh Proposal State"
                                }
                            }
                            if can_recover {
                                button {
                                    class: "mt-2 w-fit rounded border border-amber-700 px-2.5 py-1.5 text-left text-xs font-medium text-amber-100 hover:border-amber-500 hover:bg-amber-950/45",
                                    onclick: move |_| {
                                        let project_id = recover_project_id.clone();
                                        let assignment_id = recover_assignment_id.clone();
                                        async move {
                                            let _ = recover_assignment(project_id, assignment_id).await;
                                            reload_dashboard();
                                        }
                                    },
                                    "Recover Assignment"
                                }
                            }
                            if can_abandon {
                                button {
                                    class: "mt-2 w-fit rounded border border-rose-700 px-2.5 py-1.5 text-left text-xs font-medium text-rose-100 hover:border-rose-500 hover:bg-rose-950/45",
                                    onclick: move |_| {
                                        let project_id = abandon_project_id.clone();
                                        let assignment_id = abandon_assignment_id.clone();
                                        async move {
                                            let _ = abandon_assignment(project_id, assignment_id).await;
                                            reload_dashboard();
                                        }
                                    },
                                    "Abandon Assignment"
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

async fn refresh_proposal_state(
    project_id: String,
    assignment_id: String,
) -> Result<agentic_afk_contracts::IssueAssignmentResponse, String> {
    gloo_net::http::Request::post(&format!(
        "/api/projects/{project_id}/assignments/{assignment_id}/refresh-proposal-state"
    ))
    .send()
    .await
    .map_err(|error| error.to_string())?
    .json()
    .await
    .map_err(|error| error.to_string())
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

async fn fetch_planning_snapshot(project_id: String) -> Result<PlanningSnapshotResponse, String> {
    gloo_net::http::Request::get(&format!("/api/projects/{project_id}/planning-snapshot"))
        .send()
        .await
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())
}

async fn fetch_assignment_state(
    project_id: String,
) -> Result<ProjectAssignmentStateResponse, String> {
    gloo_net::http::Request::get(&format!("/api/projects/{project_id}/assignment-state"))
        .send()
        .await
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())
}

async fn fetch_project_activity(
    project_id: String,
) -> Result<Vec<ProjectActivityEntryResponse>, String> {
    gloo_net::http::Request::get(&format!("/api/projects/{project_id}/activity"))
        .send()
        .await
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())
}

async fn fetch_issue_source_candidates(
    project_id: String,
) -> Result<Vec<IssueSourceCandidate>, String> {
    gloo_net::http::Request::get(&format!(
        "/api/projects/{project_id}/issue-source-candidates"
    ))
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
) -> Result<ProjectResponse, String> {
    gloo_net::http::Request::put(&format!("/api/projects/{project_id}/issue-source"))
        .json(&EnableIssueSourceRequest { kind, locator })
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())
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

async fn start_assignment(
    project_id: String,
    source_id: String,
) -> Result<agentic_afk_contracts::IssueAssignmentResponse, String> {
    gloo_net::http::Request::post(&format!(
        "/api/projects/{project_id}/source-issues/{source_id}/assignment"
    ))
    .send()
    .await
    .map_err(|error| error.to_string())?
    .json()
    .await
    .map_err(|error| error.to_string())
}

async fn abandon_assignment(
    project_id: String,
    assignment_id: String,
) -> Result<agentic_afk_contracts::IssueAssignmentResponse, String> {
    gloo_net::http::Request::post(&format!(
        "/api/projects/{project_id}/assignments/{assignment_id}/abandon"
    ))
    .send()
    .await
    .map_err(|error| error.to_string())?
    .json()
    .await
    .map_err(|error| error.to_string())
}

async fn recover_assignment(
    project_id: String,
    assignment_id: String,
) -> Result<agentic_afk_contracts::IssueAssignmentResponse, String> {
    gloo_net::http::Request::post(&format!(
        "/api/projects/{project_id}/assignments/{assignment_id}/recover"
    ))
    .send()
    .await
    .map_err(|error| error.to_string())?
    .json()
    .await
    .map_err(|error| error.to_string())
}

async fn fetch_issue_source_sync_status(
    project_id: String,
) -> Result<IssueSourceSyncStatusResponse, String> {
    gloo_net::http::Request::get(&format!(
        "/api/projects/{project_id}/issue-source/sync-status"
    ))
    .send()
    .await
    .map_err(|error| error.to_string())?
    .json()
    .await
    .map_err(|error| error.to_string())
}

async fn sync_issue_source(project_id: String) -> Result<IssueSourceSyncResponse, String> {
    gloo_net::http::Request::post(&format!("/api/projects/{project_id}/issue-source/sync"))
        .send()
        .await
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())
}

fn reload_dashboard() {
    let _ = web_sys::window().map(|window| window.location().reload());
}
