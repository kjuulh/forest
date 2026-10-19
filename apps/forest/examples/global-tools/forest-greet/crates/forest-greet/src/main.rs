use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct GreetInput {
    #[serde(default)]
    name: Option<String>,
}

#[derive(Serialize)]
struct GreetOutput {
    greeting: String,
}

#[derive(Deserialize)]
struct ProtocolPayload {
    #[serde(default)]
    input: serde_json::Value,
}

fn protocol_method(arg: &str) -> bool {
    arg.starts_with("_meta/") || arg.starts_with("commands/") || arg.starts_with("hooks/")
}

fn main() {
    let mut args = std::env::args().skip(1);
    let first = args.next();

    match first {
        Some(method) if protocol_method(&method) => {
            let payload_json = args
                .next()
                .unwrap_or_else(|| r#"{"input":{}}"#.to_string());
            handle_protocol(&method, &payload_json);
        }
        Some(name) => {
            print_greeting(Some(name));
        }
        None => {
            print_greeting(None);
        }
    }
}

fn handle_protocol(method: &str, payload_json: &str) {
    match method {
        "_meta/describe" => {
            let resp = serde_json::json!({
                "protocol_version": "1.1",
                "methods": [
                    {"name": "greet", "kind": "command", "description": "Return a greeting as structured JSON"}
                ],
                "tool": {
                    "name": "greet",
                    "argv_passthrough": true,
                    "description": "Print a friendly greeting (callable as a CLI or as a Forest command)"
                }
            });
            println!("{}", serde_json::to_string(&resp).unwrap());
        }
        "commands/greet" => {
            let payload: ProtocolPayload =
                serde_json::from_str(payload_json).expect("payload must be json");
            let input: GreetInput =
                serde_json::from_value(payload.input).unwrap_or(GreetInput { name: None });
            let greeting = format!(
                "hello, {}!",
                input.name.unwrap_or_else(|| "world".to_string())
            );
            let out = GreetOutput { greeting };
            println!("{}", serde_json::to_string(&out).unwrap());
        }
        other => {
            eprintln!("unknown method: {other}");
            std::process::exit(64);
        }
    }
}

fn print_greeting(who: Option<String>) {
    let who = who.unwrap_or_else(|| "anonymous".to_string());
    println!("hello, {who}!");
}
