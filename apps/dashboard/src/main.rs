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
    AppInfoResponse, AutoReplanState, EnableIssueSourceRequest, GitSummary,
    IssueAssignmentResponse, IssueSourceCandidate, IssueSourceSyncResponse, PauseReason,
    PlanRunOutcome, PlanRunResponse, PlanRunStage, PlanningSnapshotResponse,
    ProjectActivityEntryResponse, ProjectEvent, ProjectExecutionConfigResponse, ProjectId,
    ProjectResponse, ProjectSnapshotResponse, SetProjectExecutionConfigRequest, SourceIssueSnapshot,
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

    // Hold the last successful `ProjectResponse` so refetches (triggered by
    // every mutation via `bump_reload`) don't unmount the Outlet and lose the
    // browser's scroll anchor. We still surface the latest error — if the
    // refetch fails, ErrorState wins over stale data.
    let mut last_ok: Signal<Option<ProjectResponse>> = use_signal(|| None);
    let resource_read = project.read_unchecked();
    let (current_ok, current_err) = match &*resource_read {
        Some(Ok(p)) => (Some(p.clone()), None),
        Some(Err(e)) => (None, Some(e.clone())),
        None => (None, None),
    };
    drop(resource_read);
    if let Some(p) = &current_ok {
        if last_ok.read().as_ref() != Some(p) {
            last_ok.set(Some(p.clone()));
        }
    }

    rsx! {
        div { class: "flex flex-col gap-6",
            if let Some(error) = current_err {
                ErrorState {
                    title: "Project unavailable".to_string(),
                    detail: error,
                    problem_json: None,
                }
            } else if let Some(project) = last_ok.read().clone() {
                ProjectCrumb { project }
                ProjectSubNav { id: id.clone() }
                Outlet::<Route> {}
            } else {
                Card {
                    CardHead {
                        title: "Loading Project".to_string(),
                        id_text: Some(short_project_id(&id)),
                    }
                    CardBody {
                        SkeletonHeading {}
                        SkeletonLine { width_percent: 60 }
                        SkeletonLine { width_percent: 40 }
                    }
                }
            }
        }
    }
}

