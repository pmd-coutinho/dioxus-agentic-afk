use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct HealthResponse {
    pub status: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct AppInfoResponse {
    pub app_name: String,
    pub version: String,
    pub api_status: String,
    pub config: EffectiveConfig,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct EffectiveConfig {
    pub bind_address: String,
    pub dashboard_asset_dir: String,
    pub database_url: String,
}

// --- Project contracts ---

/// Stable identifier for a Project, independent of filesystem path.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, ToSchema)]
pub struct ProjectId(pub String);

/// Request body for creating a new Project.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct CreateProjectRequest {
    pub path: String,
}

/// Response body representing a Project.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct ProjectResponse {
    pub id: ProjectId,
    pub path: String,
    pub git_summary: Option<GitSummary>,
    pub enabled_issue_source: Option<IssueSource>,
}

/// Read-only Git metadata derived from a Project path.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct GitSummary {
    pub branch: Option<String>,
    pub head: Option<String>,
    pub dirty: bool,
}

/// Persisted Issue Source selected for a Project.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct IssueSource {
    pub kind: String,
    pub locator: String,
}

/// Request body for deliberately enabling or switching a Project Issue Source.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct EnableIssueSourceRequest {
    pub kind: String,
    pub locator: String,
}

/// Advisory Issue Source candidate discovered from Project evidence.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct IssueSourceCandidate {
    pub kind: String,
    pub locator: String,
    pub enabled: bool,
}

/// RFC 7807 problem+json error response.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct ProblemDetail {
    #[serde(rename = "type")]
    pub problem_type: String,
    pub title: String,
    pub status: u16,
    pub detail: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_id_is_uuid_format() {
        let id = ProjectId("550e8400-e29b-41d4-a716-446655440000".to_string());
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, r#""550e8400-e29b-41d4-a716-446655440000""#);
        let deserialized: ProjectId = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, id);
    }

    #[test]
    fn create_project_request_serializes() {
        let req = CreateProjectRequest {
            path: "/home/user/my-project".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let deserialized: CreateProjectRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, req);
    }

    #[test]
    fn project_response_serializes() {
        let resp = ProjectResponse {
            id: ProjectId("550e8400-e29b-41d4-a716-446655440000".to_string()),
            path: "/home/user/my-project".to_string(),
            git_summary: None,
            enabled_issue_source: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let deserialized: ProjectResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, resp);
    }

    #[test]
    fn problem_detail_uses_rfc7807_field_names() {
        let problem = ProblemDetail {
            problem_type: "urn:agentic-afk:validation-error".to_string(),
            title: "Validation Error".to_string(),
            status: 422,
            detail: "Path does not exist".to_string(),
        };
        let json = serde_json::to_string(&problem).unwrap();
        // RFC 7807 uses "type" field name
        assert!(json.contains(r#""type":"urn:agentic-afk:validation-error""#));
        assert!(json.contains(r#""status":422"#));
        let deserialized: ProblemDetail = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, problem);
    }
}
