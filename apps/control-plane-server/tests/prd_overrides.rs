//! HTTP boundary tests for the PRD override endpoints.
//!
//! Verifies that marking a Source Issue as a Parent-Issue-style PRD strips it
//! from every active Planning Snapshot bucket and exposes it through the new
//! `prd_overrides` field. Unmarking restores the original bucketing.

use agentic_afk_contracts::{
    CreateProjectRequest, EnableIssueSourceRequest, PlanningSnapshotResponse, ProjectResponse,
};
use agentic_afk_control_plane_server::{ControlPlaneConfig, router};
use agentic_afk_persistence as persistence;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use std::path::PathBuf;
use tower::ServiceExt;

fn unique_path(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "agentic-afk-prd-{label}-{}-{nanos}",
        std::process::id()
    ))
}

async fn build_router() -> axum::Router {
    let db = persistence::connect_in_memory().await.unwrap();
    persistence::migrate(&db).await.unwrap();
    let config = ControlPlaneConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        dashboard_asset_dir: "target/dx/agentic-afk-dashboard/release/web/public".into(),
        database_url: "sqlite::memory:".into(),
        gh_binary_path: "gh".into(),
        worktrunk_binary_path: "wt".into(),
        codex_binary_path: "codex".into(),
    };
    router(config, db)
}

async fn seed_project_with_eligible_issue(app: &axum::Router) -> ProjectResponse {
    let project_path = unique_path("project");
    let issues_dir = project_path.join(".scratch/issues");
    std::fs::create_dir_all(&issues_dir).unwrap();
    std::fs::create_dir_all(project_path.join(".git")).unwrap();
    std::fs::write(
        issues_dir.join("001-prd.md"),
        "# PRD One\n\nReadiness: ready\nSource Order: 1\n",
    )
    .unwrap();
    std::fs::write(
        issues_dir.join("002-work.md"),
        "# Real work\n\nReadiness: ready\nSource Order: 2\n",
    )
    .unwrap();

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
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let project: ProjectResponse = serde_json::from_slice(&bytes).unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/projects/{}/issue-source", project.id.0))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&EnableIssueSourceRequest {
                        kind: "local_markdown".into(),
                        locator: ".scratch/issues".into(),
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/projects/{}/issue-source/sync", project.id.0))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    project
}

async fn fetch_planning(app: &axum::Router, project_id: &str) -> PlanningSnapshotResponse {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/projects/{project_id}/planning-snapshot"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn mark_prd_moves_eligible_issue_into_overrides_bucket() {
    let app = build_router().await;
    let project = seed_project_with_eligible_issue(&app).await;

    let before = fetch_planning(&app, &project.id.0).await;
    let eligible_ids: Vec<&str> = before.eligible.iter().map(|i| i.source_id.as_str()).collect();
    assert!(eligible_ids.contains(&"001-prd"));
    assert!(eligible_ids.contains(&"002-work"));
    assert!(before.prd_overrides.is_empty());

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/projects/{}/source-issues/001-prd/prd",
                    project.id.0
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let after = fetch_planning(&app, &project.id.0).await;
    let eligible_ids: Vec<&str> = after.eligible.iter().map(|i| i.source_id.as_str()).collect();
    assert!(!eligible_ids.contains(&"001-prd"));
    assert!(eligible_ids.contains(&"002-work"));
    let prd_ids: Vec<&str> = after
        .prd_overrides
        .iter()
        .map(|i| i.source_id.as_str())
        .collect();
    assert_eq!(prd_ids, vec!["001-prd"]);
}

#[tokio::test]
async fn unmark_prd_restores_original_bucketing() {
    let app = build_router().await;
    let project = seed_project_with_eligible_issue(&app).await;

    let mark = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/projects/{}/source-issues/001-prd/prd",
                    project.id.0
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(mark.status(), StatusCode::NO_CONTENT);

    let unmark = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/api/projects/{}/source-issues/001-prd/prd",
                    project.id.0
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unmark.status(), StatusCode::NO_CONTENT);

    let snapshot = fetch_planning(&app, &project.id.0).await;
    let eligible_ids: Vec<&str> = snapshot
        .eligible
        .iter()
        .map(|i| i.source_id.as_str())
        .collect();
    assert!(eligible_ids.contains(&"001-prd"));
    assert!(snapshot.prd_overrides.is_empty());
}

#[tokio::test]
async fn mark_prd_is_idempotent() {
    let app = build_router().await;
    let project = seed_project_with_eligible_issue(&app).await;

    for _ in 0..3 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/projects/{}/source-issues/001-prd/prd",
                        project.id.0
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }
    let snapshot = fetch_planning(&app, &project.id.0).await;
    let prd_ids: Vec<&str> = snapshot
        .prd_overrides
        .iter()
        .map(|i| i.source_id.as_str())
        .collect();
    assert_eq!(prd_ids, vec!["001-prd"]);
}
