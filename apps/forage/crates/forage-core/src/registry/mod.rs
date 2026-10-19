use serde::{Deserialize, Serialize};

use crate::platform::PlatformError;

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
    #[serde(default)]
    pub shape: ToolShape,
    #[serde(default)]
    pub tool: Option<ToolFacet>,
    #[serde(default)]
    pub methods: Vec<String>,
    #[serde(default)]
    pub upstream_host: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolFacet {
    pub name: String,
    pub argv_passthrough: bool,
    pub description: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolShape {
    #[default]
    Unknown,
    Component,
    Hybrid,
    ToolBinary,
    ToolExternal,
}

impl ToolShape {
    pub fn is_tool(self) -> bool {
        matches!(self, Self::Hybrid | Self::ToolBinary | Self::ToolExternal)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Component => "component",
            Self::Hybrid => "hybrid",
            Self::ToolBinary => "tool",
            Self::ToolExternal => "tool-ext",
            Self::Unknown => "tool",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSummary {
    pub organisation: String,
    pub name: String,
    pub latest_version: String,
    pub shape: ToolShape,
    pub description: String,
    pub argv_passthrough: bool,
    pub upstream_host: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentVersionInfo {
    pub version: String,
    pub protocol_version: String,
    pub kind: String,
    pub platforms: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentDetail {
    pub summary: ComponentSummary,
    pub versions: Vec<ComponentVersionInfo>,
    pub readme: String,
    pub manifest_json: String,
    pub owners: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentSearchResult {
    pub components: Vec<ComponentSummary>,
    pub total_count: i32,
}

#[async_trait::async_trait]
pub trait ForestRegistry: Send + Sync {
    /// `page` is 1-indexed; translate to the 0-indexed proto at the boundary.
    async fn search_components(
        &self,
        access_token: &str,
        query: &str,
        organisation: Option<&str>,
        page: i32,
        page_size: i32,
    ) -> Result<ComponentSearchResult, PlatformError>;

    /// Public projects only and never reads an access token: safe unauthenticated.
    async fn search_public_components(
        &self,
        query: &str,
        organisation: Option<&str>,
        page: i32,
        page_size: i32,
    ) -> Result<ComponentSearchResult, PlatformError>;

    async fn get_component_detail(
        &self,
        access_token: &str,
        organisation: &str,
        name: &str,
    ) -> Result<ComponentDetail, PlatformError>;

    /// `NotFound` for any component whose project is private.
    async fn get_public_component_detail(
        &self,
        organisation: &str,
        name: &str,
    ) -> Result<ComponentDetail, PlatformError>;

    async fn list_component_versions(
        &self,
        access_token: &str,
        organisation: &str,
        name: &str,
    ) -> Result<Vec<ComponentVersionInfo>, PlatformError>;

    async fn get_component_manifest(
        &self,
        access_token: &str,
        organisation: &str,
        name: &str,
        version: &str,
    ) -> Result<String, PlatformError>;

    /// `NotFound` when the owning project is private.
    async fn get_public_component_manifest(
        &self,
        organisation: &str,
        name: &str,
        version: &str,
    ) -> Result<String, PlatformError>;

    async fn list_org_tools(
        &self,
        access_token: &str,
        organisation: &str,
    ) -> Result<Vec<ToolSummary>, PlatformError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_tool_classifies_each_shape() {
        assert!(ToolShape::Hybrid.is_tool());
        assert!(ToolShape::ToolBinary.is_tool());
        assert!(ToolShape::ToolExternal.is_tool());
        assert!(!ToolShape::Component.is_tool());
        assert!(!ToolShape::Unknown.is_tool());
    }

    #[test]
    fn label_matches_cli_badges() {
        assert_eq!(ToolShape::Component.label(), "component");
        assert_eq!(ToolShape::Hybrid.label(), "hybrid");
        assert_eq!(ToolShape::ToolBinary.label(), "tool");
        assert_eq!(ToolShape::ToolExternal.label(), "tool-ext");
        assert_eq!(ToolShape::Unknown.label(), "tool");
    }

    #[test]
    fn default_shape_is_unknown_for_forward_compat() {
        assert_eq!(ToolShape::default(), ToolShape::Unknown);
    }
}
