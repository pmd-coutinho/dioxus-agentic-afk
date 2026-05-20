use agentic_afk_contracts::{AppInfoResponse, GitSummary, ProjectResponse};
use dioxus::prelude::*;

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    let app_info = use_resource(fetch_app_info);
    let projects = use_resource(fetch_projects);

    rsx! {
        main { class: "min-h-screen bg-zinc-950 text-zinc-100",
            section { class: "mx-auto flex w-full max-w-5xl flex-col gap-6 px-6 py-8",
                header { class: "flex flex-col gap-1 border-b border-zinc-800 pb-5",
                    p { class: "text-sm font-medium uppercase tracking-wide text-emerald-300", "Local Control Plane" }
                    h1 { class: "text-3xl font-semibold", "agentic-afk" }
                }

                match &*app_info.read_unchecked() {
                    Some(Ok(info)) => rsx! {
                        Dashboard {
                            info: info.clone(),
                            projects: projects.read_unchecked().clone(),
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
    }
}

#[component]
fn Dashboard(
    info: AppInfoResponse,
    projects: Option<Result<Vec<ProjectResponse>, String>>,
) -> Element {
    rsx! {
        div { class: "flex flex-col gap-4",
            div { class: "grid gap-4 md:grid-cols-[1.2fr_0.8fr]",
                StatusPanel {
                    title: "API connected".to_string(),
                    detail: format!("{} {}", info.app_name, info.version),
                    tone: "border-emerald-700 bg-emerald-950/35 text-emerald-50".to_string(),
                }
                section { class: "rounded-lg border border-zinc-800 bg-zinc-900 p-5",
                    h2 { class: "mb-4 text-base font-semibold", "Settings" }
                    dl { class: "grid gap-3 text-sm",
                        SettingRow { label: "Bind address".to_string(), value: info.config.bind_address }
                        SettingRow { label: "Dashboard assets".to_string(), value: info.config.dashboard_asset_dir }
                        SettingRow { label: "Database".to_string(), value: info.config.database_url }
                    }
                }
            }

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
                p { class: "break-words font-mono text-sm text-zinc-100", "{project.path}" }
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
