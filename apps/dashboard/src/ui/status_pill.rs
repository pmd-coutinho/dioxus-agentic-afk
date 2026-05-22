use dioxus::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PillTone {
    Idle,
    Pending,
    Running,
    Verified,
    Stale,
    Failed,
}

impl PillTone {
    fn color_class(self) -> &'static str {
        match self {
            PillTone::Idle => "text-ink-dim",
            PillTone::Pending => "text-amber",
            PillTone::Running => "text-cyan",
            PillTone::Verified => "text-mint",
            PillTone::Stale => "text-amber",
            PillTone::Failed => "text-coral",
        }
    }

    fn dot_extras(self) -> &'static str {
        match self {
            PillTone::Idle => "",
            PillTone::Running => "hud-pulse-dot",
            _ => "",
        }
    }
}

#[component]
pub fn StatusPill(tone: PillTone, label: String) -> Element {
    let color = tone.color_class();
    let extra = tone.dot_extras();
    let dot_shadow = if matches!(tone, PillTone::Idle) {
        "shadow-none"
    } else {
        "shadow-[0_0_8px_currentColor]"
    };
    rsx! {
        span {
            class: "inline-flex items-center gap-2 border border-current bg-black/25 px-2 py-1 pl-2 font-display text-[11px] uppercase tracking-[0.18em] {color} {extra}",
            span { class: "h-2 w-2 rounded-full bg-current {dot_shadow}" }
            "{label}"
        }
    }
}
