use dioxus::prelude::*;

#[component]
pub fn SkeletonHeading() -> Element {
    rsx! {
        div { class: "hud-scanline mb-3 h-[22px] w-[60%] border border-stroke-2 bg-[linear-gradient(180deg,rgba(91,233,255,0.06),rgba(91,233,255,0.02))]" }
    }
}

#[component]
pub fn SkeletonLine(#[props(default = 80)] width_percent: u16) -> Element {
    let style = format!("width: {width_percent}%;");
    rsx! {
        div {
            class: "hud-scanline my-2 h-3 border border-stroke-2 bg-[linear-gradient(180deg,rgba(91,233,255,0.06),rgba(91,233,255,0.02))]",
            style: "{style}",
        }
    }
}
