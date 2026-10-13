//! `forest state` — reach a project's terraform@1 state outside a release
//! (understory-io/forest#282).
//!
//! Every subcommand requires the **Admin** role in the project's organisation
//! — reads included, since a state carries secrets — and is recorded
//! server-side, naming you and the state.

use anyhow::Context;
use forest_grpc_interface::TerraformStateRef;

use crate::{grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct StateCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Print TF_HTTP_* exports for a short-lived backend credential, so plain
    /// terraform/tofu commands (`state list`, `plan`, `import`, `state rm`,
    /// `force-unlock`) run against the real state. Read-only unless --write.
    ///
    ///   eval "$(forest state env -e dev)"
    ///   terraform init -reconfigure && terraform state list
    ///
    /// The backend address is forest's internal one; it resolves only where
    /// forest's network does. Elsewhere, use `pull`, `list` and `versions`.
    Env(EnvCommand),
    /// Print the state document (latest, or --version N) to stdout. It
    /// contains secrets in plaintext.
    Pull(PullCommand),
    /// List the resource addresses in the latest state.
    List(ListCommand),
    /// List stored versions (one per apply), newest first, and the lock holder.
    Versions(VersionsCommand),
    /// Break the state's lock, whoever holds it — for an apply that died.
    Unlock(UnlockCommand),
}

impl StateCommand {
    pub fn is_mutation(&self) -> bool {
        matches!(self.commands, Commands::Unlock(_))
    }

    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Env(cmd) => cmd.execute(state).await,
            Commands::Pull(cmd) => cmd.execute(state).await,
            Commands::List(cmd) => cmd.execute(state).await,
            Commands::Versions(cmd) => cmd.execute(state).await,
            Commands::Unlock(cmd) => cmd.execute(state).await,
        }
    }
}

/// Which state: an environment, and the project and organisation from
/// forest.cue unless given.
#[derive(clap::Args)]
struct Target {
    #[arg(long, short = 'e')]
    environment: String,

    /// Organisation. Auto-detected from forest.cue if not specified.
    #[arg(long, short = 'o', visible_alias = "org")]
    organisation: Option<String>,

    /// Project. Auto-detected from forest.cue if not specified.
    #[arg(long, short = 'p')]
    project: Option<String>,

    /// The terraform destination. Needed only when the environment has more
    /// than one and the state is not a pre-#285 one.
    #[arg(long, short = 'd')]
    destination: Option<String>,
}

impl Target {
    async fn resolve(&self, state: &State) -> anyhow::Result<TerraformStateRef> {
        let (detected_org, detected_project) = crate::cli::release::detect::project(state).await;
        Ok(TerraformStateRef {
            organisation: self.organisation.clone().or(detected_org).context(
                "organisation not found: set project.organisation in forest.cue or pass --organisation",
            )?,
            project: self.project.clone().or(detected_project).context(
                "project not found: set project.name in forest.cue or pass --project",
            )?,
            environment: self.environment.clone(),
            destination: self.destination.clone().unwrap_or_default(),
        })
    }
}

#[derive(clap::Parser)]
pub struct EnvCommand {
    #[command(flatten)]
    target: Target,

    /// A read-write credential: lock, write and unlock as well as read.
    /// Needed for plan (which locks), import, state rm/mv and force-unlock.
    /// Read-only commands can use the default with `-lock=false`.
    #[arg(long)]
    write: bool,

    /// Lifetime: seconds, or with a suffix (`30m`, `2h`). Server caps it at 12h.
    #[arg(long, default_value = "1h", value_parser = parse_ttl)]
    ttl: u64,
}

impl EnvCommand {
    async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let target = self.target.resolve(state).await?;
        let cred = state
            .grpc_client()
            .issue_terraform_state_credential(target, self.write, self.ttl)
            .await?;

        // Exports alone on stdout, so `eval "$(...)"` takes exactly these.
        for (key, value) in [
            ("TF_HTTP_ADDRESS", cred.address.as_str()),
            ("TF_HTTP_LOCK_ADDRESS", cred.lock_address.as_str()),
            ("TF_HTTP_UNLOCK_ADDRESS", cred.unlock_address.as_str()),
            // The runner sets these too; terraform's defaults (LOCK/UNLOCK)
            // are not what forest's backend routes.
            ("TF_HTTP_LOCK_METHOD", "POST"),
            ("TF_HTTP_UNLOCK_METHOD", "POST"),
            ("TF_HTTP_USERNAME", cred.username.as_str()),
            ("TF_HTTP_PASSWORD", cred.password.as_str()),
        ] {
            println!("export {key}={}", shell_quote(value));
        }

        let expires = chrono::DateTime::from_timestamp(cred.expires_at_unix, 0)
            .map(|t| t.to_rfc3339())
            .unwrap_or_else(|| cred.expires_at_unix.to_string());
        eprintln!(
            "{} credential for state {} until {expires}",
            if cred.write {
                "read-write"
            } else {
                "read-only"
            },
            cred.state_id
        );
        if !cred.write {
            eprintln!("read-only: pass -lock=false to plan; state rm/mv/import need --write");
        }
        Ok(())
    }
}

#[derive(clap::Parser)]
pub struct PullCommand {
    #[command(flatten)]
    target: Target,

    /// A stored version (from `forest state versions`); latest if omitted.
    #[arg(long, default_value_t = 0)]
    version: i64,
}

