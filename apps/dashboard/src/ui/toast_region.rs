use dioxus::prelude::*;

use crate::project_store::{ProjectStore, Toast, ToastKind};

/// HUD-styled replacement for the prototype `ToastRegion` in `main.rs`.
/// Reads toasts from `ProjectStore` via context.
#[component]
pub fn HudToastRegion() -> Element {
    let store = use_context::<ProjectStore>();
    let toasts = store.toasts();
    rsx! {
        div {
            class: "flex max-w-[420px] flex-col gap-3",
            role: "status",
            "aria-live": "polite",
            for toast in toasts.read().clone().into_iter() {
                HudToast { store, toast }
            }
        }
    }
}

#[component]
fn HudToast(store: ProjectStore, toast: Toast) -> Element {
    let tone = match toast.kind {
        ToastKind::Success => "text-mint",
        ToastKind::Error => "text-coral",
    };
    let kind_label = match toast.kind {
        ToastKind::Success => "success",
        ToastKind::Error => "error",
    };
    let id = toast.id;
    rsx! {
        div {
            class: "hud-glass hud-notch-tile relative border border-stroke bg-panel px-4 pb-[14px] pl-[18px] pt-3 {tone}",
            "data-toast-kind": kind_label,
            span {
                class: "absolute bottom-[10px] left-0 top-[10px] w-[2px] bg-current shadow-[0_0_12px_currentColor]",
            }
            button {
                class: "absolute right-3 top-[10px] cursor-pointer text-ink-dim hover:text-ink",
                onclick: move |_| store.dismiss_toast(id),
                "×"
            }
            div { class: "font-display text-[11px] uppercase tracking-[0.18em]", "{toast.title}" }
            if !toast.detail.is_empty() {
                div { class: "mt-[2px] text-[13px] text-ink", "{toast.detail}" }
            }
        }
    }
}
