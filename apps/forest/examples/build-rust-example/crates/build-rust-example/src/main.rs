use forest_sdk::{CallContext, ComponentService, Error, MethodDescriptor, MethodKind};

struct Example;

impl ComponentService<serde_json::Value> for Example {
    fn call(
        &self,
        method: &str,
        _spec: &serde_json::Value,
        _input: serde_json::Value,
        _context: &CallContext,
    ) -> impl std::future::Future<Output = Result<serde_json::Value, Error>> + Send {
        let method = method.to_string();
        async move {
            match method.as_str() {
                "commands/hello" => {
                    Ok(serde_json::json!({ "message": "hello from build-rust-example" }))
                }
                other => Err(Error::MethodNotFound(other.to_string())),
            }
        }
    }

    fn methods(&self) -> Vec<MethodDescriptor> {
        vec![MethodDescriptor {
            name: "commands/hello".to_string(),
            kind: MethodKind::Command,
            description: Some("Print a greeting".to_string()),
        }]
    }
}

fn main() {
    forest_sdk::run_once(&Example);
}
