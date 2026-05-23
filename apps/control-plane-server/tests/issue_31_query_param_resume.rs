//! Tests for issue #31: SSE endpoint accepts `?last_event_id=N` as a fallback
//! when the `Last-Event-ID` header is absent. The browser's native
//! `EventSource` only sends `Last-Event-ID` on auto-reconnect, so the
//! Dashboard passes the snapshot sequence via this query parameter on the
//! initial subscribe.

use agentic_afk_contracts::{CreateProjectRequest, ProjectEvent};
use agentic_afk_control_plane_server::{
    ControlPlaneConfig, control_plane_events, event_bus::EventBus, router_with_bus,
};
use agentic_afk_persistence as persistence;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn temp_path(name: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let salt = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "agentic-afk-{name}-{}-{}",
        std::process::id(),
        nanos.wrapping_add(u128::from(salt))
    ))
}

#[tokio::test]
async fn events_endpoint_honors_last_event_id_query_param_when_header_absent() {
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let project_path = temp_path("issue31-query-resume");
    std::fs::create_dir_all(project_path.join(".git")).unwrap();
    let project = persistence::create_project(
        &db,
        &CreateProjectRequest {
            path: project_path.display().to_string(),
        },
    )
    .await
    .unwrap();

    let bus = EventBus::new();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "target/dx/agentic-afk-dashboard/release/web/public".into(),
        database_url: "sqlite::memory:".into(),
        gh_binary_path: "gh".into(),
        worktrunk_binary_path: "wt".into(),
        codex_binary_path: "codex".into(),
        docker_binary_path: "docker".into(),
        codex_auth_path: "/dev/null".into(),
    };
    let app = router_with_bus(config, db.clone(), bus.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Publish two activities before subscribing so they enter the ring.
    control_plane_events::record_activity(&db, &bus, &project.id.0, None, "first", None)
        .await
        .unwrap();
    control_plane_events::record_activity(&db, &bus, &project.id.0, None, "second", None)
        .await
        .unwrap();

    // Connect without Last-Event-ID header, with ?last_event_id=1 query param.
    let request = format!(
        "GET /api/projects/{}/events?last_event_id=1 HTTP/1.1\r\nHost: {addr}\r\nAccept: text/event-stream\r\n\r\n",
        project.id.0,
    );
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();

    let body = read_until_frame(&mut stream, std::time::Duration::from_secs(2)).await;
    let frames = parse_sse_events(&body);

    assert!(!frames.is_empty(), "expected replay frames, body was: {body}");
    assert_eq!(frames[0].id, "2", "expected replay to start at seq=2");
    let event: ProjectEvent = serde_json::from_str(&frames[0].data).unwrap();
    match event {
        ProjectEvent::Activity(entry) => assert_eq!(entry.kind, "second"),
        other => panic!("expected Activity, got {other:?}"),
    }
}

#[tokio::test]
async fn last_event_id_header_takes_precedence_over_query_param() {
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let project_path = temp_path("issue31-header-wins");
    std::fs::create_dir_all(project_path.join(".git")).unwrap();
    let project = persistence::create_project(
        &db,
        &CreateProjectRequest {
            path: project_path.display().to_string(),
        },
    )
    .await
    .unwrap();

    let bus = EventBus::new();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "target/dx/agentic-afk-dashboard/release/web/public".into(),
        database_url: "sqlite::memory:".into(),
        gh_binary_path: "gh".into(),
        worktrunk_binary_path: "wt".into(),
        codex_binary_path: "codex".into(),
        docker_binary_path: "docker".into(),
        codex_auth_path: "/dev/null".into(),
    };
    let app = router_with_bus(config, db.clone(), bus.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    for kind in ["a", "b", "c"] {
        control_plane_events::record_activity(&db, &bus, &project.id.0, None, kind, None)
            .await
            .unwrap();
    }

    // Header says resume from 2 (=> replay 3 only); query param falsely says 0
    // (would replay all). Header must win.
    let request = format!(
        "GET /api/projects/{}/events?last_event_id=0 HTTP/1.1\r\nHost: {addr}\r\nLast-Event-ID: 2\r\nAccept: text/event-stream\r\n\r\n",
        project.id.0,
    );
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();

    let body = read_until_frame(&mut stream, std::time::Duration::from_secs(2)).await;
    let frames = parse_sse_events(&body);

    assert!(!frames.is_empty(), "expected replay frames");
    assert_eq!(frames[0].id, "3");
}

#[derive(Debug)]
struct SseFrame {
    id: String,
    data: String,
}

fn parse_sse_events(raw: &str) -> Vec<SseFrame> {
    let body = raw.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or(raw);
    let mut frames = Vec::new();
    let mut current_id = String::new();
    let mut current_data = String::new();
    for line in body.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            if !current_data.is_empty() || !current_id.is_empty() {
                frames.push(SseFrame {
                    id: std::mem::take(&mut current_id),
                    data: std::mem::take(&mut current_data),
                });
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("id:") {
            current_id = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("data:") {
            current_data.push_str(value.trim());
        }
    }
    frames
}

async fn read_until_frame(stream: &mut tokio::net::TcpStream, timeout: std::time::Duration) -> String {
    let mut buffer = vec![0u8; 8192];
    let mut accumulated = String::new();
    let deadline = tokio::time::Instant::now() + timeout;
    while let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now()) {
        let read = tokio::time::timeout(remaining, stream.read(&mut buffer)).await;
        let n = match read {
            Ok(Ok(n)) if n > 0 => n,
            _ => break,
        };
        accumulated.push_str(&String::from_utf8_lossy(&buffer[..n]));
        if let Some((_, body)) = accumulated.split_once("\r\n\r\n") {
            // At least one full SSE frame: terminated by a blank line in the body.
            if body.contains("\n\n") {
                break;
            }
        }
    }
    accumulated
}
