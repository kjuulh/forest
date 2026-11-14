use std::collections::HashSet;
use std::time::Duration;

use base64::Engine;
use forage_core::integrations::baseline::SlackDirectoryUser;
use forage_core::integrations::direct::SlackDirectory;
use rsa::pkcs1v15::SigningKey;
use rsa::signature::{SignatureEncoding, Signer};
use serde::Deserialize;
use sha2::Sha256;

pub const SLACK_API_BASE: &str = "https://slack.com/api";
pub const SLACK_AUTHORIZE_URL: &str = "https://slack.com/oauth/v2/authorize";

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("failed to build reqwest client")
}

#[derive(Clone)]
pub struct SlackApi {
    http: reqwest::Client,
    base: String,
}

impl SlackApi {
    pub fn new(base: impl Into<String>) -> Self {
        Self {
            http: http(),
            base: base.into().trim_end_matches('/').to_string(),
        }
    }

    pub fn url(&self, method: &str) -> String {
        format!("{}/{method}", self.base)
    }
}

impl Default for SlackApi {
    fn default() -> Self {
        Self::new(SLACK_API_BASE)
    }
}

#[derive(Deserialize)]
struct SlackUserObject {
    id: String,
    #[serde(default)]
    team_id: String,
    #[serde(default)]
    deleted: bool,
    #[serde(default)]
    is_bot: bool,
    #[serde(default)]
    is_restricted: bool,
    #[serde(default)]
    is_ultra_restricted: bool,
    #[serde(default)]
    is_stranger: bool,
    #[serde(default)]
    is_email_confirmed: Option<bool>,
    #[serde(default)]
    profile: SlackProfile,
}

#[derive(Deserialize, Default)]
struct SlackProfile {
    #[serde(default)]
    email: Option<String>,
}

#[async_trait::async_trait]
impl SlackDirectory for SlackApi {
    async fn lookup_by_email(
        &self,
        access_token: &str,
        email: &str,
    ) -> Result<Option<SlackDirectoryUser>, String> {
        let resp = self
            .http
            .get(self.url("users.lookupByEmail"))
            .bearer_auth(access_token)
            .query(&[("email", email)])
            .send()
            .await
            .map_err(|e| format!("slack users.lookupByEmail http: {}", e.without_url()))?;
        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err("slack users.lookupByEmail: ratelimited".into());
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|_| "slack users.lookupByEmail: unparseable response".to_string())?;
        if body["ok"].as_bool() != Some(true) {
            let err = body["error"].as_str().unwrap_or("unknown");
            return if err == "users_not_found" {
                Ok(None)
            } else {
                Err(format!("slack users.lookupByEmail: {err}"))
            };
        }
        let user: SlackUserObject = serde_json::from_value(body["user"].clone())
            .map_err(|_| "slack users.lookupByEmail: malformed user".to_string())?;
        Ok(Some(SlackDirectoryUser {
            id: user.id,
            team_id: user.team_id,
            deleted: user.deleted,
            is_bot: user.is_bot,
            is_restricted: user.is_restricted,
            is_ultra_restricted: user.is_ultra_restricted,
            is_stranger: user.is_stranger,
            is_email_confirmed: user.is_email_confirmed,
            email: user.profile.email,
        }))
    }
}

