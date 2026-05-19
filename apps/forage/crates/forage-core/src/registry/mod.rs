use serde::{Deserialize, Serialize};

use crate::platform::PlatformError;

/// Compact component info for search/browse listings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentSummary {
    pub organisation: String,
    pub name: String,
    pub latest_version: String,
    pub kind: String,
    pub description: String,
    pub created_at: String,
    pub updated_at: String,
    pub version_count: i32,
    pub contracts: Vec<String>,
    pub visibility: String,
}

/// Version metadata with platform support info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentVersionInfo {
    pub version: String,
    pub protocol_version: String,
    pub kind: String,
    pub platforms: Vec<String>,
}

/// Full component detail (like a crates.io crate page).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentDetail {
    pub summary: ComponentSummary,
    pub versions: Vec<ComponentVersionInfo>,
    pub readme: String,
    pub manifest_json: String,
    pub owners: Vec<String>,
}

/// Paginated search result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentSearchResult {
    pub components: Vec<ComponentSummary>,
    pub total_count: i32,
}

#[async_trait::async_trait]
pub trait ForestRegistry: Send + Sync {
    /// Search/browse components across the registry.
    ///
    /// `page` is **1-indexed** (UI convention). Implementations targeting
    /// the 0-indexed proto must translate at the boundary.
    async fn search_components(
        &self,
        access_token: &str,
        query: &str,
        organisation: Option<&str>,
        page: i32,
        page_size: i32,
    ) -> Result<ComponentSearchResult, PlatformError>;

    /// Get full component detail (summary, versions, readme, manifest, owners).
    async fn get_component_detail(
        &self,
        access_token: &str,
        organisation: &str,
        name: &str,
    ) -> Result<ComponentDetail, PlatformError>;

    /// List all versions of a component with platform info.
    async fn list_component_versions(
        &self,
        access_token: &str,
        organisation: &str,
        name: &str,
    ) -> Result<Vec<ComponentVersionInfo>, PlatformError>;

    /// Get the manifest JSON for a specific component version.
    async fn get_component_manifest(
        &self,
        access_token: &str,
        organisation: &str,
        name: &str,
        version: &str,
    ) -> Result<String, PlatformError>;
}
