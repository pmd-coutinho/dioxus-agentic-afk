use dioxus::prelude::*;

/// Frosted glass panel with bracketed corner glyphs. Children may compose
/// `CardHead`, `CardBody`, and `CardFoot` for structured layout.
#[component]
pub fn Card(children: Element) -> Element {
    rsx! {
        article {
            class: "hud-bracket-corners hud-glass relative border border-stroke bg-panel shadow-[inset_0_0_0_1px_var(--color-stroke-2),0_30px_60px_-30px_rgba(0,0,0,0.6)]",
            {children}
        }
    }
}

#[component]
pub fn CardHead(title: String, id_text: Option<String>) -> Element {
    rsx! {
        header {
            class: "flex items-center justify-between border-b border-stroke px-[18px] py-3 font-display text-[11px] uppercase tracking-[0.18em] text-ink-2",
            h2 { class: "font-display text-[11px] font-medium uppercase tracking-[0.18em] text-ink-2",
                "{title}"
            }
            if let Some(id) = id_text {
                span { class: "font-mono tracking-[0.06em] text-cyan", "{id}" }
            }
        }
    }
}

#[component]
pub fn CardBody(children: Element) -> Element {
    rsx! { div { class: "p-[18px]", {children} } }
}

#[component]
pub fn CardFoot(children: Element) -> Element {
    rsx! {
        footer {
            class: "flex items-center gap-3 border-t border-stroke px-[18px] py-[14px]",
            {children}
        }
    }
}