/// A GitHub App's identity. Installation tokens are minted per use and never stored.
pub struct GitHubApp {
    http: reqwest::Client,
    pub app_id: String,
    pub slug: String,
    pub client_id: String,
    client_secret: String,
    pub webhook_secret: String,
    signing_key: SigningKey<Sha256>,
    pub api_base: String,
    pub web_base: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallationInfo {
    pub id: i64,
    pub account_id: i64,
    pub account_login: String,
    pub account_type: String,
    pub suspended: bool,
    pub members_permission: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubUser {
    pub id: i64,
    pub login: String,
}

pub struct GitHubAppConfig {
    pub app_id: String,
    pub slug: String,
    pub client_id: String,
    pub client_secret: String,
    pub webhook_secret: String,
    pub private_key_pem: String,
    pub api_base: String,
    pub web_base: String,
}

impl GitHubApp {
    pub fn new(config: GitHubAppConfig) -> Result<Self, String> {
        use rsa::pkcs1::DecodeRsaPrivateKey;
        use rsa::pkcs8::DecodePrivateKey;
        let pem = config.private_key_pem.replace("\\n", "\n");
        let key = rsa::RsaPrivateKey::from_pkcs1_pem(&pem)
            .or_else(|_| rsa::RsaPrivateKey::from_pkcs8_pem(&pem))
            .map_err(|_| {
                "GITHUB_APP_PRIVATE_KEY is not an RSA private key in PEM form".to_string()
            })?;
        Ok(Self {
            http: http(),
            app_id: config.app_id,
            slug: config.slug,
            client_id: config.client_id,
            client_secret: config.client_secret,
            webhook_secret: config.webhook_secret,
            signing_key: SigningKey::<Sha256>::new(key),
            api_base: config.api_base.trim_end_matches('/').to_string(),
            web_base: config.web_base.trim_end_matches('/').to_string(),
        })
    }

    pub fn install_url(&self, state: &str) -> String {
        format!(
            "{}/apps/{}/installations/new?state={}",
            self.web_base,
            urlencoding::encode(&self.slug),
            urlencoding::encode(state)
        )
    }

    pub fn authorize_url(&self, redirect_uri: &str, state: &str) -> String {
        format!(
            "{}/login/oauth/authorize?client_id={}&redirect_uri={}&state={}",
            self.web_base,
            urlencoding::encode(&self.client_id),
            urlencoding::encode(redirect_uri),
            urlencoding::encode(state)
        )
    }

    fn app_jwt(&self) -> String {
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let now = chrono::Utc::now().timestamp();
        let header = b64.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
        let claims = b64.encode(
            serde_json::json!({ "iat": now - 60, "exp": now + 540, "iss": self.app_id })
                .to_string(),
        );
        let signing_input = format!("{header}.{claims}");
        let signature = self.signing_key.sign(signing_input.as_bytes()).to_bytes();
        format!("{signing_input}.{}", b64.encode(signature))
    }

    fn get(&self, path: &str, token: &str) -> reqwest::RequestBuilder {
        self.http
            .get(format!("{}{path}", self.api_base))
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "Forest")
            .header("X-GitHub-Api-Version", "2022-11-28")
    }

    /// `Ok(None)` when the installation no longer exists (uninstalled).
    pub async fn installation(
        &self,
        installation_id: i64,
    ) -> Result<Option<InstallationInfo>, String> {
        let resp = self
            .get(
                &format!("/app/installations/{installation_id}"),
                &self.app_jwt(),
            )
            .send()
            .await
            .map_err(|e| format!("github installation http: {}", e.without_url()))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(format!("github installation: HTTP {}", resp.status()));
        }
        let v: serde_json::Value = resp
            .json()
            .await
            .map_err(|_| "github installation: unparseable".to_string())?;
        Ok(Some(InstallationInfo {
            id: v["id"].as_i64().unwrap_or_default(),
            account_id: v["account"]["id"].as_i64().unwrap_or_default(),
            account_login: v["account"]["login"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            account_type: v["account"]["type"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            suspended: !v["suspended_at"].is_null(),
            members_permission: v["permissions"]["members"].as_str().map(String::from),
        }))
    }

    pub async fn installation_token(&self, installation_id: i64) -> Result<String, String> {
        let resp = self
            .http
            .post(format!(
                "{}/app/installations/{installation_id}/access_tokens",
                self.api_base
            ))
            .bearer_auth(self.app_jwt())
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "Forest")
            .json(&serde_json::json!({ "permissions": { "members": "read" } }))
            .send()
            .await
            .map_err(|e| format!("github installation token http: {}", e.without_url()))?;
        if !resp.status().is_success() {
            return Err(format!("github installation token: HTTP {}", resp.status()));
        }
        let v: serde_json::Value = resp
            .json()
            .await
            .map_err(|_| "github installation token: unparseable".to_string())?;
        v["token"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| "github installation token: missing token".into())
    }

    /// Exchanges an OAuth code for a short-lived user token used only to prove
    /// who installed the app; the caller must discard it.
    pub async fn exchange_code(&self, code: &str, redirect_uri: &str) -> Result<String, String> {
        let resp = self
            .http
            .post(format!("{}/login/oauth/access_token", self.web_base))
            .header("Accept", "application/json")
            .header("User-Agent", "Forest")
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("code", code),
                ("redirect_uri", redirect_uri),
            ])
            .send()
            .await
            .map_err(|e| format!("github code exchange http: {}", e.without_url()))?;
        let v: serde_json::Value = resp
            .json()
            .await
            .map_err(|_| "github code exchange: unparseable".to_string())?;
        v["access_token"].as_str().map(String::from).ok_or_else(|| {
            format!(
                "github code exchange: {}",
                v["error"].as_str().unwrap_or("no access_token")
            )
        })
    }

    pub async fn user(&self, user_token: &str) -> Result<GitHubUser, String> {
        let v: serde_json::Value = self
            .get("/user", user_token)
            .send()
            .await
            .map_err(|e| format!("github user http: {}", e.without_url()))?
            .error_for_status()
            .map_err(|e| format!("github user: {}", e.without_url()))?
            .json()
            .await
            .map_err(|_| "github user: unparseable".to_string())?;
        Ok(GitHubUser {
            id: v["id"].as_i64().ok_or("github user: missing id")?,
            login: v["login"]
                .as_str()
                .ok_or("github user: missing login")?
                .to_string(),
        })
    }

    /// Installations of this app the user can access (bounded to 500).
    pub async fn user_installation_ids(&self, user_token: &str) -> Result<HashSet<i64>, String> {
        let mut ids = HashSet::new();
        for page in 1..=5 {
            let v: serde_json::Value = self
                .get(
                    &format!("/user/installations?per_page=100&page={page}"),
                    user_token,
                )
                .send()
                .await
                .map_err(|e| format!("github user installations http: {}", e.without_url()))?
                .error_for_status()
                .map_err(|e| format!("github user installations: {}", e.without_url()))?
                .json()
                .await
                .map_err(|_| "github user installations: unparseable".to_string())?;
            let batch = v["installations"].as_array().cloned().unwrap_or_default();
            ids.extend(batch.iter().filter_map(|i| i["id"].as_i64()));
            if batch.len() < 100 {
                break;
            }
        }
        Ok(ids)
    }

    /// `(role, state)` of `login` in `org`, or `None` when not a member.
    pub async fn org_membership(
        &self,
        installation_token: &str,
        org: &str,
        login: &str,
    ) -> Result<Option<(String, String)>, String> {
        let resp = self
            .get(
                &format!(
                    "/orgs/{}/memberships/{}",
                    urlencoding::encode(org),
                    urlencoding::encode(login)
                ),
                installation_token,
            )
            .send()
            .await
            .map_err(|e| format!("github membership http: {}", e.without_url()))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(format!("github membership: HTTP {}", resp.status()));
        }
        let v: serde_json::Value = resp
            .json()
            .await
            .map_err(|_| "github membership: unparseable".to_string())?;
        Ok(Some((
            v["role"].as_str().unwrap_or_default().to_string(),
            v["state"].as_str().unwrap_or_default().to_string(),
        )))
    }

    /// Numeric ids of the org's members (bounded to 2000; `true` if truncated).
    pub async fn org_member_ids(
        &self,
        installation_token: &str,
        org: &str,
    ) -> Result<(HashSet<i64>, bool), String> {
        let mut ids = HashSet::new();
        for page in 1..=20 {
            let v: serde_json::Value = self
                .get(
                    &format!(
                        "/orgs/{}/members?per_page=100&page={page}",
                        urlencoding::encode(org)
                    ),
                    installation_token,
                )
                .send()
                .await
                .map_err(|e| format!("github members http: {}", e.without_url()))?
                .error_for_status()
                .map_err(|e| format!("github members: {}", e.without_url()))?
                .json()
                .await
                .map_err(|_| "github members: unparseable".to_string())?;
            let batch = v.as_array().cloned().unwrap_or_default();
            ids.extend(batch.iter().filter_map(|m| m["id"].as_i64()));
            if batch.len() < 100 {
                return Ok((ids, false));
            }
        }
        Ok((ids, true))
    }
}
