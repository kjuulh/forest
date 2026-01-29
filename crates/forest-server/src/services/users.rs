use uuid::Uuid;

use crate::{
    State,
    native_credentials::{NativeCredentials, NativeCredentialsState},
    repositories::users::{UserRepository, UserRepositoryState},
};

pub struct UserService {
    repo: UserRepository,
    native_credentials: NativeCredentials,
}

impl UserService {
    fn db(&self) -> &sqlx::PgPool {
        self.repo.pool()
    }

    // ── Authentication ───────────────────────────────────────────────

    pub async fn register(
        &self,
        username: &str,
        email: &str,
        password: &str,
    ) -> anyhow::Result<RegisteredUser> {
        self.native_credentials
            .password_fulfills_requirements(password)
            .map_err(|errs| {
                anyhow::anyhow!(
                    "password didn't fulfill requirements: {}",
                    errs.into_iter()
                        .map(|e| e.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
        let password_hash = self.native_credentials.hash(password)?;

        let mut tx = self.repo.begin().await?;

        let user_id = Uuid::now_v7();
        let user = self
            .repo
            .create_user(tx.as_executor(), user_id, username)
            .await?;
        self.repo
            .add_user_email(tx.as_executor(), user_id, email)
            .await?;

        let credential_id = Uuid::now_v7();
        self.repo
            .set_native_credential(tx.as_executor(), credential_id, user_id, &password_hash)
            .await?;

        let identity_id = Uuid::now_v7();
        self.repo
            .create_identity(
                tx.as_executor(),
                identity_id,
                user_id,
                "native",
                &user_id.to_string(),
                Some(email),
                None,
            )
            .await?;

        tx.commit().await?;

        Ok(RegisteredUser {
            user_id: user.id,
            username: user.username,
            email: email.to_string(),
            created_at: user.created_at,
        })
    }

    pub async fn login_by_username(
        &self,
        username: &str,
        password: &str,
    ) -> anyhow::Result<Option<AuthenticatedUser>> {
        let user = self.repo.get_user_by_username(self.db(), username).await?;
        match user {
            Some(u) => self.verify_native_login(u.id, u.username, password).await,
            None => Ok(None),
        }
    }

    pub async fn login_by_email(
        &self,
        email: &str,
        password: &str,
    ) -> anyhow::Result<Option<AuthenticatedUser>> {
        let user = self.repo.get_user_by_email(self.db(), email).await?;
        match user {
            Some(u) => self.verify_native_login(u.id, u.username, password).await,
            None => Ok(None),
        }
    }

    async fn verify_native_login(
        &self,
        user_id: Uuid,
        username: String,
        password: &str,
    ) -> anyhow::Result<Option<AuthenticatedUser>> {
        let credential = self.repo.get_native_credential(self.db(), user_id).await?;
        let Some(credential) = credential else {
            return Ok(None);
        };

        if let Err(e) = self
            .native_credentials
            .verify(password, &credential.password_hash)
        {
            tracing::warn!("invalid credentials: {e:#}");
            return Ok(None);
        }

        Ok(Some(AuthenticatedUser { user_id, username }))
    }

    pub async fn create_session(
        &self,
        user_id: Uuid,
        token_hash: &[u8],
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> anyhow::Result<CreatedSession> {
        let session_id = Uuid::now_v7();
        let session = self
            .repo
            .create_session(self.db(), session_id, user_id, token_hash, None, expires_at)
            .await?;

        Ok(CreatedSession {
            session_id: session.id,
            user_id: session.user_id,
            expires_at: session.expires_at,
        })
    }

    pub async fn validate_session(&self, token_hash: &[u8]) -> anyhow::Result<Option<Uuid>> {
        let session = self
            .repo
            .get_session_by_token_hash(self.db(), token_hash)
            .await?;
        let Some(session) = session else {
            return Ok(None);
        };

        if let Some(expires_at) = session.expires_at
            && expires_at < chrono::Utc::now()
        {
            return Ok(None);
        }

        Ok(Some(session.user_id))
    }

    pub async fn logout(&self, session_id: Uuid) -> anyhow::Result<()> {
        self.repo.revoke_session(self.db(), session_id).await
    }

    pub async fn logout_all(&self, user_id: Uuid) -> anyhow::Result<()> {
        self.repo.revoke_all_user_sessions(self.db(), user_id).await
    }

    // ── User CRUD ────────────────────────────────────────────────────

    pub async fn get_user(&self, user_id: Uuid) -> anyhow::Result<Option<UserProfile>> {
        let user = self.repo.get_user_by_id(self.db(), user_id).await?;
        let Some(user) = user else {
            return Ok(None);
        };

        let emails = self.repo.get_user_emails(self.db(), user_id).await?;
        let identities = self.repo.get_identities_by_user(self.db(), user_id).await?;

        Ok(Some(UserProfile {
            user_id: user.id,
            username: user.username,
            emails: emails
                .into_iter()
                .map(|e| UserEmail {
                    email: e.email,
                    verified: e.verified,
                })
                .collect(),
            oauth_connections: identities
                .into_iter()
                .filter(|i| i.provider != "native")
                .map(|i| UserOAuthConnection {
                    provider: i.provider,
                    provider_user_id: i.provider_user_id,
                    provider_email: i.provider_email,
                    linked_at: i.created_at,
                })
                .collect(),
            created_at: user.created_at,
            updated_at: user.updated_at,
        }))
    }

    pub async fn get_user_by_username(
        &self,
        username: &str,
    ) -> anyhow::Result<Option<UserProfile>> {
        let user = self.repo.get_user_by_username(self.db(), username).await?;
        match user {
            Some(u) => self.get_user(u.id).await,
            None => Ok(None),
        }
    }

    pub async fn get_user_by_email(&self, email: &str) -> anyhow::Result<Option<UserProfile>> {
        let user = self.repo.get_user_by_email(self.db(), email).await?;
        match user {
            Some(u) => self.get_user(u.id).await,
            None => Ok(None),
        }
    }

    pub async fn update_username(&self, user_id: Uuid, username: &str) -> anyhow::Result<()> {
        self.repo
            .update_user_username(self.db(), user_id, username)
            .await?;
        Ok(())
    }

    pub async fn delete_user(&self, user_id: Uuid) -> anyhow::Result<()> {
        let mut tx = self.repo.begin().await?;

        self.repo
            .revoke_all_user_sessions(tx.as_executor(), user_id)
            .await?;
        self.repo.delete_user(tx.as_executor(), user_id).await?;

        tx.commit().await?;
        Ok(())
    }

    pub async fn list_users(&self, page_size: i64, offset: i64) -> anyhow::Result<UserList> {
        let users = self.repo.list_users(self.db(), page_size, offset).await?;
        let total = self.repo.count_users(self.db()).await?;

        Ok(UserList {
            users: users
                .into_iter()
                .map(|u| UserSummary {
                    user_id: u.id,
                    username: u.username,
                    created_at: u.created_at,
                })
                .collect(),
            total_count: total,
        })
    }

    // ── Password management ──────────────────────────────────────────

    pub async fn change_password(
        &self,
        user_id: Uuid,
        current_password: &str,
        new_password: &str,
    ) -> anyhow::Result<()> {
        let credential = self
            .repo
            .get_native_credential(self.db(), user_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no native credentials found for user"))?;

        self.native_credentials
            .verify(current_password, &credential.password_hash)
            .map_err(|_| anyhow::anyhow!("current password is incorrect"))?;

        self.native_credentials
            .password_fulfills_requirements(new_password)
            .map_err(|errs| {
                anyhow::anyhow!(
                    "password didn't fulfill requirements: {}",
                    errs.into_iter()
                        .map(|e| e.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
        let password_hash = self.native_credentials.hash(new_password)?;

        let mut tx = self.repo.begin().await?;

        let credential_id = Uuid::now_v7();
        self.repo
            .set_native_credential(tx.as_executor(), credential_id, user_id, &password_hash)
            .await?;
        self.repo
            .revoke_all_user_sessions(tx.as_executor(), user_id)
            .await?;

        tx.commit().await?;
        Ok(())
    }

    // ── Email management ─────────────────────────────────────────────

    pub async fn add_email(&self, user_id: Uuid, email: &str) -> anyhow::Result<()> {
        self.repo.add_user_email(self.db(), user_id, email).await?;
        Ok(())
    }

    pub async fn verify_email(&self, user_id: Uuid, email: &str) -> anyhow::Result<()> {
        self.repo.verify_user_email(self.db(), user_id, email).await
    }

    pub async fn remove_email(&self, user_id: Uuid, email: &str) -> anyhow::Result<()> {
        self.repo.delete_user_email(self.db(), user_id, email).await
    }

    // ── OAuth / identity linking ─────────────────────────────────────

    pub async fn link_oauth_provider(
        &self,
        user_id: Uuid,
        provider: &str,
        provider_user_id: &str,
        provider_email: Option<&str>,
        provider_data: Option<&serde_json::Value>,
    ) -> anyhow::Result<()> {
        let identity_id = Uuid::now_v7();
        self.repo
            .create_identity(
                self.db(),
                identity_id,
                user_id,
                provider,
                provider_user_id,
                provider_email,
                provider_data,
            )
            .await?;
        Ok(())
    }

    pub async fn unlink_oauth_provider(&self, user_id: Uuid, provider: &str) -> anyhow::Result<()> {
        self.repo
            .delete_identity_by_provider(self.db(), user_id, provider)
            .await
    }

    pub async fn find_user_by_oauth(
        &self,
        provider: &str,
        provider_user_id: &str,
    ) -> anyhow::Result<Option<Uuid>> {
        let identity = self
            .repo
            .get_identity_by_provider(self.db(), provider, provider_user_id)
            .await?;
        Ok(identity.map(|i| i.user_id))
    }

    // ── OAuth state ──────────────────────────────────────────────────

    pub async fn create_oauth_state(
        &self,
        provider: &str,
        state: &str,
        redirect_uri: Option<&str>,
        data: &serde_json::Value,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> anyhow::Result<Uuid> {
        let id = Uuid::now_v7();
        self.repo
            .create_oauth_state(
                self.db(),
                id,
                provider,
                state,
                redirect_uri,
                data,
                expires_at,
            )
            .await?;
        Ok(id)
    }

    pub async fn consume_oauth_state(&self, state: &str) -> anyhow::Result<Option<OAuthStateInfo>> {
        let row = self.repo.get_oauth_state_by_state(self.db(), state).await?;
        let Some(row) = row else {
            return Ok(None);
        };

        if let Some(expires_at) = row.expires_at
            && expires_at < chrono::Utc::now()
        {
            self.repo.delete_oauth_state(self.db(), row.id).await?;
            return Ok(None);
        }

        self.repo.delete_oauth_state(self.db(), row.id).await?;

        Ok(Some(OAuthStateInfo {
            provider: row.provider,
            redirect_uri: row.redirect_uri,
            data: row.data,
        }))
    }

    // ── Personal access tokens ───────────────────────────────────────

    pub async fn create_personal_access_token(
        &self,
        user_id: Uuid,
        name: &str,
        token_hash: &[u8],
        scopes: &serde_json::Value,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> anyhow::Result<Uuid> {
        let id = Uuid::now_v7();
        self.repo
            .create_personal_access_token(
                self.db(),
                id,
                user_id,
                name,
                token_hash,
                scopes,
                expires_at,
            )
            .await?;
        Ok(id)
    }

    pub async fn validate_personal_access_token(
        &self,
        user_id: Uuid,
        token_hash: &[u8],
    ) -> anyhow::Result<Option<Uuid>> {
        let pat = self
            .repo
            .get_personal_access_token_by_hash(self.db(), user_id, token_hash)
            .await?;
        let Some(pat) = pat else {
            return Ok(None);
        };

        if let Some(expires_at) = pat.expires_at
            && expires_at < chrono::Utc::now()
        {
            return Ok(None);
        }

        self.repo
            .touch_personal_access_token(self.db(), pat.id)
            .await?;
        Ok(Some(pat.id))
    }

    pub async fn list_personal_access_tokens(
        &self,
        user_id: Uuid,
    ) -> anyhow::Result<Vec<PersonalAccessTokenInfo>> {
        let tokens = self
            .repo
            .list_personal_access_tokens(self.db(), user_id)
            .await?;
        Ok(tokens
            .into_iter()
            .map(|t| PersonalAccessTokenInfo {
                id: t.id,
                name: t.name,
                scopes: t.scopes,
                expires_at: t.expires_at,
                last_used: t.last_used,
                created_at: t.created_at,
            })
            .collect())
    }

    pub async fn delete_personal_access_token(&self, token_id: Uuid) -> anyhow::Result<()> {
        self.repo
            .delete_personal_access_token(self.db(), token_id)
            .await
    }

    // ── MFA ──────────────────────────────────────────────────────────

    pub async fn setup_mfa(
        &self,
        user_id: Uuid,
        mfa_type: &str,
        secret: &[u8],
    ) -> anyhow::Result<Uuid> {
        let id = Uuid::now_v7();
        self.repo
            .create_native_mfa(self.db(), id, user_id, mfa_type, secret)
            .await?;
        Ok(id)
    }

    pub async fn verify_mfa(&self, mfa_id: Uuid) -> anyhow::Result<()> {
        self.repo.verify_native_mfa(self.db(), mfa_id).await
    }

    pub async fn disable_mfa(&self, user_id: Uuid) -> anyhow::Result<()> {
        self.repo.delete_native_mfa(self.db(), user_id).await
    }
}

// ─── Return types ────────────────────────────────────────────────────

pub struct RegisteredUser {
    pub user_id: Uuid,
    pub username: String,
    pub email: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub struct AuthenticatedUser {
    pub user_id: Uuid,
    pub username: String,
}

pub struct CreatedSession {
    pub session_id: Uuid,
    pub user_id: Uuid,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub struct UserProfile {
    pub user_id: Uuid,
    pub username: String,
    pub emails: Vec<UserEmail>,
    pub oauth_connections: Vec<UserOAuthConnection>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub struct UserEmail {
    pub email: String,
    pub verified: bool,
}

pub struct UserOAuthConnection {
    pub provider: String,
    pub provider_user_id: String,
    pub provider_email: Option<String>,
    pub linked_at: chrono::DateTime<chrono::Utc>,
}

pub struct UserSummary {
    pub user_id: Uuid,
    pub username: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub struct UserList {
    pub users: Vec<UserSummary>,
    pub total_count: i64,
}

pub struct OAuthStateInfo {
    pub provider: String,
    pub redirect_uri: Option<String>,
    pub data: serde_json::Value,
}

pub struct PersonalAccessTokenInfo {
    pub id: Uuid,
    pub name: String,
    pub scopes: serde_json::Value,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_used: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

// ─── State trait ─────────────────────────────────────────────────────

pub trait UserServiceState {
    fn user_service(&self) -> UserService;
}

impl UserServiceState for State {
    fn user_service(&self) -> UserService {
        UserService {
            repo: self.user_repository(),
            native_credentials: self.native_credentials(),
        }
    }
}
