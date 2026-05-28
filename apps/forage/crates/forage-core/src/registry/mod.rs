use serde::{Deserialize, Serialize};

use crate::platform::PlatformError;

/// Compact component info for search/browse listings.
///
/// `shape` carries the §1a.2e taxonomy from TASKS/018-global-tools.md so the
/// UI can drive cross-links between `/components/{org}/{name}` and
/// `/tools/{org}/{name}` based on whether the artefact is a regular component,
/// a hybrid, or a tool.
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
    /// Populated for HYBRID / TOOL_* shapes; None for plain COMPONENT.
    #[serde(default)]
    pub tool: Option<ToolFacet>,
    /// Method names (populated for COMPONENT / HYBRID).
    #[serde(default)]
    pub methods: Vec<String>,
    /// For TOOL_EXTERNAL only; the full URL appears only on detail responses.
    #[serde(default)]
    pub upstream_host: String,
}

/// Tool-side metadata mirroring forest's `ToolFacet` proto message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolFacet {
    pub name: String,
    pub argv_passthrough: bool,
    pub description: String,
}

/// Component / tool shape — domain mirror of forest's `ComponentShape` proto
/// enum. We keep this as a separate enum so the UI layer never depends on
/// generated proto code.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolShape {
    /// COMPONENT_SHAPE_UNSPECIFIED — server returned 0 / a future variant.
    #[default]
    Unknown,
    /// COMPONENT_SHAPE_COMPONENT — binary + methods, no tool facet.
    Component,
    /// COMPONENT_SHAPE_HYBRID — binary + methods + tool facet.
    Hybrid,
    /// COMPONENT_SHAPE_TOOL_BINARY — binary + tool facet, no methods.
    ToolBinary,
    /// COMPONENT_SHAPE_TOOL_EXTERNAL — external URL manifest + tool facet.
    ToolExternal,
}

impl ToolShape {
    /// `true` if this shape should be surfaced on the Tools tab. Plain
    /// components return `false`.
    pub fn is_tool(self) -> bool {
        matches!(self, Self::Hybrid | Self::ToolBinary | Self::ToolExternal)
    }

    /// Short human label used by templates / tests. Matches the CLI badges
    /// (`tool`, `hybrid`, `tool-ext`).
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

/// Compact tool info for the Tools list views (org-scoped + global).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSummary {
    pub organisation: String,
    pub name: String,
    pub latest_version: String,
    pub shape: ToolShape,
    /// Falls back to `""` when the tool facet has no description.
    pub description: String,
    pub argv_passthrough: bool,
    /// Populated for TOOL_EXTERNAL only. `""` for other shapes.
    pub upstream_host: String,
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

    /// Public catalog search. Calls `SearchPublicComponents`, which
    /// always restricts to projects with `visibility = 'public'` and
    /// never reads any access token. Use for the unauthenticated
    /// `/components` surface.
    async fn search_public_components(
        &self,
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

    /// Public-only detail. Returns `NotFound` for any component whose
    /// project is private. Calls `GetPublicComponentDetail`.
    async fn get_public_component_detail(
        &self,
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

    /// Public-only manifest fetch. Returns `NotFound` when the owning
    /// project is private. Calls `GetPublicComponentManifest`.
    async fn get_public_component_manifest(
        &self,
        organisation: &str,
        name: &str,
        version: &str,
    ) -> Result<String, PlatformError>;

    /// List the tools published by an organisation. Mirrors the forest-server
    /// `ListOrgTools` RPC (TASKS/018-global-tools.md), which filters to shape
    /// in {HYBRID, TOOL_BINARY, TOOL_EXTERNAL} and returns the highest
    /// non-prerelease version per tool. The streaming response is collected
    /// at the wrapper boundary so callers see a simple `Vec`.
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
        // Tools — surfaced on the Tools tab.
        assert!(ToolShape::Hybrid.is_tool());
        assert!(ToolShape::ToolBinary.is_tool());
        assert!(ToolShape::ToolExternal.is_tool());
        // Not tools — surfaced on the Components tab.
        assert!(!ToolShape::Component.is_tool());
        assert!(!ToolShape::Unknown.is_tool());
    }

    #[test]
    fn label_matches_cli_badges() {
        // Same vocabulary the CLI uses on `forest components list`:
        //   [component] / [hybrid] / [tool] / [tool-ext]
        assert_eq!(ToolShape::Component.label(), "component");
        assert_eq!(ToolShape::Hybrid.label(), "hybrid");
        assert_eq!(ToolShape::ToolBinary.label(), "tool");
        assert_eq!(ToolShape::ToolExternal.label(), "tool-ext");
        assert_eq!(ToolShape::Unknown.label(), "tool");
    }

    #[test]
    fn default_shape_is_unknown_for_forward_compat() {
        // Any new shape variant we don't know about must round-trip as
        // Unknown, never crash deserialization.
        assert_eq!(ToolShape::default(), ToolShape::Unknown);
    }
}
