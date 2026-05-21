//! Per-Project event bus for live Dashboard deltas (ADR-0032).
//!
//! Publishes typed `ProjectEvent` values with a monotonic per-Project
//! `sequence`. Keeps a bounded ring buffer per Project so reconnecting
//! subscribers can replay events newer than their `Last-Event-ID`. When the
//! caller's last seen sequence has fallen out of the ring, the first event
//! the subscriber receives is `ProjectEvent::Resync` — the client must then
//! re-hydrate from `GET /api/projects/{id}/snapshot`.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;

use agentic_afk_contracts::{ProjectEvent, ProjectId};
use futures_util::Stream;
use futures_util::stream::StreamExt;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

/// Default ring-buffer capacity per Project (~200 events as per #27).
pub const DEFAULT_RING_CAPACITY: usize = 200;

/// Broadcast channel capacity. Slow consumers that fall behind by more than
/// this many events receive `Resync` and continue from the live tail.
const BROADCAST_CAPACITY: usize = 256;

/// One delta plus its monotonic per-Project sequence.
#[derive(Clone, Debug)]
pub struct SequencedEvent {
    pub sequence: u64,
    pub event: ProjectEvent,
}

#[derive(Debug)]
struct ProjectChannel {
    next_sequence: u64,
    ring: VecDeque<SequencedEvent>,
    ring_capacity: usize,
    sender: broadcast::Sender<SequencedEvent>,
}

impl ProjectChannel {
    fn new(ring_capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            next_sequence: 1,
            ring: VecDeque::with_capacity(ring_capacity),
            ring_capacity,
            sender,
        }
    }
}

/// Per-Project fan-out hub. Cheap to clone (`Arc` inside).
#[derive(Clone, Debug)]
pub struct EventBus {
    inner: Arc<Mutex<HashMap<ProjectId, ProjectChannel>>>,
    ring_capacity: usize,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::with_ring_capacity(DEFAULT_RING_CAPACITY)
    }
}

impl EventBus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_ring_capacity(ring_capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            ring_capacity,
        }
    }

    /// Publish one delta for `project_id`. Returns the assigned sequence.
    /// Sequence is monotonic per Project and starts at 1.
    pub fn publish(&self, project_id: &ProjectId, event: ProjectEvent) -> u64 {
        let mut guard = self.inner.lock().expect("event bus mutex poisoned");
        let channel = guard
            .entry(project_id.clone())
            .or_insert_with(|| ProjectChannel::new(self.ring_capacity));
        let sequence = channel.next_sequence;
        channel.next_sequence += 1;
        let entry = SequencedEvent { sequence, event };
        if channel.ring.len() == channel.ring_capacity {
            channel.ring.pop_front();
        }
        channel.ring.push_back(entry.clone());
        // It is OK if there are no current subscribers.
        let _ = channel.sender.send(entry);
        sequence
    }

    /// Latest published sequence for `project_id` (0 if never published).
    pub fn latest_sequence(&self, project_id: &ProjectId) -> u64 {
        let guard = self.inner.lock().expect("event bus mutex poisoned");
        guard
            .get(project_id)
            .map(|channel| channel.next_sequence.saturating_sub(1))
            .unwrap_or(0)
    }

    /// Subscribe to deltas for `project_id`. If `last_seen_seq` is `Some(n)`
    /// the stream first replays buffered events with `sequence > n`. If `n`
    /// predates the oldest buffered event (or the project has no history
    /// reaching back that far), the first emitted event is `Resync` with
    /// sequence `n + 1`, signalling the client to re-fetch `/snapshot`.
    pub fn subscribe(
        &self,
        project_id: &ProjectId,
        last_seen_seq: Option<u64>,
    ) -> impl Stream<Item = SequencedEvent> + Send + 'static {
        let (replay, receiver) = {
            let mut guard = self.inner.lock().expect("event bus mutex poisoned");
            let channel = guard
                .entry(project_id.clone())
                .or_insert_with(|| ProjectChannel::new(self.ring_capacity));
            let receiver = channel.sender.subscribe();
            let replay = compute_replay(channel, last_seen_seq);
            (replay, receiver)
        };

        let live = BroadcastStream::new(receiver)
            .filter_map(move |item| async move { item.ok() });

        futures_util::stream::iter(replay).chain(live)
    }
}

