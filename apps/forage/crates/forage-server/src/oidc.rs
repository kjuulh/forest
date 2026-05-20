use forage_core::auth::{AuthError, OidcExchange, OidcIdentity};
use reqwest::header;
use serde::Deserialize;

/// Real Google OIDC exchange implementation using reqwest.
pub struct GoogleOidcExchange {
    client_id: String,
    client_secret: String,
    http: reqwest::Client,
}

impl GoogleOidcExchange {
    pub fn new(client_id: String, client_secret: String, _redirect_host: String) -> Self {
        Self {
            client_id,
            client_secret,
            http: reqwest::Client::new(),
        }
    }
}

#[derive(Deserialize)]
struct GoogleTokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct GoogleUserInfo {
    sub: String,
    email: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    picture: Option<String>,
}

#[async_trait::async_trait]
impl OidcExchange for GoogleOidcExchange {
    async fn exchange_code(
        &self,
        code: &str,
        redirect_uri: &str,
    ) -> Result<OidcIdentity, AuthError> {
        // Step 1: Exchange authorization code for tokens.
        let token_resp = self
            .http
            .post("https://oauth2.googleapis.com/token")
            .form(&[
                ("code", code),
                ("client_id", &self.client_id),
                ("client_secret", &self.client_secret),
                ("redirect_uri", redirect_uri),
                ("grant_type", "authorization_code"),
            ])
            .send()
            .await
            .map_err(|e| AuthError::Other(format!("Google token exchange failed: {e}")))?;

        if !token_resp.status().is_success() {
            let status = token_resp.status();
            let body = token_resp.text().await.unwrap_or_default();
            return Err(AuthError::Other(format!(
                "Google token exchange returned {status}: {body}"
            )));
        }

        let token_data: GoogleTokenResponse = token_resp
            .json()
            .await
            .map_err(|e| AuthError::Other(format!("failed to parse Google token response: {e}")))?;

        // Step 2: Fetch user info.
        let userinfo_resp = self
            .http
            .get("https://www.googleapis.com/oauth2/v3/userinfo")
            .bearer_auth(&token_data.access_token)
            .send()
            .await
            .map_err(|e| AuthError::Other(format!("Google userinfo request failed: {e}")))?;

        if !userinfo_resp.status().is_success() {
            let status = userinfo_resp.status();
            return Err(AuthError::Other(format!(
                "Google userinfo returned {status}"
            )));
        }

        let userinfo: GoogleUserInfo = userinfo_resp
            .json()
            .await
            .map_err(|e| AuthError::Other(format!("failed to parse Google userinfo: {e}")))?;

        Ok(OidcIdentity {
            sub: userinfo.sub,
            email: userinfo.email,
            name: userinfo.name,
            picture_url: userinfo.picture,
            login: None,
        })
    }
}

// ─── GitHub ─────────────────────────────────────────────────────────

pub struct GitHubOidcExchange {
    client_id: String,
    client_secret: String,
    http: reqwest::Client,
}

impl GitHubOidcExchange {
    pub fn new(client_id: String, client_secret: String) -> Self {
        Self {
            client_id,
            client_secret,
            http: reqwest::Client::new(),
        }
    }
}

#[derive(Deserialize)]
struct GitHubTokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct GitHubUser {
    id: u64,
    #[serde(default)]
    login: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    avatar_url: Option<String>,
}

#[derive(Deserialize)]
struct GitHubEmail {
    email: String,
    primary: bool,
    verified: bool,
}

#[async_trait::async_trait]
impl OidcExchange for GitHubOidcExchange {
    async fn exchange_code(
        &self,
        code: &str,
        _redirect_uri: &str,
    ) -> Result<OidcIdentity, AuthError> {
        // Step 1: Exchange code for access token.
        let token_resp = self
            .http
            .post("https://github.com/login/oauth/access_token")
            .header(header::ACCEPT, "application/json")
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("code", code),
            ])
            .send()
            .await
            .map_err(|e| AuthError::Other(format!("GitHub token exchange failed: {e}")))?;

        if !token_resp.status().is_success() {
            let status = token_resp.status();
            let body = token_resp.text().await.unwrap_or_default();
            return Err(AuthError::Other(format!(
                "GitHub token exchange returned {status}: {body}"
            )));
        }

        let token_data: GitHubTokenResponse = token_resp
            .json()
            .await
            .map_err(|e| AuthError::Other(format!("failed to parse GitHub token response: {e}")))?;

        // Step 2: Fetch user profile.
        let user: GitHubUser = self
            .http
            .get("https://api.github.com/user")
            .bearer_auth(&token_data.access_token)
            .header(header::USER_AGENT, "forage")
            .send()
            .await
            .map_err(|e| AuthError::Other(format!("GitHub user request failed: {e}")))?
            .json()
            .await
            .map_err(|e| AuthError::Other(format!("failed to parse GitHub user: {e}")))?;

        // Step 3: Get verified primary email (may not be in user profile).
        let email = if let Some(ref e) = user.email {
            e.clone()
        } else {
            let emails: Vec<GitHubEmail> = self
                .http
                .get("https://api.github.com/user/emails")
                .bearer_auth(&token_data.access_token)
                .header(header::USER_AGENT, "forage")
                .send()
                .await
                .map_err(|e| AuthError::Other(format!("GitHub emails request failed: {e}")))?
                .json()
                .await
                .map_err(|e| AuthError::Other(format!("failed to parse GitHub emails: {e}")))?;

            emails
                .into_iter()
                .find(|e| e.primary && e.verified)
                .map(|e| e.email)
                .ok_or_else(|| AuthError::Other("no verified primary email on GitHub account".into()))?
        };

        let login = user.login.clone();
        Ok(OidcIdentity {
            sub: user.id.to_string(),
            email,
            name: user.name.unwrap_or_else(|| login.clone()),
            picture_url: user.avatar_url,
            login: Some(login),
        })
    }
}
