//! `forest components show <org>/<name>` — full component detail (shape,
//! tool facet, methods, platforms, versions, upstream URL for externals).

use crate::{grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct ShowCommand {
    /// `<org>/<name>` reference.
    component: String,
}

impl ShowCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let (org, name) = self
            .component
            .split_once('/')
            .ok_or_else(|| anyhow::anyhow!("expected <org>/<name>, got {:?}", self.component))?;

        let client = state.grpc_client();
        let detail = client.get_component_detail(org, name).await?;
        let summary = detail.summary.as_ref().ok_or_else(|| {
            anyhow::anyhow!("component not found: {org}/{name}")
        })?;

        use crate::cli::output::OutputFormat;
        match state.config.format {
            OutputFormat::Json => {
                // Surface the full detail response verbatim — scriptable.
                let body = serde_json::json!({
                    "summary": {
                        "organisation": summary.organisation,
                        "name": summary.name,
                        "latest_version": summary.latest_version,
                        "kind": summary.kind,
                        "shape": shape_label(summary.shape),
                        "description": summary.description,
                        "visibility": summary.visibility,
                        "version_count": summary.version_count,
                        "tool": summary.tool.as_ref().map(|t| serde_json::json!({
                            "name": t.name,
                            "argv_passthrough": t.argv_passthrough,
                            "description": t.description,
                        })),
                        "methods": summary.methods,
                        "contracts": summary.contracts,
                        "upstream_host": summary.upstream_host,
                    },
                    "versions": detail.versions.iter().map(|v| serde_json::json!({
                        "version": v.version,
                        "kind": v.kind,
                        "platforms": v.platforms,
                    })).collect::<Vec<_>>(),
                    "manifest_json": detail.manifest_json,
                });
                println!("{}", serde_json::to_string_pretty(&body)?);
                return Ok(());
            }
            OutputFormat::Name => {
                println!("{}/{}", summary.organisation, summary.name);
                return Ok(());
            }
            OutputFormat::Pretty | OutputFormat::Text => {
                // fall through to the rich text rendering below
            }
        }

        println!("{}/{} @ {}", summary.organisation, summary.name, summary.latest_version);
        println!("  shape:     {}", shape_label(summary.shape));
        println!("  kind:      {}", summary.kind);
        if !summary.description.is_empty() {
            println!("  desc:      {}", summary.description);
        }
        if !summary.visibility.is_empty() {
            println!("  visibility: {}", summary.visibility);
        }
        if let Some(tool) = &summary.tool {
            if !tool.name.is_empty() {
                println!(
                    "  tool:      {} (argv passthrough)",
                    tool.name
                );
                if !tool.description.is_empty() {
                    println!("             {}", tool.description);
                }
            }
        }
        if !summary.methods.is_empty() {
            println!("  methods:   {}", summary.methods.join(", "));
        }
        if !summary.contracts.is_empty() {
            println!("  contracts: {}", summary.contracts.join(", "));
        }
        if !summary.upstream_host.is_empty() {
            println!("  upstream:  {}", summary.upstream_host);
        }

        if !detail.versions.is_empty() {
            println!("  versions:");
            for v in &detail.versions {
                let platforms = if v.platforms.is_empty() {
                    "(no platforms)".to_string()
                } else {
                    v.platforms.join(", ")
                };
                println!("    - {}  [{}]  {}", v.version, v.kind, platforms);
            }
        }

        if !summary.upstream_host.is_empty() && !detail.manifest_json.is_empty() {
            // Surface the full URL only on the detail view (§1a.2e: full URL is
            // detail-only, host on list).
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&detail.manifest_json)
                && let Some(p) = v.get("platforms").and_then(|p| p.as_object())
            {
                println!("  platform urls:");
                for (key, platform) in p {
                    if let Some(u) = platform.get("url").and_then(|u| u.as_str()) {
                        println!("    - {key}: {u}");
                    }
                }
            }
        }

        Ok(())
    }
}

fn shape_label(shape: i32) -> &'static str {
    use forest_grpc_interface::ComponentShape;
    match ComponentShape::try_from(shape) {
        Ok(ComponentShape::Component) => "component",
        Ok(ComponentShape::Hybrid) => "hybrid_component",
        Ok(ComponentShape::ToolBinary) => "tool_binary",
        Ok(ComponentShape::ToolExternal) => "tool_external",
        _ => "unspecified",
    }
}
