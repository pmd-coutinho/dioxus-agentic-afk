use dioxus::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EmptyStateAccent {
    Cyan,
    Magenta,
}

#[component]
pub fn EmptyState(
    title: String,
    body: String,
    #[props(default = EmptyStateAccent::Cyan)] accent: EmptyStateAccent,
    children: Element,
) -> Element {
    let glyph = match accent {
        EmptyStateAccent::Cyan => "border-cyan shadow-[0_0_24px_rgba(91,233,255,0.35),inset_0_0_16px_rgba(91,233,255,0.18)]",
        EmptyStateAccent::Magenta => "border-magenta shadow-[0_0_24px_rgba(199,146,255,0.35),inset_0_0_16px_rgba(199,146,255,0.18)]",
    };
    rsx! {
        div {
            class: "relative overflow-hidden border border-dashed border-stroke bg-cyan/5 px-6 py-9 text-center",
            div {
                class: "pointer-events-none absolute inset-0",
                style: "background: radial-gradient(circle at 50% 30%, rgba(91,233,255,0.12), transparent 60%);",
            }
            div { class: "relative mx-auto mb-[18px] h-16 w-16 rotate-45 border {glyph}",
                div { class: "absolute inset-3 border border-cyan/55" }
            }
            p { class: "mb-[6px] font-display text-[14px] uppercase tracking-[0.22em] text-ink", "{title}" }
            p { class: "mx-auto mb-[18px] max-w-[36ch] text-[13px] text-ink-2", "{body}" }
            {children}
        }
    }
}
