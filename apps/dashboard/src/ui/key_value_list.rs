use dioxus::prelude::*;

/// HUD-styled label/value list. Compose with `KeyValueRow` children so callers
/// retain control over per-row formatting (mono vs body, multi-value rows).
#[component]
pub fn KeyValueList(children: Element) -> Element {
    rsx! {
        dl {
            class: "grid gap-x-6 gap-y-3 font-mono sm:grid-cols-[max-content_1fr]",
            {children}
        }
    }
}

/// One row of a `KeyValueList`. `mono` defaults to `true` so identifiers,
/// paths, and timestamps line up; pass `mono: false` for prose values.
#[component]
pub fn KeyValueRow(
    label: String,
    value: String,
    #[props(default = true)] mono: bool,
) -> Element {
    let value_class = if mono {
        "min-w-0 break-words font-mono text-[13px] text-ink"
    } else {
        "min-w-0 break-words font-body text-[13px] text-ink"
    };
    rsx! {
        dt { class: "font-display text-[10px] uppercase tracking-[0.18em] text-ink-dim", "{label}" }
        dd { class: value_class, "{value}" }
    }
}
