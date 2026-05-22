use dioxus::prelude::*;

#[component]
pub fn ErrorState(
    title: String,
    detail: String,
    problem_json: Option<String>,
    children: Element,
) -> Element {
    rsx! {
        div {
            class: "relative border border-coral/50 bg-[linear-gradient(180deg,rgba(255,110,110,0.10),rgba(255,110,110,0.02))] px-6 py-[22px] shadow-[0_0_40px_-10px_rgba(255,110,110,0.35)]",
            span {
                class: "absolute -top-[10px] left-[18px] bg-void px-2 font-display text-[10px] uppercase tracking-[0.32em] text-coral",
                "Error"
            }
            h3 { class: "mb-[6px] font-display text-[16px] uppercase tracking-[0.18em] text-coral", "{title}" }
            p { class: "mb-4 max-w-[56ch] text-ink-2", "{detail}" }
            if let Some(json) = problem_json {
                pre {
                    class: "mb-4 whitespace-pre-wrap border-l-2 border-coral pl-3 font-mono text-[11px] text-ink-2",
                    "{json}"
                }
            }
            {children}
        }
    }
}
