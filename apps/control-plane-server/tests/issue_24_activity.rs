//! Tests for issue #24: Expose comprehensive Issue Assignment Activity on Project detail.
//!
//! Covers the API projection of `project_activity` rows and the bounded
//! `detail` projection that prevents full Codex output from leaking into the
//! Activity audit log.

use agentic_afk_contracts::{
    CreateProjectRequest, EnableIssueSourceRequest, ProjectResponse,
};
use agentic_afk_control_plane_server::{ControlPlaneConfig, router};
use agentic_afk_persistence::{self as persistence};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use tower::ServiceExt;

fn temp_project_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "agentic-afk-{name}-{}-{}",
        std::process::id(),
        unique_nonce()
    ))
}

fn unique_nonce() -> u128 {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let salt = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    nanos.wrapping_add(u128::from(salt))
}

fn write_fake_command(name: &str, body: &str) -> PathBuf {
    let path = temp_project_path(name);
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();
    }
    path
}

async fn test_router(
    worktrunk_binary_path: PathBuf,
    codex_binary_path: PathBuf,
) -> (axum::Router, persistence::Db) {
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "target/dx/agentic-afk-dashboard/release/web/public".into(),
        database_url: "sqlite::memory:".into(),
        gh_binary_path: "gh".into(),
        worktrunk_binary_path,
        codex_binary_path,
    };
    (router(config, db.clone()), db)
}

fn setup_local_markdown_project(name: &str) -> (PathBuf, PathBuf) {
    let project_path = temp_project_path(name);
    let issues_dir = project_path.join(".scratch/issues");
    std::fs::create_dir_all(&issues_dir).unwrap();
    std::fs::create_dir_all(project_path.join(".git")).unwrap();
    (project_path, issues_dir)
}

async fn create_trusted_local_markdown_project(
    app: &axum::Router,
    project_path: &std::path::Path,
) -> ProjectResponse {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/projects")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&CreateProjectRequest {
                        path: project_path.display().to_string(),
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let project: ProjectResponse = serde_json::from_slice(&body).unwrap();

    app.clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/projects/{}/issue-source", project.id.0))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&EnableIssueSourceRequest {
                        kind: "local_markdown".to_string(),
                        locator: ".scratch/issues".to_string(),
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/projects/{}/trust", project.id.0))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).unwrap()
}

/// Activity detail is truncated so full Codex output never lands in the audit
/// log.
#[tokio::test]
async fn activity_detail_is_truncated_to_protect_against_codex_output() {
    use agentic_afk_persistence::PROJECT_ACTIVITY_DETAIL_MAX_BYTES;

    let (project_path, _issues_dir) = setup_local_markdown_project("activity-truncate");
    let (app, db) = test_router(
        write_fake_command("activity-truncate-wt", "#!/bin/sh\nexit 0\n"),
        write_fake_command("activity-truncate-codex", "#!/bin/sh\nexit 0\n"),
    )
    .await;
    let project = create_trusted_local_markdown_project(&app, &project_path).await;
    let huge_detail = "x".repeat(PROJECT_ACTIVITY_DETAIL_MAX_BYTES * 4);
    let entry = agentic_afk_persistence::record_project_activity(
        &db,
        &project.id.0,
        None,
        "test_truncation",
        Some(&huge_detail),
    )
    .await
    .unwrap();
    let stored = entry.detail.unwrap();
    assert!(stored.len() <= PROJECT_ACTIVITY_DETAIL_MAX_BYTES + 8);
    assert!(stored.ends_with('…'));
}

/// The activity endpoint rejects unknown projects with a problem+json 404.
#[tokio::test]
async fn activity_endpoint_returns_404_for_unknown_project() {
    let (app, _db) = test_router(
        write_fake_command("activity-unknown-wt", "#!/bin/sh\nexit 0\n"),
        write_fake_command("activity-unknown-codex", "#!/bin/sh\nexit 0\n"),
    )
    .await;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/projects/does-not-exist/activity")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
