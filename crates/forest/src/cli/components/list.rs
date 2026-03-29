use crate::{grpc::GrpcClientState, state::State};

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
            println!("No components found.");
            return Ok(());
        }

        for comp in &components {
            let contracts = if comp.contracts.is_empty() {
                String::new()
            } else {
                format!(" [{}]", comp.contracts.join(", "))
            };

            println!(
                "{}/{}@{}  {}{}",
                comp.organisation,
                comp.name,
                comp.latest_version,
                comp.kind,
                contracts,
            );
            if !comp.description.is_empty() {
                println!("  {}", comp.description);
            }
        }

        println!("\n{} component(s)", components.len());

        Ok(())
    }
}