impl PullCommand {
    async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let target = self.target.resolve(state).await?;
        let resp = state
            .grpc_client()
            .get_terraform_state(target, self.version)
            .await?;
        eprintln!(
            "{} version {} ({})",
            resp.state_id, resp.version, resp.created_at
        );
        println!("{}", resp.content);
        Ok(())
    }
}

#[derive(clap::Parser)]
pub struct ListCommand {
    #[command(flatten)]
    target: Target,
}

impl ListCommand {
    async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let target = self.target.resolve(state).await?;
        let resp = state.grpc_client().get_terraform_state(target, 0).await?;
        let document: serde_json::Value =
            serde_json::from_str(&resp.content).context("state is not JSON")?;
        for address in resource_addresses(&document) {
            println!("{address}");
        }
        Ok(())
    }
}

#[derive(clap::Parser)]
pub struct VersionsCommand {
    #[command(flatten)]
    target: Target,

    #[arg(long, default_value_t = 20)]
    limit: u32,
}

impl VersionsCommand {
    async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let target = self.target.resolve(state).await?;
        let resp = state
            .grpc_client()
            .list_terraform_state_versions(target, self.limit)
            .await?;
        eprintln!("{}", resp.state_id);
        if resp.lock_id.is_empty() {
            eprintln!("unlocked");
        } else {
            eprintln!("LOCKED by {}", resp.lock_id);
        }
        for v in resp.versions {
            println!("{}\t{}\t{}", v.version, v.created_at, v.size_bytes);
        }
        Ok(())
    }
}

#[derive(clap::Parser)]
pub struct UnlockCommand {
    #[command(flatten)]
    target: Target,
}

impl UnlockCommand {
    async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let target = self.target.resolve(state).await?;
        let resp = state
            .grpc_client()
            .force_unlock_terraform_state(target)
            .await?;
        if resp.removed_lock_id.is_empty() {
            println!("{} was not locked", resp.state_id);
        } else {
            println!("{}: removed lock {}", resp.state_id, resp.removed_lock_id);
        }
        Ok(())
    }
}

/// `module.x.aws_thing.name[0]`-style addresses from a v4 state document.
fn resource_addresses(document: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    let Some(resources) = document.get("resources").and_then(|r| r.as_array()) else {
        return out;
    };
    for resource in resources {
        let module = resource.get("module").and_then(|m| m.as_str());
        let mode = resource
            .get("mode")
            .and_then(|m| m.as_str())
            .unwrap_or("managed");
        let kind = resource.get("type").and_then(|t| t.as_str()).unwrap_or("?");
        let name = resource.get("name").and_then(|n| n.as_str()).unwrap_or("?");

        let mut base = String::new();
        if let Some(module) = module {
            base.push_str(module);
            base.push('.');
        }
        if mode == "data" {
            base.push_str("data.");
        }
        base.push_str(&format!("{kind}.{name}"));

        let instances = resource
            .get("instances")
            .and_then(|i| i.as_array())
            .cloned()
            .unwrap_or_default();
        if instances.is_empty() {
            out.push(base);
            continue;
        }
        for instance in instances {
            match instance.get("index_key") {
                Some(serde_json::Value::Number(n)) => out.push(format!("{base}[{n}]")),
                Some(serde_json::Value::String(k)) => out.push(format!("{base}[\"{k}\"]")),
                _ => out.push(base.clone()),
            }
        }
    }
    out
}

fn parse_ttl(raw: &str) -> Result<u64, String> {
    let raw = raw.trim();
    let (digits, unit) = match raw.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
        Some((i, _)) => raw.split_at(i),
        None => (raw, "s"),
    };
    let n: u64 = digits
        .parse()
        .map_err(|_| format!("'{raw}' is not a duration (e.g. 900, 30m, 2h)"))?;
    let seconds = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        _ => return Err(format!("'{raw}': unit must be s, m or h")),
    };
    if seconds == 0 {
        return Err("ttl must be positive".into());
    }
    Ok(seconds)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ttl_accepts_seconds_minutes_and_hours() {
        assert_eq!(parse_ttl("900"), Ok(900));
        assert_eq!(parse_ttl("30m"), Ok(1800));
        assert_eq!(parse_ttl("2h"), Ok(7200));
        assert!(parse_ttl("0").is_err());
        assert!(parse_ttl("1d").is_err());
        assert!(parse_ttl("soon").is_err());
    }

    #[test]
    fn addresses_follow_terraforms_notation() {
        let document = serde_json::json!({
            "version": 4,
            "resources": [
                {"mode": "managed", "type": "aws_lambda_function", "name": "this",
                 "instances": [{}]},
                {"mode": "managed", "type": "aws_sqs_queue", "name": "queue",
                 "instances": [{"index_key": 0}]},
                {"mode": "managed", "type": "aws_sns_topic_subscription", "name": "queue",
                 "instances": [{"index_key": "0"}, {"index_key": "1"}]},
                {"module": "module.otel", "mode": "data", "type": "aws_secretsmanager_secret",
                 "name": "key", "instances": [{}]},
            ]
        });
        assert_eq!(
            resource_addresses(&document),
            vec![
                "aws_lambda_function.this",
                "aws_sqs_queue.queue[0]",
                "aws_sns_topic_subscription.queue[\"0\"]",
                "aws_sns_topic_subscription.queue[\"1\"]",
                "module.otel.data.aws_secretsmanager_secret.key",
            ]
        );
    }

    #[test]
    fn exported_values_survive_the_shell() {
        assert_eq!(shell_quote("a'b"), r"'a'\''b'");
    }
}
