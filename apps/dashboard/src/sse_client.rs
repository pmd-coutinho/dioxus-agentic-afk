//! Browser-side SSE client wrapping `web_sys::EventSource`. See ADR-0032.
//!
//! Used by `ProjectLayout` to subscribe to `/api/projects/:id/events` and
//! drive `ProjectStore::apply_event`. Browser handles auto-reconnect and
//! `Last-Event-ID` natively; we only pass the initial snapshot sequence via
//! a `?last_event_id=N` query parameter because the browser does not send
//! `Last-Event-ID` on the very first connection.

use agentic_afk_contracts::ProjectEvent;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{EventSource, MessageEvent};

/// Handle that keeps the SSE subscription alive. Dropping it closes the
/// `EventSource` and detaches the message listener.
pub struct SseSubscription {
    source: EventSource,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
}

impl Drop for SseSubscription {
    fn drop(&mut self) {
        self.source.set_onmessage(None);
        self.source.close();
    }
}

/// Open an SSE subscription to `/api/projects/{project_id}/events`. The
/// `last_event_id` is passed via query string so the server replays missed
/// events from the per-Project ring buffer (or sends `Resync` if the buffer
/// has rolled over). Each incoming `ProjectEvent` is handed to `on_event`
/// together with its monotonic sequence.
pub fn subscribe<F>(project_id: &str, last_event_id: u64, mut on_event: F) -> SseSubscription
where
    F: FnMut(u64, ProjectEvent) + 'static,
{
    let url = format!("/api/projects/{project_id}/events?last_event_id={last_event_id}");
    let source = EventSource::new(&url).expect("EventSource construction should not fail");

    let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let Some(data) = event.data().as_string() else {
            return;
        };
        let Ok(parsed) = serde_json::from_str::<ProjectEvent>(&data) else {
            return;
        };
        let sequence = event
            .last_event_id()
            .parse::<u64>()
            .unwrap_or(0);
        on_event(sequence, parsed);
    });
    source.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

    SseSubscription {
        source,
        _on_message: on_message,
    }
}
