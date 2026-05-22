//! `ActionButton` — only stateful primitive. Binds to `ProjectStore` via
//! `MutationKey` so call sites do not repeat pending / error wiring.
//!
//! The visual variants (Default / Primary / Destructive) and the dynamic
//! render-state (Idle / Pending / IdleWithError) are decoupled: pure
//! `derive_button_render_state` is unit-tested without Dioxus or WASM, and
//! the component just translates the result into Tailwind classes.

use dioxus::prelude::*;

use crate::project_store::{MutationKey, MutationState, ProjectStore};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ButtonVariant {
    Default,
    Primary,
    Destructive,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ButtonRenderState {
    Idle,
    Pending,
    IdleWithError { title: String, detail: String },
}

/// Pure mapping from store state for a `MutationKey` to the button's
/// render-state. No Dioxus, no Signal, no WASM — exercised by unit tests in
/// this module.
pub fn derive_button_render_state(state: Option<MutationState>) -> ButtonRenderState {
    match state {
        Some(MutationState::Pending) => ButtonRenderState::Pending,
        Some(MutationState::Error { title, detail, .. }) => {
            ButtonRenderState::IdleWithError { title, detail }
        }
        _ => ButtonRenderState::Idle,
    }
}

fn variant_classes(variant: ButtonVariant) -> &'static str {
    match variant {
        ButtonVariant::Default => "text-ink bg-cyan/5 border-stroke hover:text-void hover:bg-cyan hover:shadow-[0_0_28px_rgba(91,233,255,0.45)]",
        ButtonVariant::Primary => "text-cyan border-cyan/50 bg-[linear-gradient(180deg,rgba(91,233,255,0.18),rgba(91,233,255,0.04))] hover:text-void hover:bg-cyan",
        ButtonVariant::Destructive => "text-coral border-coral/50 bg-[linear-gradient(180deg,rgba(255,110,110,0.18),rgba(255,110,110,0.04))] hover:text-void hover:bg-coral hover:shadow-[0_0_28px_rgba(255,110,110,0.45)]",
    }
}

const BASE_BTN: &str = "hud-notch-btn font-display text-[12px] font-medium uppercase tracking-[0.18em] border px-[18px] py-[9px] cursor-pointer transition-[background,color,box-shadow] duration-150 ease-out";
const PENDING_BTN: &str = "hud-notch-btn hud-pending-sweep font-display text-[12px] font-medium uppercase tracking-[0.18em] border px-[18px] py-[9px] text-amber border-amber/50 bg-[linear-gradient(180deg,rgba(255,200,87,0.18),rgba(255,200,87,0.04))] pointer-events-none";
const DISABLED_BTN: &str = "hud-notch-btn font-display text-[12px] font-medium uppercase tracking-[0.18em] border px-[18px] py-[9px] text-ink-dim border-stroke-2 bg-transparent pointer-events-none";

#[component]
pub fn ActionButton(
    mutation_key: MutationKey,
    #[props(default = ButtonVariant::Default)] variant: ButtonVariant,
    #[props(default)] disabled: bool,
    #[props(default)] testid: Option<String>,
    #[props(default)] error_marker: Option<String>,
    on_press: EventHandler<()>,
    children: Element,
) -> Element {
    let store = use_context::<ProjectStore>();
    let render_state = derive_button_render_state(store.state(&mutation_key));
    let pending_attr = matches!(render_state, ButtonRenderState::Pending);

    match render_state {
        ButtonRenderState::Pending => rsx! {
            button {
                class: PENDING_BTN,
                r#type: "button",
                disabled: true,
                "data-testid": testid.clone(),
                "data-mutation-pending": "true",
                {children}
            }
        },
        ButtonRenderState::Idle if disabled => rsx! {
            button {
                class: DISABLED_BTN,
                r#type: "button",
                disabled: true,
                "data-testid": testid.clone(),
                "data-mutation-pending": "false",
                {children}
            }
        },
        ButtonRenderState::Idle => rsx! {
            button {
                class: format!("{BASE_BTN} {}", variant_classes(variant)),
                r#type: "button",
                "data-testid": testid.clone(),
                "data-mutation-pending": if pending_attr { "true" } else { "false" },
                onclick: move |_| on_press.call(()),
                {children}
            }
        },
        ButtonRenderState::IdleWithError { title, detail } => rsx! {
            div { class: "flex flex-col items-start gap-3",
                button {
                    class: format!("{BASE_BTN} {}", variant_classes(variant)),
                    r#type: "button",
                    "data-testid": testid.clone(),
                    "data-mutation-pending": "false",
                    onclick: move |_| on_press.call(()),
                    {children}
                }
                div {
                    class: "max-w-[360px] border border-coral/35 border-l-[3px] bg-coral/5 px-3 py-2 font-mono text-[11px] text-coral",
                    "data-error-marker": error_marker.clone(),
                    span { class: "block uppercase tracking-[0.14em]", "{title}" }
                    span { class: "mt-[2px] block text-ink-2", "{detail}" }
                }
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_store_entry_renders_idle() {
        assert_eq!(
            derive_button_render_state(None),
            ButtonRenderState::Idle
        );
    }

    #[test]
    fn pending_state_renders_pending() {
        assert_eq!(
            derive_button_render_state(Some(MutationState::Pending)),
            ButtonRenderState::Pending
        );
    }

    #[test]
    fn error_state_renders_idle_with_problem_fields_adjacent() {
        let state = MutationState::Error {
            category: crate::project_store::MutationCategory::Validation,
            title: "project trust required".into(),
            detail: "trust the project from settings before booting an agent".into(),
        };
        assert_eq!(
            derive_button_render_state(Some(state)),
            ButtonRenderState::IdleWithError {
                title: "project trust required".into(),
                detail: "trust the project from settings before booting an agent".into(),
            }
        );
    }

    #[test]
    fn done_state_renders_idle() {
        assert_eq!(
            derive_button_render_state(Some(MutationState::Done)),
            ButtonRenderState::Idle
        );
    }
}