fn compute_replay(
    channel: &ProjectChannel,
    last_seen_seq: Option<u64>,
) -> Vec<SequencedEvent> {
    let Some(last_seen) = last_seen_seq else {
        return Vec::new();
    };
    let oldest_in_ring = channel.ring.front().map(|entry| entry.sequence);
    let latest = channel.next_sequence.saturating_sub(1);
    if last_seen >= latest {
        // Caller is already caught up; nothing to replay.
        return Vec::new();
    }
    match oldest_in_ring {
        // Ring still holds everything strictly greater than `last_seen`.
        Some(oldest) if oldest <= last_seen + 1 => channel
            .ring
            .iter()
            .filter(|entry| entry.sequence > last_seen)
            .cloned()
            .collect(),
        // Either the ring's oldest event is newer than `last_seen + 1`
        // (events were evicted) or the ring is empty but `last_seen > 0`
        // (server restart). Either way the caller has missed work and must
        // resync from REST.
        _ => vec![SequencedEvent {
            sequence: last_seen + 1,
            event: ProjectEvent::Resync,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_afk_contracts::ProjectActivityEntryResponse;
    use futures_util::StreamExt;

    fn pid(id: &str) -> ProjectId {
        ProjectId(id.to_string())
    }

    fn activity(kind: &str) -> ProjectEvent {
        ProjectEvent::Activity(ProjectActivityEntryResponse {
            id: format!("act-{kind}"),
            project_id: "p".to_string(),
            assignment_id: None,
            kind: kind.to_string(),
            detail: None,
            recorded_at: "0".to_string(),
        })
    }

    #[test]
    fn publish_returns_monotonic_sequence_starting_at_one() {
        let bus = EventBus::new();
        let project = pid("p1");
        assert_eq!(bus.publish(&project, activity("a")), 1);
        assert_eq!(bus.publish(&project, activity("b")), 2);
        assert_eq!(bus.publish(&project, activity("c")), 3);
    }

    #[test]
    fn sequences_are_isolated_per_project() {
        let bus = EventBus::new();
        let a = pid("a");
        let b = pid("b");
        assert_eq!(bus.publish(&a, activity("x")), 1);
        assert_eq!(bus.publish(&b, activity("y")), 1);
        assert_eq!(bus.publish(&a, activity("z")), 2);
        assert_eq!(bus.publish(&b, activity("w")), 2);
    }

    #[test]
    fn latest_sequence_reflects_published_count() {
        let bus = EventBus::new();
        let p = pid("p");
        assert_eq!(bus.latest_sequence(&p), 0);
        bus.publish(&p, activity("a"));
        bus.publish(&p, activity("b"));
        assert_eq!(bus.latest_sequence(&p), 2);
    }

    #[test]
    fn ring_buffer_evicts_oldest_when_capacity_exceeded() {
        let bus = EventBus::with_ring_capacity(3);
        let p = pid("p");
        for kind in ["a", "b", "c", "d", "e"] {
            bus.publish(&p, activity(kind));
        }
        let guard = bus.inner.lock().unwrap();
        let channel = guard.get(&p).unwrap();
        let kept: Vec<u64> = channel.ring.iter().map(|entry| entry.sequence).collect();
        // Five published, three kept: sequences 3, 4, 5.
        assert_eq!(kept, vec![3, 4, 5]);
    }

    #[tokio::test]
    async fn subscriber_without_last_seen_only_receives_live_events() {
        let bus = EventBus::new();
        let p = pid("p");
        bus.publish(&p, activity("before"));
        let mut stream = Box::pin(bus.subscribe(&p, None));
        bus.publish(&p, activity("after"));
        let next = stream.next().await.unwrap();
        assert_eq!(next.sequence, 2);
        match next.event {
            ProjectEvent::Activity(entry) => assert_eq!(entry.kind, "after"),
            _ => panic!("expected Activity event"),
        }
    }

    #[tokio::test]
    async fn subscriber_with_last_seen_replays_missed_events_from_ring() {
        let bus = EventBus::new();
        let p = pid("p");
        bus.publish(&p, activity("one"));
        bus.publish(&p, activity("two"));
        bus.publish(&p, activity("three"));
        let mut stream = Box::pin(bus.subscribe(&p, Some(1)));
        let replay_two = stream.next().await.unwrap();
        let replay_three = stream.next().await.unwrap();
        assert_eq!(replay_two.sequence, 2);
        assert_eq!(replay_three.sequence, 3);
    }

    #[tokio::test]
    async fn subscriber_with_last_seen_older_than_ring_receives_resync_first() {
        let bus = EventBus::with_ring_capacity(2);
        let p = pid("p");
        for kind in ["a", "b", "c", "d"] {
            bus.publish(&p, activity(kind));
        }
        // Ring now holds sequences 3, 4. Caller last saw sequence 1, so
        // sequence 2 was evicted; caller cannot be caught up by replay.
        let mut stream = Box::pin(bus.subscribe(&p, Some(1)));
        let first = stream.next().await.unwrap();
        assert!(matches!(first.event, ProjectEvent::Resync));
    }

    #[tokio::test]
    async fn caught_up_subscriber_does_not_replay() {
        let bus = EventBus::new();
        let p = pid("p");
        bus.publish(&p, activity("a"));
        bus.publish(&p, activity("b"));
        let mut stream = Box::pin(bus.subscribe(&p, Some(2)));
        bus.publish(&p, activity("c"));
        let next = stream.next().await.unwrap();
        assert_eq!(next.sequence, 3);
    }

    #[tokio::test]
    async fn cross_project_subscriber_does_not_see_other_project_events() {
        let bus = EventBus::new();
        let a = pid("a");
        let b = pid("b");
        let mut stream_a = Box::pin(bus.subscribe(&a, None));
        bus.publish(&b, activity("b-only"));
        bus.publish(&a, activity("a-only"));
        let next = stream_a.next().await.unwrap();
        match next.event {
            ProjectEvent::Activity(entry) => assert_eq!(entry.kind, "a-only"),
            _ => panic!("expected a-only"),
        }
    }
}
