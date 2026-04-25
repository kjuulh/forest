//! `forest-contrib/gitea-create-repo@v0.1` — REST API wrapper for
//! creating a Gitea repository.
//!
//! Posts to `/api/v1/orgs/<org>/repos` (or `/api/v1/user/repos` if no
//! `org` is given). Reads the API token from a file path so the secret
//! never appears on the CLI or in environment listings — workflow
//! authors deliver it via the existing Forest secret channel:
//!
//!   secrets:
//!     - { name: gitea-token, target_path: /run/secrets/gitea-token, ... }
//!   steps:
//!     - uses: forest-contrib/gitea-create-repo@0.1.0
//!       with:
//!         base_url:   https://gitea.example.com
//!         org:        my-org
//!         name:       my-new-repo
//!         token_path: /run/secrets/gitea-token

#[allow(dead_code)]
mod forestgen;

use std::time::Duration;

use forestgen::*;

const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

struct Commands;

impl CommandHandler for Commands {
    async fn gitea_create_repo(
        &self,
        _spec: &Spec,
        input: GiteaCreateRepoInput,
    ) -> Result<GiteaCreateRepoOutput, forest_sdk::Error> {
        // ureq is blocking; offload so we don't block the runtime.
        // run_once already uses tokio::Runtime::new which is fine for
        // spawn_blocking.
        tokio::task::spawn_blocking(move || do_create(input))
            .await
            .map_err(|e| forest_sdk::Error::Handler(format!("join: {e}").into()))?
    }
}

fn do_create(input: GiteaCreateRepoInput) -> Result<GiteaCreateRepoOutput, forest_sdk::Error> {
    let token = std::fs::read_to_string(&input.token_path)
        .map_err(|e| {
            forest_sdk::Error::Handler(
                format!("read token from {}: {e}", input.token_path).into(),
            )
        })?
        .trim()
        .to_string();

    if token.is_empty() {
        return Err(forest_sdk::Error::Handler(
            format!("token file {} is empty", input.token_path).into(),
        ));
    }

    let endpoint = if input.org.is_empty() {
        format!("{}/api/v1/user/repos", input.base_url.trim_end_matches('/'))
    } else {
        format!(
            "{}/api/v1/orgs/{}/repos",
            input.base_url.trim_end_matches('/'),
            input.org
        )
    };

    let body = serde_json::json!({
        "name":           input.name,
        "description":    input.description,
        "private":        input.private,
        "auto_init":      input.auto_init,
        "default_branch": input.default_branch,
    });

    let agent = ureq::AgentBuilder::new()
        .timeout(HTTP_TIMEOUT)
        // Gitea returns the JSON we want on 201 Created and a JSON error
        // body on 4xx/5xx — we want to read both as JSON, so don't let
        // ureq promote 4xx into Err automatically.
        .build();

    let resp = agent
        .post(&endpoint)
        .set("Authorization", &format!("token {token}"))
        .set("Accept", "application/json")
        .set("User-Agent", "forest-gitea-create-repo/0.1")
        .send_json(&body);

    let parsed: serde_json::Value = match resp {
        Ok(r) => r.into_json().map_err(|e| {
            forest_sdk::Error::Handler(format!("parse Gitea response: {e}").into())
        })?,
        Err(ureq::Error::Status(code, r)) => {
            // Surface Gitea's error body verbatim so workflow authors
            // see "repository already exists" / "auth failed" / etc.
            let body = r
                .into_string()
                .unwrap_or_else(|e| format!("(failed to read body: {e})"));
            return Err(forest_sdk::Error::Handler(
                format!("Gitea {endpoint} → {code}: {body}").into(),
            ));
        }
        Err(e) => {
            return Err(forest_sdk::Error::Handler(
                format!("Gitea {endpoint}: {e}").into(),
            ));
        }
    };

    let pick_str = |v: &serde_json::Value, key: &str| -> Result<String, forest_sdk::Error> {
        v.get(key)
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                forest_sdk::Error::Handler(
                    format!("Gitea response missing string field {key:?}").into(),
                )
            })
    };

    Ok(GiteaCreateRepoOutput {
        id: parsed
            .get("id")
            .and_then(|x| x.as_i64())
            .ok_or_else(|| {
                forest_sdk::Error::Handler("Gitea response missing integer field 'id'".into())
            })?,
        clone_url: pick_str(&parsed, "clone_url")?,
        ssh_url: pick_str(&parsed, "ssh_url")?,
        html_url: pick_str(&parsed, "html_url")?,
        full_name: pick_str(&parsed, "full_name")?,
    })
}

fn main() {
    let router = ComponentRouter::new(Commands);
    forest_sdk::run_once(&router);
}
