//! `forest-component-script` — generic engine that turns a directory of
//! shell scripts into a components-v2-protocol-speaking component.
//!
//! Layout each script-component lives under (typically baked into the
//! `exec-v1` image at `/usr/local/lib/forest-components/<name>/<v>/`):
//!
//! ```text
//! .../<name>/<version>/
//!   component             # entrypoint wrapper that exec's this engine
//!                         # with FOREST_COMPONENT_DIR set to the dir
//!   manifest.json         # declares: name, version, methods, schema
//!   scripts/<method>.sh   # executable, run when commands/<method> fires
//! ```
//!
//! Wrapper script body (POSIX sh, baked into the image):
//!
//! ```sh
//! #!/bin/sh
//! export FOREST_COMPONENT_DIR="$(cd -- "$(dirname -- "$0")" && pwd)"
//! exec /usr/local/lib/forest-components/_engine/forest-component-script "$@"
//! ```
//!
//! Protocol mapping:
//!
//! - `_meta/describe` → emits the methods declared in manifest.json,
//!   plus the protocol version baked into this binary.
//! - `commands/<name>` →
//!     * Inputs from payload.input arrive as `INPUT_<UPPERCASE_KEY>`
//!       env vars (matching the `uses: <image> + with:` shape).
//!     * Context from payload.context arrives as `FOREST_<UPPERCASE_KEY>`
//!       env vars (work_dir, organisation, project, environment,
//!       release_id, dry_run).
//!     * `FOREST_OUTPUT` points to a tmpfile the script writes
//!       `key=value` lines to. After the script exits, those lines are
//!       parsed and emitted as a JSON object on stdout — the same
//!       shape a hand-written Rust component would return.
//!     * Script exit code propagates verbatim.
//!
//! With this, shipping a new component is "drop a directory, write a
//! shell script" — no Rust crate, no recompile.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use anyhow::{Context, anyhow, bail};
use serde::{Deserialize, Serialize};

/// Protocol version this engine implements. Matches `forest_sdk::PROTOCOL_VERSION`.
const PROTOCOL_VERSION: &str = "1.1";

#[derive(Debug, Deserialize)]
struct Manifest {
    name: String,
    version: String,
    #[serde(default)]
    methods: Vec<MethodEntry>,
}

#[derive(Debug, Deserialize)]
struct MethodEntry {
    name: String,
    #[serde(default = "default_kind")]
    kind: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    topic: Option<String>,
}

fn default_kind() -> String {
    "command".to_string()
}

#[derive(Debug, Serialize)]
struct Descriptor<'a> {
    protocol_version: &'a str,
    methods: Vec<DescriptorMethod<'a>>,
}

#[derive(Debug, Serialize)]
struct DescriptorMethod<'a> {
    name: &'a str,
    kind: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    topic: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
struct Payload {
    #[serde(default)]
    input: serde_json::Value,
    #[serde(default)]
    context: serde_json::Value,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(err) => {
            eprintln!("forest-component-script: {err:#}");
            ExitCode::from(1)
        }
    }
}

