pub use error::Error;

pub mod error {
    #[derive(thiserror::Error, Debug)]
    pub enum Error {
        #[error("method not found: {0}")]
        MethodNotFound(String),
        #[error("deserialization error: {0}")]
        Deserialization(#[from] serde_json::Error),
        #[error("handler error: {0}")]
        Handler(#[source] Box<dyn std::error::Error + Send + Sync>),
    }
}

/// Protocol version supported by this SDK.
pub const PROTOCOL_VERSION: &str = "1.1";

/// Execution context provided by the Forest runtime when invoking a component.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct CallContext {
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub organisation: Option<String>,
    #[serde(default)]
    pub environment: Option<String>,
    #[serde(default)]
    pub release_id: Option<String>,
    #[serde(default)]
    pub work_dir: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
}

/// Dispatch interface for a component, generic over the spec type `S`.
///
/// All handler methods are async to support real I/O (HTTP calls, kubectl, etc.)
/// inside component implementations.
pub trait ComponentService<S>: Send + Sync {
    fn call(
        &self,
        method: &str,
        spec: &S,
        input: serde_json::Value,
        context: &CallContext,
    ) -> impl std::future::Future<Output = Result<serde_json::Value, Error>> + Send;

    fn methods(&self) -> Vec<MethodDescriptor>;

    /// Template rendering configuration. Override to customize how
    /// Forest renders the component's template files (skip, rename, extra vars).
    fn template_config(&self) -> TemplateConfig {
        TemplateConfig::default()
    }
}

pub struct MethodDescriptor {
    pub name: String,
    pub kind: MethodKind,
    pub description: Option<String>,
}

pub enum MethodKind {
    Command,
    Hook { topic: String },
}

/// Template rendering configuration returned by `_meta/template_config`.
/// Controls how Forest renders the component's template files.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct TemplateConfig {
    /// File patterns to skip (glob syntax, e.g., "*.bak", "README.md")
    #[serde(default)]
    pub skip: Vec<String>,
    /// File renames: source name → target name (supports {{name}} substitution)
    #[serde(default)]
    pub rename: std::collections::HashMap<String, String>,
    /// Extra template variables injected alongside the config
    #[serde(default)]
    pub vars: std::collections::HashMap<String, serde_json::Value>,
}

/// Response for `_meta/describe`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ComponentDescriptor {
    pub protocol_version: String,
    pub methods: Vec<MethodInfo>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MethodInfo {
    pub name: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Run a single invocation against a component service, then exit.
///
/// Reads the method from CLI args and the payload from either a CLI
/// argument or stdin:
///
/// ```text
/// # Payload via stdin:
/// echo '{"spec": {...}, "input": {...}}' | ./component <method>
/// ```
///
/// Handles `_meta/describe` automatically (no payload required).
pub fn run_once<S: serde::de::DeserializeOwned, CS: ComponentService<S>>(service: &CS) {
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    rt.block_on(run_once_async(service));
}

async fn run_once_async<S: serde::de::DeserializeOwned, CS: ComponentService<S>>(service: &CS) {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("usage: {} <method> [payload_json]", args[0]);
        eprintln!();
        eprintln!("payload format: {{\"spec\": {{...}}, \"input\": {{...}}}}");
        eprintln!("payload can be passed as a CLI argument or piped via stdin");
        std::process::exit(1);
    }

    let method = &args[1];

    // Handle meta-methods that don't require a payload
    if method == "_meta/describe" {
        let descriptor = build_descriptor(service);
        match serde_json::to_string_pretty(&descriptor) {
            Ok(json) => println!("{json}"),
            Err(e) => {
                eprintln!("error: failed to serialize descriptor: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    if method == "_meta/template_config" {
        let config = service.template_config();
        match serde_json::to_string_pretty(&config) {
            Ok(json) => println!("{json}"),
            Err(e) => {
                eprintln!("error: failed to serialize template config: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    let raw: String = if args.len() >= 3 {
        args[2].clone()
    } else {
        let mut buf = String::new();
        if let Err(e) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf) {
            eprintln!("error: failed to read stdin: {e}");
            std::process::exit(1);
        }

        buf
    };

    let payload: Payload = match serde_json::from_str(&raw) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: invalid payload JSON: {e}");
            std::process::exit(1);
        }
    };

    let spec: S = match serde_json::from_value(payload.spec) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: invalid spec: {e}");
            std::process::exit(1);
        }
    };

    let input = payload
        .input
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

    let context = payload.context.unwrap_or_default();

    match service.call(method, &spec, input, &context).await {
        Ok(output) => match serde_json::to_string_pretty(&output) {
            Ok(json) => println!("{json}"),
            Err(e) => {
                eprintln!("error: failed to serialize output: {e}");
                std::process::exit(1);
            }
        },
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

fn build_descriptor<S, CS: ComponentService<S>>(service: &CS) -> ComponentDescriptor {
    let methods = service
        .methods()
        .into_iter()
        .map(|m| MethodInfo {
            name: m.name,
            kind: match &m.kind {
                MethodKind::Command => "command".to_string(),
                MethodKind::Hook { .. } => "hook".to_string(),
            },
            topic: match &m.kind {
                MethodKind::Command => None,
                MethodKind::Hook { topic } => Some(topic.clone()),
            },
            description: m.description,
        })
        .collect();

    ComponentDescriptor {
        protocol_version: PROTOCOL_VERSION.to_string(),
        methods,
    }
}

#[derive(serde::Deserialize)]
struct Payload {
    spec: serde_json::Value,
    input: Option<serde_json::Value>,
    /// Execution context (optional for backward compatibility).
    #[serde(default)]
    context: Option<CallContext>,
}
