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

/// Dispatch interface for a component, generic over the spec type `S`.
pub trait ComponentService<S>: Send + Sync {
    fn call(
        &self,
        method: &str,
        spec: &S,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, Error>;

    fn methods(&self) -> Vec<MethodDescriptor>;
}

pub struct MethodDescriptor {
    pub name: String,
    pub kind: MethodKind,
}

pub enum MethodKind {
    Command,
    Hook { topic: String },
}

/// Run a single invocation against a component service, then exit.
///
/// Reads the method from CLI args and the payload from either a CLI
/// argument or stdin:
///
/// ```text
/// # Payload as CLI argument:
/// ./component <method> '{"spec": {...}, "input": {...}}'
///
/// # Payload via stdin:
/// echo '{"spec": {...}, "input": {...}}' | ./component <method>
/// ```
///
/// `input` defaults to `{}` if omitted from the payload.
/// Prints JSON result to stdout on success, error to stderr with exit code 1.
pub fn run_once<S: serde::de::DeserializeOwned, CS: ComponentService<S>>(service: &CS) {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("usage: {} <method> [payload_json]", args[0]);
        eprintln!();
        eprintln!("payload format: {{\"spec\": {{...}}, \"input\": {{...}}}}");
        eprintln!("payload can be passed as a CLI argument or piped via stdin");
        std::process::exit(1);
    }

    let method = &args[1];

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

    match service.call(method, &spec, input) {
        Ok(output) => {
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

#[derive(serde::Deserialize)]
struct Payload {
    spec: serde_json::Value,
    input: Option<serde_json::Value>,
}