/// Slim breadcrumb above the sub-nav: back link + project path. Heavier
/// metadata lives in `ProjectOverview`.
#[component]
fn ProjectCrumb(project: ProjectResponse) -> Element {
    rsx! {
        div { class: "flex flex-wrap items-baseline gap-x-4 gap-y-1",
            Link {
                to: Route::ProjectList {},
                class: "font-display text-[11px] uppercase tracking-[0.22em] text-ink-2 hover:text-cyan",
                "← Projects"
            }
            p { class: "break-words font-mono text-[13px] text-cyan", "{project.path}" }
            p { class: "font-mono text-[11px] text-ink-dim", "{short_project_id(&project.id.0)}" }
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
        last_project_id.clone().set(Some(project_id.clone()));
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
    let link_class = "border border-stroke px-3 py-1.5 font-display text-[11px] uppercase tracking-[0.22em] text-ink-2 hover:border-cyan hover:text-cyan";
    rsx! {
        nav { class: "flex flex-wrap gap-2",
            Link { to: Route::ProjectOverview { id: id.clone() }, class: link_class, "Overview" }
            Link { to: Route::ProjectPlanning { id: id.clone() }, class: link_class, "Planning" }
            Link { to: Route::ProjectIssueSource { id: id.clone() }, class: link_class, "Issue Source" }
            Link { to: Route::ProjectActivity { id: id.clone() }, class: link_class, "Activity" }
        }
    }
}

/// Trust button bound to `ActionButton`. Pending/disabled/error wiring is
/// owned by `ActionButton`; this component only knows the mutation future
/// to invoke on press and the post-success toast to announce.
#[component]
fn TrustProjectButton(project_id: String) -> Element {
    let store = use_context::<ProjectStore>();
    let key = MutationKey::TrustProject(ProjectId(project_id.clone()));
    rsx! {
        ActionButton {
            mutation_key: key.clone(),
            variant: ButtonVariant::Primary,
            testid: "trust-project-button".to_string(),
            error_marker: "trust-project".to_string(),
            on_press: {
                let key = key.clone();
                let project_id = project_id.clone();
                move |_| {
                    let key = key.clone();
                    let project_id = project_id.clone();
                    // `wasm_bindgen_futures::spawn_local` is unbound to the
                    // ActionButton's scope, so the post-success toast still
                    // fires after the button unmounts on trust=true.
                    wasm_bindgen_futures::spawn_local(async move {
                        let result = store
                            .mutate(key, trust_project_api(project_id.clone()))
                            .await;
                        if result.is_ok() {
                            store.push_success(
                                "Project trusted",
                                "Agent execution is now allowed",
                            );
                        }
                    });
                }
            },
            "Trust Project"
        }
    }
}

#[component]
fn AutoReplanControl(project: ProjectResponse) -> Element {
    let store = use_context::<ProjectStore>();
    let project_id = project.id.0.clone();
    let (tone, label) = derive_auto_replan_pill(project.auto_replan_state);
    let (key, button_label, variant, endpoint) = match project.auto_replan_state {
        AutoReplanState::Off => (
            MutationKey::ArmAutoReplan(project.id.clone()),
            "Arm",
            ButtonVariant::Primary,
            "arm",
        ),
        AutoReplanState::Armed => (
            MutationKey::DisarmAutoReplan(project.id.clone()),
            "Disarm",
            ButtonVariant::Destructive,
            "disarm",
        ),
        AutoReplanState::Paused => (
            MutationKey::ResumeAutoReplan(project.id.clone()),
            "Resume",
            ButtonVariant::Primary,
            "resume",
        ),
    };
    rsx! {
        div { class: "flex flex-wrap items-center gap-2",
            StatusPill { tone, label: label.to_string() }
            ActionButton {
                mutation_key: key.clone(),
                variant,
                testid: Some("auto-replan-action".to_string()),
                error_marker: Some("auto-replan".to_string()),
                on_press: {
                    let store = store.clone();
                    let key = key.clone();
                    let project_id = project_id.clone();
                    let endpoint = endpoint.to_string();
                    move |_| {
                        let store = store.clone();
                        let key = key.clone();
                        let project_id = project_id.clone();
                        let endpoint = endpoint.clone();
                        spawn(async move {
                            let _ = store.mutate(key, auto_replan_api(project_id, endpoint)).await;
                        });
                    }
                },
                "{button_label}"
            }
        }
    }
}

#[component]
fn ProjectOverview(id: String) -> Element {
    let state_sig = use_context::<ProjectStore>().state_signal();
    let s = state_sig.read();
    let project = s.project.clone();
    let execution_config = s.execution_config.clone();
    let active_plan_run = s.active_plan_run.clone();
    let recent_plan_runs = s.recent_plan_runs.clone();
    drop(s);

    rsx! {
        match project {
            Some(project) => rsx! {
                AutoReplanPausedBanner { project: project.clone() }
                div { class: "grid gap-6 lg:grid-cols-2",
                    ProjectMetaCard { project: project.clone() }
                }
                ExecutionConfigCard {
                    project_id: project.id.0.clone(),
                    config: execution_config.clone(),
                    detected_default_branch: project
                        .git_summary
                        .as_ref()
                        .and_then(|gs| gs.default_branch.clone()),
                }
                PlanRunCard {
                    project_id: project.id.0.clone(),
                    trusted: project.trusted,
                    has_config: execution_config.is_some(),
                    active: active_plan_run.clone(),
                    recent: recent_plan_runs.clone(),
                }
                GitSummaryCard { git_summary: project.git_summary.clone() }
            },
            None => rsx! {
                Card {
                    CardHead {
                        title: "Project".to_string(),
                        id_text: Some(short_project_id(&id)),
                    }
                    CardBody {
                        SkeletonHeading {}
                        SkeletonLine { width_percent: 70 }
                        SkeletonLine { width_percent: 40 }
                    }
                }
            },
        }
    }
}

#[component]
fn AutoReplanPausedBanner(project: ProjectResponse) -> Element {
    if project.auto_replan_state != AutoReplanState::Paused {
        return rsx! {};
    }

    let store = use_context::<ProjectStore>();
    let project_id = project.id.0.clone();
    let reason = project
        .auto_replan_pause_reason
        .unwrap_or(PauseReason::PlanningFailed);
    let copy = auto_replan_pause_copy(reason);
    let resume_key = MutationKey::ResumeAutoReplan(project.id.clone());
    let disarm_key = MutationKey::DisarmAutoReplan(project.id.clone());

    rsx! {
        div {
            class: "flex flex-col gap-3 border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-950 sm:flex-row sm:items-center sm:justify-between",
            "data-testid": "auto-replan-paused-banner",
            div { class: "flex min-w-0 flex-col gap-1",
                div { class: "flex flex-wrap items-center gap-2",
                    StatusPill { tone: PillTone::Failed, label: "Paused".to_string() }
                    p { class: "font-semibold", "Auto-Replan paused" }
                }
                p { class: "text-red-900", "{copy}" }
            }
            div { class: "flex shrink-0 flex-wrap items-center gap-2",
                ActionButton {
                    mutation_key: resume_key.clone(),
                    variant: ButtonVariant::Primary,
                    testid: Some("auto-replan-banner-resume".to_string()),
                    error_marker: Some("auto-replan".to_string()),
                    on_press: {
                        let store = store.clone();
                        let key = resume_key.clone();
                        let project_id = project_id.clone();
                        move |_| {
                            let store = store.clone();
                            let key = key.clone();
                            let project_id = project_id.clone();
                            spawn(async move {
                                let _ = store
                                    .mutate(key, auto_replan_api(project_id, "resume".to_string()))
                                    .await;
                            });
                        }
                    },
                    "Resume"
                }
                ActionButton {
                    mutation_key: disarm_key.clone(),
                    variant: ButtonVariant::Default,
                    testid: Some("auto-replan-banner-disarm".to_string()),
                    error_marker: Some("auto-replan".to_string()),
                    on_press: {
                        let store = store.clone();
                        let key = disarm_key.clone();
                        let project_id = project_id.clone();
                        move |_| {
                            let store = store.clone();
                            let key = key.clone();
                            let project_id = project_id.clone();
                            spawn(async move {
                                let _ = store
                                    .mutate(key, auto_replan_api(project_id, "disarm".to_string()))
                                    .await;
                            });
                        }
                    },
                    "Disarm"
                }
            }
        }
    }
}

#[component]
fn ExecutionConfigCard(
    project_id: String,
    config: Option<ProjectExecutionConfigResponse>,
    detected_default_branch: Option<String>,
) -> Element {
    let store = use_context::<ProjectStore>();
    let key = MutationKey::SetExecutionConfig(ProjectId(project_id.clone()));
    let mut integration_branch = use_signal(|| {
        config
            .as_ref()
            .map(|c| c.integration_branch.clone())
            .or_else(|| detected_default_branch.clone())
            .unwrap_or_else(|| "main".to_string())
    });
    let mut max_parallel_tasks = use_signal(|| {
        config
            .as_ref()
            .map(|c| c.max_parallel_tasks.to_string())
            .unwrap_or_else(|| "1".to_string())
    });
    let mut review_retry_limit = use_signal(|| {
        config
            .as_ref()
            .map(|c| c.review_retry_limit.to_string())
            .unwrap_or_else(|| "3".to_string())
    });

    let submit = use_callback({
        let store = store.clone();
        let key = key.clone();
        let project_id = project_id.clone();
        move |()| {
            let max = max_parallel_tasks.read().parse::<i64>().unwrap_or(0);
            let retry = review_retry_limit.read().parse::<i64>().unwrap_or(0);
            let request = SetProjectExecutionConfigRequest {
                integration_branch: integration_branch.read().clone(),
                max_parallel_tasks: max,
                review_retry_limit: retry,
            };
            let store = store.clone();
            let key = key.clone();
            let project_id = project_id.clone();
            spawn(async move {
                let _ = store
                    .mutate(key, set_execution_config_api(project_id, request))
                    .await;
            });
        }
    });

    rsx! {
        Card {
            CardHead { title: "Execution Config".to_string(), id_text: None }
            CardBody {
                form {
                    "data-testid": "execution-config-form",
                    class: "flex flex-col gap-3",
                    onsubmit: move |event| {
                        event.prevent_default();
                        submit.call(());
                    },
                    label { class: "flex flex-col gap-1 text-[12px] text-ink-2",
                        "Integration Branch"
                        input {
                            "data-testid": "execution-config-integration-branch",
                            class: "bg-bg-2 px-2 py-1 font-mono text-[12px] text-ink",
                            r#type: "text",
                            value: integration_branch.read().clone(),
                            oninput: move |e| integration_branch.set(e.value()),
                        }
                    }
                    label { class: "flex flex-col gap-1 text-[12px] text-ink-2",
                        "Max Parallel Tasks"
                        input {
                            "data-testid": "execution-config-max-parallel-tasks",
                            class: "bg-bg-2 px-2 py-1 font-mono text-[12px] text-ink",
                            r#type: "number",
                            min: "1",
                            value: max_parallel_tasks.read().clone(),
                            oninput: move |e| max_parallel_tasks.set(e.value()),
                        }
                    }
                    label { class: "flex flex-col gap-1 text-[12px] text-ink-2",
                        "Review Retry Limit"
                        input {
                            "data-testid": "execution-config-review-retry-limit",
                            class: "bg-bg-2 px-2 py-1 font-mono text-[12px] text-ink",
                            r#type: "number",
                            min: "1",
                            value: review_retry_limit.read().clone(),
                            oninput: move |e| review_retry_limit.set(e.value()),
                        }
                    }
                    ActionButton {
                        mutation_key: key,
                        variant: ButtonVariant::Primary,
                        testid: Some("execution-config-save".to_string()),
                        on_press: move |_| submit.call(()),
                        "Save Execution Config"
                    }
                }
            }
        }
    }
}

#[component]
fn PlanRunCard(
    project_id: String,
    trusted: bool,
    has_config: bool,
    active: Option<PlanRunResponse>,
    recent: Vec<PlanRunResponse>,
) -> Element {
    let store = use_context::<ProjectStore>();
    let start_key = MutationKey::StartPlanRun(ProjectId(project_id.clone()));
    let can_start = trusted && has_config && active.is_none();
    let start = {
        let store = store.clone();
        let start_key = start_key.clone();
        let project_id = project_id.clone();
        move |_| {
            let store = store.clone();
            let start_key = start_key.clone();
            let project_id = project_id.clone();
            spawn(async move {
                let _ = store
                    .mutate(start_key, start_plan_run_api(project_id))
                    .await;
            });
        }
    };
    rsx! {
        Card {
            CardHead { title: "Plan Run".to_string(), id_text: None }
            CardBody {
                div { class: "flex flex-col gap-3", "data-testid": "plan-run-card",
                    match active {
                        Some(active) => {
                            let id_short = active.id.chars().take(8).collect::<String>();
                            let (stage_tone, stage_label) = derive_plan_run_stage_pill(active.stage);
                            let assignments = active.assignments.clone();
                            // Plan-Run-scoped Phase Output rows (assignment_id =
                            // None) — currently the typed `Push` rows recorded
                            // by the Merge Phase push boundary and operator
                            // Retry Push (ADR-0038 / #61). Append-only,
                            // ordered by `recorded_at`.
                            let mut header_phase_outputs: Vec<agentic_afk_contracts::PhaseOutputResponse> = active
                                .phase_outputs
                                .iter()
                                .filter(|p| p.assignment_id.is_none())
                                .cloned()
                                .collect();
                            header_phase_outputs.sort_by(|a, b| a.recorded_at.cmp(&b.recorded_at));
                            rsx! {
                                div { class: "flex flex-col gap-2", "data-testid": "plan-run-active",
                                    StatusPill { tone: stage_tone, label: stage_label.to_string() }
                                    p { class: "font-mono text-[12px] text-ink", "{id_short}" }
                                    p { class: "text-[12px] text-ink-2",
                                        "{active.integration_branch} @ {active.baseline_commit}"
                                    }
                                    if !header_phase_outputs.is_empty() {
                                        div { class: "mt-1 flex flex-col gap-1",
                                            "data-testid": "plan-run-card-header-phase-outputs",
                                            for phase_output in header_phase_outputs.iter() {
                                                PhaseOutputRow { phase_output: phase_output.clone() }
                                            }
                                        }
                                    }
                                    if !assignments.is_empty() {
                                        div { class: "mt-1 flex flex-col gap-1",
                                            "data-testid": "plan-run-assignments",
                                            p { class: "text-[11px] text-ink-2", "Selected Issue Assignments" }
                                            for assignment in assignments.iter() {
                                                PlanRunAssignmentRow {
                                                    assignment: assignment.clone(),
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        None => rsx! {
                            EmptyState {
                                title: "No active Plan Run".to_string(),
                                body: "Start one to plan the next batch.".to_string(),
                                accent: EmptyStateAccent::Cyan,
                            }
                        },
                    }
                    ActionButton {
                        mutation_key: start_key,
                        variant: ButtonVariant::Primary,
                        disabled: !can_start,
                        on_press: move |_| start(()),
                        testid: Some("start-plan-run".to_string()),
                        "Start Plan Run"
                    }
                }
                if !recent.is_empty() {
                    div { class: "mt-4 flex flex-col gap-2", "data-testid": "plan-run-history",
                        p { class: "text-[12px] text-ink-2", "Recent Plan Runs" }
                        for plan_run in recent.iter().take(5) {
                            PlanRunHistoryRow { plan_run: plan_run.clone() }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PlanRunAssignmentRow(assignment: IssueAssignmentResponse) -> Element {
    let tone = match assignment.status.as_str() {
        "merged" => PillTone::Verified,
        "reviewed" => PillTone::Verified,
        "implemented" | "claimed" | "implementing" | "merging" => PillTone::Running,
        // ADR-0037: `merge_staged` is dormant — local integration is done
        // but the Integration Branch push has not yet succeeded. Render
        // it as a pending state (read-only badge for this slice; Retry
        // Push / Abandon Staged affordances land in follow-up work).
        "merge_staged" => PillTone::Pending,
        "rejected" | "blocked" => PillTone::Failed,
        _ => PillTone::Pending,
    };
    let status_testid = match assignment.status.as_str() {
        "merge_staged" => Some("assignment-status-merge-staged"),
        _ => None,
    };
    let summary = assignment
        .selection_summary
        .clone()
        .unwrap_or_else(|| String::from("(no selection summary)"));
    let phase_outputs = assignment.phase_outputs.clone();
    let review_rejection_count = assignment.review_rejection_count;
    let block_reason = assignment.block_reason.clone();
    let is_blocked = assignment.status == "blocked";
    let is_merge_staged = assignment.status == "merge_staged";
    let project_id = assignment.project_id.0.clone();
    let assignment_id = assignment.id.clone();
    rsx! {
        div { class: "flex flex-col gap-1 rounded border border-line/40 bg-surface-2/40 p-2",
            "data-testid": "plan-run-assignment-row",
            div { class: "flex items-center gap-2",
                if let Some(testid) = status_testid {
                    span { "data-testid": testid,
                        StatusPill { tone, label: assignment.status.clone() }
                    }
                } else {
                    StatusPill { tone, label: assignment.status.clone() }
                }
                p { class: "font-mono text-[12px] text-ink",
                    "{assignment.source_id}: {assignment.source_title}"
                }
            }
            p { class: "font-mono text-[11px] text-ink-2", "{assignment.branch}" }
            p { class: "text-[11px] text-ink-2", "{summary}" }
            if review_rejection_count > 0 {
                p { class: "font-mono text-[11px] text-coral",
                    "data-testid": "assignment-review-rejection-count",
                    "Review rejections: {review_rejection_count}"
                }
            }
            if let Some(reason) = block_reason.clone() {
                p { class: "font-mono text-[11px] text-coral",
                    "data-testid": "assignment-block-reason-kind",
                    "Blocked: {reason.kind.as_wire()}"
                }
                if let Some(detail) = reason.detail.clone() {
                    p { class: "font-mono text-[11px] text-coral",
                        "data-testid": "assignment-block-reason-detail",
                        "{detail}"
                    }
                }
            }
            if !phase_outputs.is_empty() {
                div { class: "mt-1 flex flex-col gap-0.5",
                    "data-testid": "assignment-phase-outputs",
                    for phase_output in phase_outputs.iter() {
                        AssignmentPhaseOutputRow { phase_output: phase_output.clone() }
                    }
                }
            }
            if is_blocked {
                ReEnableSourceIssueButton {
                    project_id: project_id.clone(),
                    source_id: assignment.source_id.clone(),
                }
            }
            if is_merge_staged {
                div { class: "flex gap-2",
                    RetryPushAssignmentButton {
                        project_id: project_id.clone(),
                        assignment_id: assignment_id.clone(),
                    }
                    AbandonStagedAssignmentButton {
                        project_id,
                        assignment_id,
                    }
                }
            }
        }
    }
}

#[component]
fn RetryPushAssignmentButton(project_id: String, assignment_id: String) -> Element {
    let store = use_context::<ProjectStore>();
    let key = MutationKey::RetryPushAssignment(
        ProjectId(project_id.clone()),
        IssueAssignmentId(assignment_id.clone()),
    );
    rsx! {
        ActionButton {
            mutation_key: key.clone(),
            variant: ButtonVariant::Primary,
            testid: "retry-push-assignment-button".to_string(),
            error_marker: "retry-push-assignment".to_string(),
            on_press: {
                let key = key.clone();
                let project_id = project_id.clone();
                let assignment_id = assignment_id.clone();
                move |_| {
                    let key = key.clone();
                    let project_id = project_id.clone();
                    let assignment_id = assignment_id.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        let result = store
                            .mutate(
                                key,
                                retry_push_assignment_api(project_id, assignment_id),
                            )
                            .await;
                        if let Ok(response) = result {
                            match response.status.as_str() {
                                "merged" => store.push_success(
                                    "Retry Push succeeded",
                                    "Assignment merged; Plan Run history preserves the original failure",
                                ),
                                "blocked" => store.push_success(
                                    "Retry Push rejected as non-fast-forward",
                                    "Integration Branch has diverged; recovery belongs in a new Plan Run",
                                ),
                                _ => store.push_success(
                                    "Retry Push failed",
                                    "Assignment remains staged; you may retry or abandon",
                                ),
                            }
                        }
                    });
                }
            },
            "Retry Push"
        }
    }
}

#[component]
fn AbandonStagedAssignmentButton(project_id: String, assignment_id: String) -> Element {
    let store = use_context::<ProjectStore>();
    let key = MutationKey::AbandonStagedAssignment(
        ProjectId(project_id.clone()),
        IssueAssignmentId(assignment_id.clone()),
    );
    rsx! {
        ActionButton {
            mutation_key: key.clone(),
            variant: ButtonVariant::Destructive,
            testid: "abandon-staged-assignment-button".to_string(),
            error_marker: "abandon-staged-assignment".to_string(),
            on_press: {
                let key = key.clone();
                let project_id = project_id.clone();
                let assignment_id = assignment_id.clone();
                move |_| {
                    let key = key.clone();
                    let project_id = project_id.clone();
                    let assignment_id = assignment_id.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        let result = store
                            .mutate(
                                key,
                                abandon_staged_assignment_api(project_id, assignment_id, None),
                            )
                            .await;
                        if result.is_ok() {
                            store.push_success(
                                "Abandon Staged",
                                "Assignment blocked; staged work will not land",
                            );
                        }
                    });
                }
            },
            "Abandon Staged"
        }
    }
}

#[component]
fn ReEnableSourceIssueButton(project_id: String, source_id: String) -> Element {
    let store = use_context::<ProjectStore>();
    let key = MutationKey::ReEnableSourceIssue(
        ProjectId(project_id.clone()),
        SourceIssueId(source_id.clone()),
    );
    rsx! {
        ActionButton {
            mutation_key: key.clone(),
            variant: ButtonVariant::Primary,
            testid: "re-enable-source-issue-button".to_string(),
            error_marker: "re-enable-source-issue".to_string(),
            on_press: {
                let key = key.clone();
                let project_id = project_id.clone();
                let source_id = source_id.clone();
                move |_| {
                    let key = key.clone();
                    let project_id = project_id.clone();
                    let source_id = source_id.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        let result = store
                            .mutate(
                                key,
                                re_enable_source_issue_api(project_id, source_id),
                            )
                            .await;
                        if let Ok(outcome) = result {
                            if outcome.writeback.ok {
                                store.push_success(
                                    "Source Issue re-enabled",
                                    "A later Plan Run may pick this Source Issue again",
                                );
                            } else {
                                let detail = outcome
                                    .writeback
                                    .error
                                    .clone()
                                    .unwrap_or_else(|| "Issue Source write-back failed".to_string());
                                // Partial success per ADR-0035: local clear
                                // succeeded but upstream Lifecycle write-back
                                // failed. Surface as a success entry with the
                                // failure detail so the operator sees both
                                // halves without an additional fetch.
                                store.push_success(
                                    "Source Issue re-enabled (upstream write-back failed)",
                                    &detail,
                                );
                            }
                        }
                    });
                }
            },
            "Re-enable Source Issue"
        }
    }
}

#[component]
fn AssignmentPhaseOutputRow(phase_output: agentic_afk_contracts::PhaseOutputResponse) -> Element {
    rsx! { PhaseOutputRow { phase_output } }
}

/// Render one Phase Output row collapsed by default with a click-to-expand
/// body view (ADR-0038 / issue #58). Each row owns its own expansion signal
/// so multiple rows may be open at once.
#[component]
fn PhaseOutputRow(phase_output: agentic_afk_contracts::PhaseOutputResponse) -> Element {
    use agentic_afk_contracts::PhaseOutputBody;

    let tone = match phase_output.outcome.as_str() {
        "ready_for_review" | "approved" | "merged" | "succeeded" | "succeeded_empty" => {
            PillTone::Verified
        }
        "rejected" | "failed" | "blocked" => PillTone::Failed,
        _ => PillTone::Pending,
    };
    let summary = phase_output
        .body_json
        .get("summary")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let truncated_bytes = phase_output
        .body_json
        .get("truncated_at")
        .and_then(serde_json::Value::as_u64);
    // Permissive parse: in-the-wild rows that pre-date typed bodies fall
    // back to Failed via `from_legacy_value`, so the click-to-expand view
    // always has a typed body to render.
    let typed = PhaseOutputBody::from_legacy_value(phase_output.body_json.clone());
    let mut expanded = use_signal(|| false);
    let pill_label = format!("{} {}", phase_output.phase, phase_output.outcome);

    let row_testid = if phase_output.assignment_id.is_some() {
        "assignment-phase-output-row"
    } else {
        "plan-run-phase-output-row"
    };
    rsx! {
        div { class: "flex flex-col gap-1",
            "data-testid": row_testid,
            "data-expanded": if expanded() { "true" } else { "false" },
            button {
                r#type: "button",
                class: "flex items-center gap-2 text-left",
                "data-testid": "phase-output-toggle",
                onclick: move |_| {
                    let next = !expanded();
                    expanded.set(next);
                },
                StatusPill { tone, label: pill_label }
                if !summary.is_empty() {
                    p { class: "text-[11px] text-ink-2", "{summary}" }
                }
            }
            if expanded() {
                div { class: "rounded border border-line/40 bg-surface-2/60 p-2",
                    "data-testid": "phase-output-body",
                    PhaseOutputBodyView { body: typed }
                    if let Some(bytes) = truncated_bytes {
                        p { class: "mt-1 font-mono text-[11px] text-coral",
                            "data-testid": "phase-output-truncated",
                            "[truncated] body was {bytes} bytes"
                        }
                    }
                }
            }
        }
    }
}

/// Render the typed Phase Output body. The Failed variant is the only
/// variant fully covered by this slice; other variants fall back to a
/// pretty-printed JSON dump until follow-up slices tighten them.
#[component]
fn PhaseOutputBodyView(body: agentic_afk_contracts::PhaseOutputBody) -> Element {
    use agentic_afk_contracts::PhaseOutputBody;
    match body {
        PhaseOutputBody::Failed {
            error,
            problem_type,
        } => rsx! {
            div { class: "flex flex-col gap-1",
                p { class: "font-mono text-[12px] text-coral",
                    "data-testid": "phase-output-error",
                    "{error}"
                }
                if let Some(urn) = problem_type {
                    p { class: "font-mono text-[11px] text-ink-2",
                        "data-testid": "phase-output-problem-type",
                        "{urn}"
                    }
                }
            }
        },
        PhaseOutputBody::Implementation {
            commits,
            verification,
            gaps,
            summary,
        } => rsx! {
            div { class: "flex flex-col gap-2",
                "data-testid": "phase-output-implementation",
                if !summary.is_empty() {
                    p { class: "text-[12px] text-ink",
                        "data-testid": "phase-output-impl-summary",
                        "{summary}"
                    }
                }
                if !commits.is_empty() {
                    ul { class: "list-disc pl-4 text-[11px] text-ink",
                        "data-testid": "phase-output-commits",
                        for commit in commits.iter() {
                            li { "data-testid": "phase-output-commit", "{commit}" }
                        }
                    }
                }
                if !verification.is_empty() {
                    pre { class: "whitespace-pre-wrap font-mono text-[11px] text-ink-2 rounded border border-line/40 bg-surface-2/60 p-2",
                        "data-testid": "phase-output-verification",
                        "{verification.join(\"\\n\")}"
                    }
                }
                if !gaps.is_empty() {
                    ul { class: "list-disc pl-4 text-[11px] text-coral",
                        "data-testid": "phase-output-gaps",
                        for gap in gaps.iter() {
                            li { "data-testid": "phase-output-gap", "{gap}" }
                        }
                    }
                }
            }
        },
        PhaseOutputBody::Review {
            findings,
            verification,
            gaps,
            summary,
        } => rsx! {
            div { class: "flex flex-col gap-2",
                "data-testid": "phase-output-review",
                if !summary.is_empty() {
                    p { class: "text-[12px] text-ink",
                        "data-testid": "phase-output-review-summary",
                        "{summary}"
                    }
                }
                if !findings.is_empty() {
                    ol { class: "list-decimal pl-4 text-[11px] text-ink",
                        "data-testid": "phase-output-review-findings",
                        for finding in findings.iter() {
                            li { class: "flex flex-col",
                                "data-testid": "phase-output-review-finding",
                                if let Some(loc) = finding.location.as_ref() {
                                    span { class: "font-mono text-[10px] text-ink-2",
                                        "data-testid": "phase-output-review-finding-location",
                                        "{loc}"
                                    }
                                }
                                span {
                                    "data-testid": "phase-output-review-finding-message",
                                    "{finding.message}"
                                }
                            }
                        }
                    }
                }
                if !verification.is_empty() {
                    pre { class: "whitespace-pre-wrap font-mono text-[11px] text-ink-2 rounded border border-line/40 bg-surface-2/60 p-2",
                        "data-testid": "phase-output-verification",
                        "{verification.join(\"\\n\")}"
                    }
                }
                if !gaps.is_empty() {
                    ul { class: "list-disc pl-4 text-[11px] text-coral",
                        "data-testid": "phase-output-gaps",
                        for gap in gaps.iter() {
                            li { "data-testid": "phase-output-gap", "{gap}" }
                        }
                    }
                }
            }
        },
        PhaseOutputBody::Merge {
            merged_source_ids,
            verification,
            gaps,
            summary,
            block_reason,
        } => rsx! {
            div { class: "flex flex-col gap-2",
                "data-testid": "phase-output-merge",
                if !summary.is_empty() {
                    p { class: "text-[12px] text-ink",
                        "data-testid": "phase-output-merge-summary",
                        "{summary}"
                    }
                }
                if let Some(reason) = block_reason.as_ref() {
                    p { class: "text-[12px] text-coral",
                        "data-testid": "phase-output-merge-block-reason",
                        "{reason}"
                    }
                }
                if !merged_source_ids.is_empty() {
                    ul { class: "list-disc pl-4 text-[11px] text-ink",
                        "data-testid": "phase-output-merge-commits",
                        for source_id in merged_source_ids.iter() {
                            li { "data-testid": "phase-output-merge-source", "{source_id}" }
                        }
                    }
                }
                if !verification.is_empty() {
                    pre { class: "whitespace-pre-wrap font-mono text-[11px] text-ink-2 rounded border border-line/40 bg-surface-2/60 p-2",
                        "data-testid": "phase-output-merge-verification",
                        "{verification.join(\"\\n\")}"
                    }
                }
                if !gaps.is_empty() {
                    ul { class: "list-disc pl-4 text-[11px] text-coral",
                        "data-testid": "phase-output-merge-gaps",
                        for gap in gaps.iter() {
                            li { "data-testid": "phase-output-merge-gap", "{gap}" }
                        }
                    }
                }
            }
        },
        PhaseOutputBody::Push {
            stderr,
            fast_forward,
            attempt,
        } => rsx! {
            div { class: "flex flex-col gap-2",
                "data-testid": "phase-output-push",
                p { class: "font-mono text-[11px] text-ink-2",
                    "data-testid": "phase-output-push-attempt",
                    "attempt {attempt} \u{2022} fast_forward={fast_forward}"
                }
                if !stderr.is_empty() {
                    pre { class: "whitespace-pre-wrap font-mono text-[11px] text-coral rounded border border-line/40 bg-surface-2/60 p-2",
                        "data-testid": "phase-output-push-stderr",
                        "{stderr}"
                    }
                }
            }
        },
        PhaseOutputBody::Planning {
            selections,
            summary,
            rejected_candidates,
        } => {
            let is_empty = selections.is_empty() && rejected_candidates.is_empty();
            rsx! {
                div { class: "flex flex-col gap-2",
                    "data-testid": "phase-output-planning",
                    if !summary.is_empty() {
                        p { class: "text-[12px] text-ink",
                            "data-testid": "phase-output-planning-rationale",
                            "{summary}"
                        }
                    }
                    if !selections.is_empty() {
                        ul { class: "list-disc pl-4 text-[11px] text-ink",
                            "data-testid": "phase-output-planning-selections",
                            for selection in selections.iter() {
                                li { class: "flex flex-col",
                                    "data-testid": "phase-output-planning-selection",
                                    span {
                                        "data-testid": "phase-output-planning-selection-source",
                                        "{selection.source_issue_id} — {selection.title}"
                                    }
                                    span { class: "font-mono text-[10px] text-ink-2",
                                        "data-testid": "phase-output-planning-selection-branch",
                                        "{selection.branch}"
                                    }
                                    if !selection.selection_summary.is_empty() {
                                        span { class: "text-[11px] text-ink-2",
                                            "data-testid": "phase-output-planning-selection-summary",
                                            "{selection.selection_summary}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if !rejected_candidates.is_empty() {
                        ul { class: "list-disc pl-4 text-[11px] text-coral",
                            "data-testid": "phase-output-planning-rejected-list",
                            for candidate in rejected_candidates.iter() {
                                li { class: "flex flex-col",
                                    "data-testid": "phase-output-planning-rejected",
                                    span {
                                        "data-testid": "phase-output-planning-rejected-source",
                                        "{candidate.source_issue_id}"
                                    }
                                    span { class: "text-[11px] text-ink-2",
                                        "data-testid": "phase-output-planning-rejected-reason",
                                        "{candidate.reason}"
                                    }
                                }
                            }
                        }
                    }
                    if is_empty && summary.is_empty() {
                        p { class: "text-[12px] text-ink-2",
                            "data-testid": "phase-output-planning-empty",
                            "No eligible Source Issues selected for this Plan Run."
                        }
                    } else if is_empty {
                        p { class: "text-[12px] text-ink-2",
                            "data-testid": "phase-output-planning-empty",
                            "No selections."
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PlanRunHistoryRow(plan_run: PlanRunResponse) -> Element {
    let id_short = plan_run.id.chars().take(8).collect::<String>();
    let (tone, label) = derive_plan_run_history_pill(&plan_run);
    rsx! {
        div { class: "flex items-center gap-2", "data-testid": "plan-run-history-row",
            StatusPill { tone, label: label.to_string() }
            p { class: "font-mono text-[12px] text-ink", "{id_short}" }
            p { class: "text-[11px] text-ink-2",
                "{plan_run.integration_branch} @ {plan_run.baseline_commit}"
            }
        }
    }
}

fn derive_plan_run_stage_pill(stage: Option<PlanRunStage>) -> (PillTone, &'static str) {
    match stage {
        Some(PlanRunStage::Planning) => (PillTone::Pending, "Planning"),
        Some(PlanRunStage::Implementing) => (PillTone::Running, "Implementing"),
        Some(PlanRunStage::Reviewing) => (PillTone::Running, "Reviewing"),
        Some(PlanRunStage::Merging) => (PillTone::Running, "Merging"),
        Some(PlanRunStage::Pushing) => (PillTone::Running, "Pushing"),
        None => (PillTone::Running, "Running"),
    }
}

fn derive_plan_run_history_pill(plan_run: &PlanRunResponse) -> (PillTone, &'static str) {
    if plan_run.state == agentic_afk_contracts::PlanRunState::Running {
        return (PillTone::Pending, "Running");
    }
    match PlanRunResponse::classify_outcome(plan_run) {
        Some(PlanRunOutcome::EmptyBacklog) => (PillTone::Verified, "Succeeded (empty)"),
        Some(PlanRunOutcome::MergedWork) => (PillTone::Verified, "Finished"),
        Some(
            PlanRunOutcome::PlanningFailed
            | PlanRunOutcome::AssignmentBlocked
            | PlanRunOutcome::PushNonFastForward
            | PlanRunOutcome::MergeStagedLeft,
        ) => (PillTone::Failed, "Finished"),
        None => (PillTone::Verified, "Finished"),
    }
}

#[component]
fn ProjectMetaCard(project: ProjectResponse) -> Element {
    let (trust_tone, trust_label) = derive_trust_pill(project.trusted);
    let issue_source_label = project
        .enabled_issue_source
        .clone()
        .map(|s| format!("{} {}", s.kind, s.locator))
        .unwrap_or_else(|| "Not enabled".to_string());
    rsx! {
        Card {
            CardHead {
                title: "Project".to_string(),
                id_text: Some(short_project_id(&project.id.0)),
            }
            CardBody {
                div { class: "flex flex-col gap-4",
                    div { class: "flex flex-wrap items-center gap-2",
                        StatusPill { tone: trust_tone, label: trust_label.to_string() }
                        if !project.trusted {
                            TrustProjectButton { project_id: project.id.0.clone() }
                        }
                        AutoReplanControl { project: project.clone() }
                    }
                    KeyValueList {
                        KeyValueRow { label: "Path".to_string(), value: project.path.clone() }
                        KeyValueRow { label: "Project ID".to_string(), value: project.id.0.clone() }
                        KeyValueRow {
                            label: "Issue Source".to_string(),
                            value: issue_source_label,
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn GitSummaryCard(git_summary: Option<GitSummary>) -> Element {
    let (tone, label) = derive_git_pill(git_summary.as_ref());
    rsx! {
        Card {
            CardHead { title: "Git Summary".to_string(), id_text: None }
            CardBody {
                match git_summary {
                    Some(summary) => {
                        let branch = summary.branch.clone().unwrap_or_else(|| "detached".to_string());
                        let head = summary
                            .head
                            .clone()
                            .map(|h| h.chars().take(12).collect::<String>())
                            .unwrap_or_else(|| "unknown".to_string());
                        let state = if summary.dirty { "dirty" } else { "clean" };
                        rsx! {
                            div { class: "flex flex-col gap-4",
                                StatusPill { tone, label: label.clone() }
                                KeyValueList {
                                    KeyValueRow { label: "Branch".to_string(), value: branch }
                                    KeyValueRow { label: "Head".to_string(), value: head }
                                    KeyValueRow { label: "State".to_string(), value: state.to_string() }
                                }
                            }
                        }
                    },
                    None => rsx! {
                        EmptyState {
                            title: "No Git Summary".to_string(),
                            body: "Initialize a Git repository at the Project path to populate this card.".to_string(),
                            accent: EmptyStateAccent::Cyan,
                        }
                    },
                }
            }
        }
    }
}

#[component]
fn ProjectPlanning(id: String) -> Element {
    rsx! { PlanningPanel { id } }
}

// TODO: Plan Run UI (issue #41 Phase B) — restore an assignment surface once
// Plan Runs drive Issue Assignments. The legacy `/projects/:id/assignment`
// route and its `AssignmentPanel` / `AssignmentState` components were removed.

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
    let s = state.read();
    let entries = s.activity.clone();
    // The store is fully hydrated once `project` is set; before that the
    // empty `activity` slice is "still loading", not "no entries". Render a
    // LoadingSkeleton until hydrate completes.
    let hydrated = s.project.is_some();
    drop(s);
    if !hydrated {
        rsx! {
            Card {
                CardHead { title: "Activity".to_string(), id_text: None }
                CardBody {
                    SkeletonHeading {}
                    SkeletonLine { width_percent: 70 }
                    SkeletonLine { width_percent: 55 }
                }
            }
        }
    } else {
        rsx! { ActivityPanel { entries } }
    }
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

fn derive_trust_pill(trusted: bool) -> (PillTone, &'static str) {
    if trusted {
        (PillTone::Verified, "Trusted")
    } else {
        (PillTone::Stale, "Untrusted")
    }
}

fn derive_auto_replan_pill(state: AutoReplanState) -> (PillTone, &'static str) {
    match state {
        AutoReplanState::Off => (PillTone::Idle, "Off"),
        AutoReplanState::Armed => (PillTone::Verified, "Armed"),
        AutoReplanState::Paused => (PillTone::Failed, "Paused"),
    }
}

fn auto_replan_pause_copy(reason: PauseReason) -> &'static str {
    match reason {
        PauseReason::EmptyBacklog => "Backlog drained. No Ready Issues left to plan.",
        PauseReason::AssignmentBlocked => {
            "Plan Run finished with blocked Issue Assignment(s). Resolve via Re-Enable."
        }
        PauseReason::PushNonFastForward => {
            "Integration Branch push diverged. Retry Push or Abandon Staged on the affected Issue Assignment."
        }
        PauseReason::MergeStagedLeft => {
            "Plan Run finished with merge_staged work. Decide Retry Push or Abandon Staged."
        }
        PauseReason::PlanningFailed => {
            "Planning Phase failed. See latest Plan Run for diagnostics."
        }
        PauseReason::SyncFailed => "Issue Source sync failed. Check GitHub auth or network.",
    }
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
        Card {
            CardHead {
                title: "Activity".to_string(),
                id_text: Some(format!("{}", entries.len())),
            }
            CardBody {
                if entries.is_empty() {
                    EmptyState {
                        title: "No Activity recorded yet.".to_string(),
                        body: "Project Activity entries appear here as the Control Plane records lifecycle transitions.".to_string(),
                        accent: EmptyStateAccent::Cyan,
                    }
                } else {
                    ul { class: "grid gap-2",
                        for entry in entries {
                            li { class: "flex flex-col gap-1 border-b border-stroke pb-2 last:border-0 last:pb-0",
                                div { class: "flex items-baseline justify-between gap-3",
                                    span { class: "font-mono text-[12px] text-cyan", "{entry.kind}" }
                                    span { class: "font-mono text-[11px] text-ink-dim", "{entry.recorded_at}" }
                                }
                                if let Some(detail) = entry.detail.clone() {
                                    p { class: "break-words font-mono text-[12px] text-ink-2", "{detail}" }
                                }
                                if let Some(assignment_id) = entry.assignment_id.clone() {
                                    p { class: "font-mono text-[10px] text-ink-dim", "assignment {assignment_id}" }
                                }
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

    let key = MutationKey::SyncIssueSource(ProjectId(project_id.clone()));
    let sync_label = last_successful_sync_at
        .clone()
        .unwrap_or_else(|| "Never synced".to_string());
    let (status_tone, status_label) = if sync_in_progress {
        (PillTone::Running, "Syncing".to_string())
    } else if last_failure.is_some() {
        (PillTone::Failed, "Failed".to_string())
    } else if last_successful_sync_at.is_some() {
        (PillTone::Verified, "Synced".to_string())
    } else {
        (PillTone::Idle, "Never synced".to_string())
    };

    rsx! {
        Card {
            CardHead {
                title: "Last sync status".to_string(),
                id_text: Some(sync_label),
            }
            CardBody {
                div { class: "flex flex-col gap-4",
                    StatusPill { tone: status_tone, label: status_label }
                    if let Some(failure) = last_failure.clone() {
                        ErrorState {
                            title: "Issue Source sync failed".to_string(),
                            detail: failure,
                            problem_json: None,
                        }
                    }
                    div {
                        ActionButton {
                            mutation_key: key.clone(),
                            variant: ButtonVariant::Primary,
                            testid: "refresh-issue-source-button".to_string(),
                            error_marker: "refresh-issue-source".to_string(),
                            on_press: {
                                let key = key.clone();
                                let project_id = project_id.clone();
                                move |_| {
                                    let key = key.clone();
                                    let project_id = project_id.clone();
                                    wasm_bindgen_futures::spawn_local(async move {
                                        let result = store
                                            .mutate(key, sync_issue_source(project_id.clone()))
                                            .await;
                                        if result.is_ok() {
                                            store.push_success(
                                                "Issue Source synced",
                                                String::new(),
                                            );
                                        }
                                    });
                                }
                            },
                            "Refresh Issue Source"
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
        Card {
            CardHead {
                title: "Issue Source candidates".to_string(),
                id_text: Some(format!("{}", candidates.len())),
            }
            CardBody {
                if candidates.is_empty() {
                    EmptyState {
                        title: "No candidates".to_string(),
                        body: "The Local Control Plane discovered no Issue Source candidates from the Project evidence.".to_string(),
                        accent: EmptyStateAccent::Cyan,
                    }
                } else {
                    ul { class: "grid gap-3",
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
}

#[component]
fn IssueSourceCandidateRow(project_id: String, candidate: IssueSourceCandidate) -> Element {
    let store = use_context::<ProjectStore>();
    let enable_project_id = project_id.clone();
    let enable_kind = candidate.kind.clone();
    let enable_locator = candidate.locator.clone();

    let key = MutationKey::EnableIssueSource(
        ProjectId(project_id.clone()),
        candidate.kind.clone(),
        candidate.locator.clone(),
    );
    let testid = format!(
        "enable-issue-source-{}-{}",
        candidate.kind, candidate.locator
    );
    let error_marker = format!(
        "enable-issue-source-{}-{}",
        candidate.kind, candidate.locator
    );

    rsx! {
        li { class: "flex flex-col gap-2 border-b border-stroke pb-3 last:border-0 last:pb-0 md:flex-row md:items-center md:justify-between",
            p { class: "break-words font-mono text-[13px] text-ink", "{candidate.kind} {candidate.locator}" }
            if candidate.enabled {
                StatusPill { tone: PillTone::Verified, label: "Enabled".to_string() }
            } else {
                ActionButton {
                    mutation_key: key.clone(),
                    variant: ButtonVariant::Primary,
                    testid: testid,
                    error_marker: error_marker,
                    on_press: {
                        let key = key.clone();
                        let project_id = enable_project_id.clone();
                        let kind = enable_kind.clone();
                        let locator = enable_locator.clone();
                        move |_| {
                            let key = key.clone();
                            let project_id = project_id.clone();
                            let kind = kind.clone();
                            let locator = locator.clone();
                            wasm_bindgen_futures::spawn_local(async move {
                                let _ = store
                                    .mutate(
                                        key,
                                        enable_issue_source(project_id, kind, locator),
                                    )
                                    .await;
                            });
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
    let _ = trusted;
    let last_sync = snapshot
        .last_successful_sync_at
        .clone()
        .unwrap_or_else(|| "Never synced".to_string());
    let source_label = format!("{} {}", snapshot.source.kind, snapshot.source.locator);

    rsx! {
        Card {
            CardHead {
                title: "Planning snapshot".to_string(),
                id_text: Some(last_sync),
            }
            CardBody {
                div { class: "flex flex-col gap-4",
                    p { class: "font-mono text-[11px] text-ink-dim", "{source_label}" }
                    if let Some(failure) = snapshot.last_failure.clone() {
                        ErrorState {
                            title: "Issue Source sync failed".to_string(),
                            detail: failure,
                            problem_json: None,
                        }
                    }
                    div { class: "grid gap-4 lg:grid-cols-5",
                        PlanningGroup {
                            project_id: project_id.clone(),
                            title: "Eligible Ready Issues".to_string(),
                            issues: snapshot.eligible,
                            allow_mark_prd: true,
                        }
                        PlanningGroup {
                            project_id: project_id.clone(),
                            title: "Active Issues".to_string(),
                            issues: snapshot.active,
                            allow_mark_prd: true,
                        }
                        PlanningGroup {
                            project_id: project_id.clone(),
                            title: "Blocked Ready Issues".to_string(),
                            issues: snapshot.dependency_blocked,
                            allow_mark_prd: true,
                        }
                        PlanningGroup {
                            project_id: project_id.clone(),
                            title: "Completed Issues".to_string(),
                            issues: snapshot.completed,
                            allow_mark_prd: false,
                        }
                        PlanningGroup {
                            project_id: project_id.clone(),
                            title: "Non-ready Source Issues".to_string(),
                            issues: snapshot.non_ready,
                            allow_mark_prd: true,
                        }
                    }
                    PrdOverridesFooter {
                        project_id,
                        overrides: snapshot.prd_overrides,
                    }
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
    allow_mark_prd: bool,
) -> Element {
    rsx! {
        section { class: "min-w-0 border border-stroke bg-panel/40 p-4",
            h3 { class: "font-display text-[11px] uppercase tracking-[0.18em] text-ink-2", "{title}" }
            if issues.is_empty() {
                p { class: "mt-3 font-mono text-[11px] uppercase tracking-[0.18em] text-ink-dim", "None" }
            } else {
                ul { class: "mt-3 grid gap-3",
                    for issue in issues {
                        PlanningIssue {
                            project_id: project_id.clone(),
                            issue,
                            allow_mark_prd,
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PlanningIssue(project_id: String, issue: SourceIssueSnapshot, allow_mark_prd: bool) -> Element {
    let dependencies = if issue.issue_dependencies.is_empty() {
        "No dependencies".to_string()
    } else {
        issue.issue_dependencies.join(", ")
    };
    let parent = issue
        .parent_issue
        .clone()
        .unwrap_or_else(|| "No parent".to_string());
    let source_id = issue.source_id.clone();

    rsx! {
        li { class: "grid gap-1 border-b border-stroke pb-3 text-[12px] text-ink-2 last:border-0 last:pb-0",
            div { class: "flex items-baseline justify-between gap-3",
                p { class: "min-w-0 break-words font-display text-[13px] text-ink", "{issue.title}" }
                p { class: "shrink-0 font-mono text-[11px] text-ink-dim", "#{issue.source_order}" }
            }
            p { class: "break-words font-mono text-[11px] text-ink-dim", "{issue.source_id}" }
            p { class: "font-mono text-[11px] text-ink-2", "Parent {parent}" }
            p { class: "font-mono text-[11px] text-ink-2", "{dependencies}" }
            if allow_mark_prd {
                MarkPrdButton { project_id, source_id }
            }
        }
    }
}

#[component]
fn MarkPrdButton(project_id: String, source_id: String) -> Element {
    let store = use_context::<ProjectStore>();
    let key = MutationKey::MarkPrd(
        ProjectId(project_id.clone()),
        SourceIssueId(source_id.clone()),
    );
    rsx! {
        ActionButton {
            mutation_key: key.clone(),
            variant: ButtonVariant::Default,
            testid: format!("mark-prd-{source_id}"),
            error_marker: "mark-prd".to_string(),
            on_press: {
                let key = key.clone();
                let project_id = project_id.clone();
                let source_id = source_id.clone();
                move |_| {
                    let key = key.clone();
                    let project_id = project_id.clone();
                    let source_id = source_id.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        let result = store
                            .mutate(key, mark_prd_api(project_id, source_id))
                            .await;
                        if result.is_ok() {
                            store.push_success(
                                "Marked as PRD",
                                "Hidden from Planning. Unmark from the PRDs footer.",
                            );
                        }
                    });
                }
            },
            "Mark as PRD"
        }
    }
}

#[component]
fn PrdOverridesFooter(project_id: String, overrides: Vec<SourceIssueSnapshot>) -> Element {
    let expanded = use_signal(|| false);
    if overrides.is_empty() {
        return rsx! {};
    }
    let count = overrides.len();
    rsx! {
        section { class: "mt-2 border border-stroke bg-panel/20 p-3",
            "data-testid": "prd-overrides-footer",
            button {
                class: "font-display text-[11px] uppercase tracking-[0.18em] text-ink-2 hover:text-cyan",
                "data-testid": "prd-overrides-toggle",
                onclick: {
                    let mut expanded = expanded;
                    move |_| {
                        let next = !*expanded.read();
                        expanded.set(next);
                    }
                },
                if *expanded.read() {
                    "{count} PRDs hidden \u{00B7} Hide"
                } else {
                    "{count} PRDs hidden \u{00B7} Show"
                }
            }
            if *expanded.read() {
                ul { class: "mt-3 grid gap-2",
                    for issue in overrides {
                        PrdOverrideRow {
                            project_id: project_id.clone(),
                            issue,
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PrdOverrideRow(project_id: String, issue: SourceIssueSnapshot) -> Element {
    let source_id = issue.source_id.clone();
    rsx! {
        li { class: "flex items-center justify-between gap-3 border-b border-stroke/40 pb-2 text-[12px] text-ink-2 last:border-0 last:pb-0",
            div { class: "min-w-0",
                p { class: "min-w-0 break-words font-display text-[13px] text-ink", "{issue.title}" }
                p { class: "break-words font-mono text-[11px] text-ink-dim", "{issue.source_id}" }
            }
            UnmarkPrdButton { project_id, source_id }
        }
    }
}

#[component]
fn UnmarkPrdButton(project_id: String, source_id: String) -> Element {
    let store = use_context::<ProjectStore>();
    let key = MutationKey::UnmarkPrd(
        ProjectId(project_id.clone()),
        SourceIssueId(source_id.clone()),
    );
    rsx! {
        ActionButton {
            mutation_key: key.clone(),
            variant: ButtonVariant::Default,
            testid: format!("unmark-prd-{source_id}"),
            error_marker: "unmark-prd".to_string(),
            on_press: {
                let key = key.clone();
                let project_id = project_id.clone();
                let source_id = source_id.clone();
                move |_| {
                    let key = key.clone();
                    let project_id = project_id.clone();
                    let source_id = source_id.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        let _ = store
                            .mutate(key, unmark_prd_api(project_id, source_id))
                            .await;
                    });
                }
            },
            "Unmark"
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

async fn fetch_project_snapshot(project_id: String) -> Result<ProjectSnapshotResponse, String> {
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
    let response =
        gloo_net::http::Request::put(&format!("/api/projects/{project_id}/issue-source"))
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

async fn auto_replan_api(
    project_id: String,
    endpoint: String,
) -> Result<ProjectResponse, project_store::MutationFailure> {
    let response = gloo_net::http::Request::post(&format!(
        "/api/projects/{project_id}/auto-replan/{endpoint}"
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

async fn set_execution_config_api(
    project_id: String,
    request: SetProjectExecutionConfigRequest,
) -> Result<ProjectExecutionConfigResponse, project_store::MutationFailure> {
    let response =
        gloo_net::http::Request::put(&format!("/api/projects/{project_id}/execution-config"))
            .json(&request)
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

async fn mark_prd_api(
    project_id: String,
    source_id: String,
) -> Result<(), project_store::MutationFailure> {
    let response = gloo_net::http::Request::post(&format!(
        "/api/projects/{project_id}/source-issues/{source_id}/prd"
    ))
    .send()
    .await
    .map_err(|error| project_store::MutationFailure::network(error.to_string()))?;
    let status = response.status();
    if !(200..300).contains(&status) {
        let body = response.text().await.unwrap_or_default();
        return Err(project_store::MutationFailure::http(status, body));
    }
    Ok(())
}

async fn unmark_prd_api(
    project_id: String,
    source_id: String,
) -> Result<(), project_store::MutationFailure> {
    let response = gloo_net::http::Request::delete(&format!(
        "/api/projects/{project_id}/source-issues/{source_id}/prd"
    ))
    .send()
    .await
    .map_err(|error| project_store::MutationFailure::network(error.to_string()))?;
    let status = response.status();
    if !(200..300).contains(&status) {
        let body = response.text().await.unwrap_or_default();
        return Err(project_store::MutationFailure::http(status, body));
    }
    Ok(())
}

async fn re_enable_source_issue_api(
    project_id: String,
    source_id: String,
) -> Result<agentic_afk_contracts::ReEnableSourceIssueResponse, project_store::MutationFailure> {
    let response = gloo_net::http::Request::post(&format!(
        "/api/projects/{project_id}/source-issues/{source_id}/re-enable"
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

async fn retry_push_assignment_api(
    project_id: String,
    assignment_id: String,
) -> Result<agentic_afk_contracts::RetryPushResponse, project_store::MutationFailure> {
    let response = gloo_net::http::Request::post(&format!(
        "/api/projects/{project_id}/assignments/{assignment_id}/retry-push"
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

async fn abandon_staged_assignment_api(
    project_id: String,
    assignment_id: String,
    note: Option<String>,
) -> Result<agentic_afk_contracts::AbandonStagedResponse, project_store::MutationFailure> {
    let request_body = agentic_afk_contracts::AbandonStagedRequest { note };
    let body = serde_json::to_string(&request_body)
        .map_err(|error| project_store::MutationFailure::network(error.to_string()))?;
    let response = gloo_net::http::Request::post(&format!(
        "/api/projects/{project_id}/assignments/{assignment_id}/abandon-staged"
    ))
    .header("content-type", "application/json")
    .body(body)
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

async fn start_plan_run_api(
    project_id: String,
) -> Result<PlanRunResponse, project_store::MutationFailure> {
    let response = gloo_net::http::Request::post(&format!("/api/projects/{project_id}/plan-runs"))
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
    let response =
        gloo_net::http::Request::post(&format!("/api/projects/{project_id}/issue-source/sync"))
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
        let err_key = MutationKey::StartPlanRun(ProjectId("demo".into()));
        store.force_state(pending_key, MutationState::Pending);
        store.force_state(
            err_key,
            MutationState::Error {
                category: MutationCategory::Validation,
                title: "project trust required".into(),
                detail: "trust the project from settings before starting a Plan Run".into(),
            },
        );
    });

    let idle_key = MutationKey::TrustProject(ProjectId("demo".into()));
    let pending_key = MutationKey::SyncIssueSource(ProjectId("demo".into()));
    let err_key = MutationKey::StartPlanRun(ProjectId("demo".into()));

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
                                "Trust Project"
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
                                "Start Plan Run"
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
                            title: "No Plan Runs".to_string(),
                            body: "Start a Plan Run to plan and run a parallel batch of issue work.".to_string(),
                            accent: EmptyStateAccent::Cyan,
                            ActionButton {
                                mutation_key: MutationKey::TrustProject(ProjectId("demo-empty".into())),
                                variant: ButtonVariant::Primary,
                                on_press: move |_| {},
                                "Start a Plan Run"
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
    use agentic_afk_contracts::PlanRunStage;

    #[test]
    fn plan_run_stage_planning_maps_to_pending_pill() {
        let (tone, label) = derive_plan_run_stage_pill(Some(PlanRunStage::Planning));
        assert_eq!(tone, PillTone::Pending);
        assert_eq!(label, "Planning");
    }

    #[test]
    fn plan_run_stage_implementing_maps_to_running_pill() {
        let (tone, label) = derive_plan_run_stage_pill(Some(PlanRunStage::Implementing));
        assert_eq!(tone, PillTone::Running);
        assert_eq!(label, "Implementing");
    }

    #[test]
    fn plan_run_stage_reviewing_maps_to_running_pill() {
        let (tone, label) = derive_plan_run_stage_pill(Some(PlanRunStage::Reviewing));
        assert_eq!(tone, PillTone::Running);
        assert_eq!(label, "Reviewing");
    }

    #[test]
    fn plan_run_stage_merging_maps_to_running_pill() {
        let (tone, label) = derive_plan_run_stage_pill(Some(PlanRunStage::Merging));
        assert_eq!(tone, PillTone::Running);
        assert_eq!(label, "Merging");
    }

    #[test]
    fn plan_run_stage_pushing_maps_to_running_pill() {
        let (tone, label) = derive_plan_run_stage_pill(Some(PlanRunStage::Pushing));
        assert_eq!(tone, PillTone::Running);
        assert_eq!(label, "Pushing");
    }

    #[test]
    fn plan_run_stage_none_falls_back_to_running() {
        let (tone, label) = derive_plan_run_stage_pill(None);
        assert_eq!(tone, PillTone::Running);
        assert_eq!(label, "Running");
    }

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
            default_branch: None,
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
            default_branch: None,
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
            default_branch: None,
        };
        let (_tone, label) = derive_git_pill(Some(&summary));
        assert_eq!(label, "clean");
    }

    #[test]
    fn short_project_id_truncates_to_eight_chars() {
        assert_eq!(short_project_id("0123456789abcdef"), "01234567");
        assert_eq!(short_project_id("abc"), "abc");
    }

    #[test]
    fn trust_pill_trusted_is_verified() {
        let (tone, label) = derive_trust_pill(true);
        assert_eq!(tone, PillTone::Verified);
        assert_eq!(label, "Trusted");
    }

    #[test]
    fn trust_pill_untrusted_is_stale() {
        let (tone, label) = derive_trust_pill(false);
        assert_eq!(tone, PillTone::Stale);
        assert_eq!(label, "Untrusted");
    }
}
