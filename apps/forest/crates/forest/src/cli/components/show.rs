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
        // Default env shipped with the tool (TASKS/023 `include.env`). Read
        // from the manifest, which the server returns verbatim.
        let env_defaults = include_env_from_manifest(&detail.manifest_json);
        if !env_defaults.is_empty() {
            println!("  env defaults:");
            for (k, val) in &env_defaults {
                println!("    - {k}={val}");
            }
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

/// Extract `include.env` (TASKS/023) from a manifest JSON blob as sorted
/// key→value pairs. Empty for any missing/malformed input — the server
/// returns the manifest verbatim, so this is the same data the JSON output
/// surfaces under `manifest_json`.
fn include_env_from_manifest(manifest_json: &str) -> std::collections::BTreeMap<String, String> {
    serde_json::from_str::<serde_json::Value>(manifest_json)
        .ok()
        .as_ref()
        .and_then(|v| v.pointer("/include/env"))
        .and_then(|e| e.as_object())
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
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

#[cfg(test)]
mod tests {
    use super::include_env_from_manifest;

    #[test]
    fn extracts_include_env() {
        let m = r#"{"kind":"binary","include":{"env":{"FUNGUS_SERVER":"https://fungus.understory.sh"}}}"#;
        let env = include_env_from_manifest(m);
        assert_eq!(env.get("FUNGUS_SERVER").map(String::as_str), Some("https://fungus.understory.sh"));
    }

    #[test]
    fn empty_when_no_include() {
        assert!(include_env_from_manifest(r#"{"kind":"binary"}"#).is_empty());
    }

    #[test]
    fn empty_on_garbage() {
        assert!(include_env_from_manifest("not json").is_empty());
        assert!(include_env_from_manifest("").is_empty());
    }
}
