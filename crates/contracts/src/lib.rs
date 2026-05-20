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