fn run() -> anyhow::Result<u8> {
    let component_dir: PathBuf = std::env::var("FOREST_COMPONENT_DIR")
        .context("FOREST_COMPONENT_DIR not set — run via the entrypoint wrapper")?
        .into();
    let manifest_path = component_dir.join("manifest.json");
    let manifest_bytes = std::fs::read(&manifest_path)
        .with_context(|| format!("read manifest {}", manifest_path.display()))?;
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
        .with_context(|| format!("parse manifest {}", manifest_path.display()))?;

    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!(
            "usage: {} <method> [payload_json]\n\nfor component '{}/{}'",
            args[0], manifest.name, manifest.version
        );
        return Ok(1);
    }
    let method = &args[1];

    if method == "_meta/describe" {
        let descriptor = Descriptor {
            protocol_version: PROTOCOL_VERSION,
            methods: manifest
                .methods
                .iter()
                .map(|m| DescriptorMethod {
                    name: &m.name,
                    kind: &m.kind,
                    topic: m.topic.as_deref(),
                    description: m.description.as_deref(),
                })
                .collect(),
        };
        let json = serde_json::to_string_pretty(&descriptor)?;
        println!("{json}");
        return Ok(0);
    }

    // commands/<name> → scripts/<name>.sh
    let command_name = method
        .strip_prefix("commands/")
        .ok_or_else(|| anyhow!("unsupported method: {method}"))?;

    if !manifest.methods.iter().any(|m| m.name == command_name) {
        bail!(
            "component '{}/{}' has no method '{command_name}'",
            manifest.name,
            manifest.version
        );
    }

    let script_path = component_dir.join("scripts").join(format!("{command_name}.sh"));
    if !script_path.is_file() {
        bail!(
            "manifest declares '{command_name}' but {} is missing",
            script_path.display()
        );
    }

    // Payload: argv[2] when present, otherwise stdin.
    let raw_payload = if args.len() >= 3 {
        args[2].clone()
    } else {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
            .context("read payload from stdin")?;
        buf
    };
    let payload: Payload = serde_json::from_str(&raw_payload).context("parse payload JSON")?;

    let output_file = std::env::temp_dir().join(format!(
        "forest-component-script-out-{}-{}",
        std::process::id(),
        nanos()
    ));
    // Best-effort cleanup if a previous run left this around.
    let _ = std::fs::remove_file(&output_file);
    std::fs::File::create(&output_file)
        .with_context(|| format!("create output file {}", output_file.display()))?;

    let mut cmd = Command::new("sh");
    cmd.arg(script_path.as_os_str());

    // Strip the inherited environment in the same way `set -a` would
    // export everything we set explicitly. We deliberately keep PATH
    // and other host-side basics so the script can call git, gh, etc.
    cmd.env("FOREST_OUTPUT", &output_file);

    add_input_env(&mut cmd, &payload.input)?;
    add_context_env(&mut cmd, &payload.context);

    let status = cmd
        .status()
        .with_context(|| format!("spawn {}", script_path.display()))?;
    let exit = status.code().unwrap_or(1) as u8;

    // Translate the script's $FOREST_OUTPUT into a JSON object on
    // our stdout. Empty file → empty object. Same shape a hand-written
    // Rust component would return; keeps the runner-side parsing path
    // identical across both component types.
    let kv = std::fs::read_to_string(&output_file).unwrap_or_default();
    let _ = std::fs::remove_file(&output_file);
    let obj = parse_kv_to_json(&kv);
    let json = serde_json::to_string(&obj)?;
    if !json.is_empty() && json != "{}" {
        println!("{json}");
    } else {
        // Always emit something on stdout so the caller sees a clean
        // "no outputs" signal rather than blank.
        println!("{{}}");
    }

    Ok(exit)
}

/// Map JSON object inputs to `INPUT_<UPPERCASE_KEY>=value` env vars.
/// Dashes in keys map to underscores (matches the docker-mode convention).
/// Object/array values are JSON-stringified — scripts can parse with jq if
/// they care; most won't.
fn add_input_env(cmd: &mut Command, input: &serde_json::Value) -> anyhow::Result<()> {
    let serde_json::Value::Object(obj) = input else {
        return Ok(());
    };
    for (k, v) in obj {
        let key = format!("INPUT_{}", k.to_ascii_uppercase().replace('-', "_"));
        let value = match v {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Null => String::new(),
            other => serde_json::to_string(other)?,
        };
        cmd.env(key, value);
    }
    Ok(())
}

/// Map JSON object context to `FOREST_<UPPERCASE_KEY>=value` env vars.
fn add_context_env(cmd: &mut Command, context: &serde_json::Value) {
    let serde_json::Value::Object(obj) = context else {
        return;
    };
    for (k, v) in obj {
        let key = format!("FOREST_{}", k.to_ascii_uppercase());
        let value = match v {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Bool(b) => b.to_string(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Null => String::new(),
            other => serde_json::to_string(other).unwrap_or_default(),
        };
        cmd.env(key, value);
    }
}

/// Parse `key=value` lines (skipping blanks and `#`-comments) into a
/// JSON object. Values are kept as strings — workflows that need typed
/// values can teach individual components to emit JSON-shaped output.
fn parse_kv_to_json(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            let k = k.trim();
            if k.is_empty() {
                continue;
            }
            out.insert(k.to_string(), v.to_string());
        }
    }
    out
}

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}
