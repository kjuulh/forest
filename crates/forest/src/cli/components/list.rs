use crate::{grpc::GrpcClientState, state::State};

fn shape_badge(shape: i32) -> &'static str {
    use forest_grpc_interface::ComponentShape;
    match ComponentShape::try_from(shape) {
        Ok(ComponentShape::Component) => "component",
        Ok(ComponentShape::Hybrid) => "hybrid",
        Ok(ComponentShape::ToolBinary) => "tool",
        Ok(ComponentShape::ToolExternal) => "tool-ext",
        _ => "unspecified",
    }
}

#[derive(serde::Serialize, tabled::Tabled)]
struct ComponentRow {
    #[tabled(rename = "ORG/NAME")]
    qualified: String,
    #[tabled(rename = "VERSION")]
    version: String,
    #[tabled(rename = "SHAPE")]
    shape: String,
    #[tabled(rename = "UPSTREAM")]
    upstream_host: String,
    #[tabled(rename = "DESCRIPTION")]
    description: String,
}

/// Search and list components in the registry.
///
/// Shows available components with their latest version, kind, and contracts.
/// Optionally filter by organisation or search query.
///
/// Examples:
///   forest components list
///   forest components list --org forest-contrib
///   forest components list --query terraform
#[derive(clap::Parser)]
pub struct ListCommand {
    /// Filter by organisation
    #[arg(long, short = 'o')]
    org: Option<String>,

    /// Search query (matches name and description)
    #[arg(long, short = 'q')]
    query: Option<String>,
}

impl ListCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let client = state.grpc_client();

        let components = client
            .search_components(
                self.query.as_deref().unwrap_or(""),
                self.org.as_deref().unwrap_or(""),
            )
            .await?;

        if components.is_empty() {
            use crate::cli::output::OutputFormat;
            match state.config.format {
                OutputFormat::Pretty | OutputFormat::Text => {
                    eprintln!("No components found.");
                }
                OutputFormat::Name => {}
                OutputFormat::Json => println!("[]"),
            }
            return Ok(());
        }

        let rows: Vec<ComponentRow> = components
            .iter()
            .map(|comp| ComponentRow {
                qualified: format!("{}/{}@{}", comp.organisation, comp.name, comp.latest_version),
                version: comp.latest_version.clone(),
                shape: shape_badge(comp.shape).to_string(),
                upstream_host: comp.upstream_host.clone(),
                description: comp.description.clone(),
            })
            .collect();
        print!("{}", crate::cli::output::render(&state.config.format, &rows));
        Ok(())
    }
}
