//! End-to-end tests for the OAuth 2.0 authorization-code flow (M2):
//! LookupOAuthClient → CreateOAuthAuthorizationCode → ExchangeOAuthCode.
//!
//! The authorization-server RPCs are service-account-gated (only Forage calls
//! them), so these run against `restricted_fixture()` which configures a
//! service-account key. App management uses a normal user (org admin).

use base64::Engine;
use forest_grpc_interface::*;
use sha2::Digest;
use tonic::metadata::MetadataValue;

use crate::accepttest::fixtures::{
    Fixture, RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY, fixture, restricted_fixture,
};

const SA: &str = RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY;
const REDIRECT: &str = "https://app.example/callback";

fn authed<T>(token: &str, inner: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(inner);
    let val: MetadataValue<_> = format!("Bearer {token}").parse().expect("valid metadata");
    req.metadata_mut().insert("authorization", val);
    req
}

/// App management runs against the default fixture (no email gating, returns
/// tokens). The service-account auth-server RPCs run against the restricted
/// fixture — both share the same database, so apps are visible to both.
async fn registered_user(fixture: &Fixture) -> (String, String) {
    let resp = fixture
        .users()
        .register(RegisterRequest {
            username: format!("oa-user-{}", uuid::Uuid::now_v7()),
            email: format!("oa-{}@test.com", uuid::Uuid::now_v7()),
            password: "TestPassword123!".into(),
        })
        .await
        .expect("register")
        .into_inner();
    let user = resp.user.expect("user");
    let token = resp.tokens.expect("tokens").access_token;
    (user.user_id, token)
}

async fn create_org(fixture: &Fixture, token: &str) -> String {
    let name = format!("oa-org-{}", uuid::Uuid::now_v7());
    fixture
        .organisations()
        .create_organisation(authed(
            token,
            CreateOrganisationRequest { name: name.clone() },
        ))
        .await
        .expect("create org");
    let id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM organisations WHERE name = $1")
        .bind(&name)
        .fetch_one(&fixture.db)
        .await
        .expect("org id");
    id.to_string()
}

/// Create an app and return (client_id, client_secret).
async fn create_app(fixture: &Fixture, token: &str, org_id: &str) -> (String, String) {
    let resp = fixture
        .oauth_apps()
        .create_o_auth_app(authed(
            token,
            CreateOAuthAppRequest {
                organisation_id: org_id.into(),
                name: "Flow App".into(),
                description: String::new(),
                homepage_url: String::new(),
                redirect_uris: vec![REDIRECT.into()],
                scopes: vec!["profile".into(), "email".into()],
            },
        ))
        .await
        .expect("create app")
        .into_inner();
    let app = resp.app.expect("app");
    (app.client_id, resp.client_secret)
}

async fn mint_code(sa: &Fixture, client_id: &str, user_id: &str, scopes: Vec<String>) -> String {
    sa.oauth_apps()
        .create_o_auth_authorization_code(authed(
            SA,
            CreateOAuthAuthorizationCodeRequest {
                client_id: client_id.into(),
                user_id: user_id.into(),
                redirect_uri: REDIRECT.into(),
                scopes,
                code_challenge: String::new(),
                code_challenge_method: String::new(),
                nonce: String::new(),
            },
        ))
        .await
        .expect("mint code")
        .into_inner()
        .code
}

#[tokio::test(flavor = "multi_thread")]
async fn full_authorization_code_flow_issues_tokens() {
    let app_fx = fixture().await.unwrap();
    let fixture = restricted_fixture().await.unwrap();
    let (user_id, token) = registered_user(&app_fx).await;
    let org_id = create_org(&app_fx, &token).await;
    let (client_id, client_secret) = create_app(&app_fx, &token, &org_id).await;

    // Lookup returns public metadata.
    let lookup = fixture
        .oauth_apps()
        .lookup_o_auth_client(authed(
            SA,
            LookupOAuthClientRequest {
                client_id: client_id.clone(),
            },
        ))
        .await
        .expect("lookup")
        .into_inner();
    assert_eq!(lookup.name, "Flow App");
    assert_eq!(lookup.redirect_uris, vec![REDIRECT]);
    assert_eq!(lookup.scopes, vec!["profile", "email"]);

    // Mint a code, then exchange it for tokens.
    let code = mint_code(&fixture, &client_id, &user_id, vec!["profile".into()]).await;
    let tokens = fixture
        .oauth_apps()
        .exchange_o_auth_code(authed(
            SA,
            ExchangeOAuthCodeRequest {
                client_id: client_id.clone(),
                client_secret: client_secret.clone(),
                code: code.clone(),
                redirect_uri: REDIRECT.into(),
                code_verifier: String::new(),
            },
        ))
        .await
        .expect("exchange")
        .into_inner()
        .tokens
        .expect("tokens");
    assert!(tokens.access_token.starts_with("forest_oat_"));
    assert!(tokens.refresh_token.starts_with("forest_ort_"));
    assert_eq!(tokens.token_type, "bearer");
    assert_eq!(tokens.expires_in_seconds, 8 * 3600);
    assert_eq!(tokens.scopes, vec!["profile"]);

    // The access token is persisted (hashed) for the consenting user.
    let token_hash = sha2::Sha256::digest(tokens.access_token.as_bytes()).to_vec();
    let stored_user: uuid::Uuid =
        sqlx::query_scalar("SELECT user_id FROM oauth_access_tokens WHERE token_hash = $1")
            .bind(&token_hash)
            .fetch_one(&fixture.db)
            .await
            .expect("token row");
    assert_eq!(stored_user.to_string(), user_id);

    // Re-using the same code is rejected (single-use).
    let replay = fixture
        .oauth_apps()
        .exchange_o_auth_code(authed(
            SA,
            ExchangeOAuthCodeRequest {
                client_id: client_id.clone(),
                client_secret: client_secret.clone(),
                code,
                redirect_uri: REDIRECT.into(),
                code_verifier: String::new(),
            },
        ))
        .await
        .expect_err("replay rejected");
    assert_eq!(replay.code(), tonic::Code::FailedPrecondition);
    assert_eq!(replay.message(), "invalid_grant");
}

#[tokio::test(flavor = "multi_thread")]
async fn userinfo_returns_claims_gated_by_scope() {
    let app_fx = fixture().await.unwrap();
    let fixture = restricted_fixture().await.unwrap();
    let (user_id, token) = registered_user(&app_fx).await;
    let org_id = create_org(&app_fx, &token).await;
    let (client_id, client_secret) = create_app(&app_fx, &token, &org_id).await;

    // Grant only `profile` — email claims must be withheld.
    let code = mint_code(&fixture, &client_id, &user_id, vec!["profile".into()]).await;
    let tokens = fixture
        .oauth_apps()
        .exchange_o_auth_code(authed(
            SA,
            ExchangeOAuthCodeRequest {
                client_id: client_id.clone(),
                client_secret: client_secret.clone(),
                code,
                redirect_uri: REDIRECT.into(),
                code_verifier: String::new(),
            },
        ))
        .await
        .expect("exchange")
        .into_inner()
        .tokens
        .expect("tokens");

    let info = fixture
        .oauth_apps()
        .get_o_auth_userinfo(authed(
            SA,
            GetOAuthUserinfoRequest {
                access_token: tokens.access_token.clone(),
            },
        ))
        .await
        .expect("userinfo")
        .into_inner()
        .userinfo
        .expect("userinfo");
    assert_eq!(info.sub, user_id);
    assert!(
        !info.username.is_empty(),
        "profile scope → username present"
    );
    assert!(info.email.is_empty(), "email scope NOT granted → no email");
    assert!(info.emails.is_empty());
    assert_eq!(info.scopes, vec!["profile"]);

    // A bogus access token is rejected.
    let err = fixture
        .oauth_apps()
        .get_o_auth_userinfo(authed(
            SA,
            GetOAuthUserinfoRequest {
                access_token: "forest_oat_bogus".into(),
            },
        ))
        .await
        .expect_err("bad token");
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
}

async fn exchange(sa: &Fixture, client_id: &str, client_secret: &str, code: &str) -> OAuthTokens {
    sa.oauth_apps()
        .exchange_o_auth_code(authed(
            SA,
            ExchangeOAuthCodeRequest {
                client_id: client_id.into(),
                client_secret: client_secret.into(),
                code: code.into(),
                redirect_uri: REDIRECT.into(),
                code_verifier: String::new(),
            },
        ))
        .await
        .expect("exchange")
        .into_inner()
        .tokens
        .expect("tokens")
}

#[tokio::test(flavor = "multi_thread")]
async fn refresh_rotates_and_detects_reuse() {
    let app_fx = fixture().await.unwrap();
    let fixture = restricted_fixture().await.unwrap();
    let (user_id, token) = registered_user(&app_fx).await;
    let org_id = create_org(&app_fx, &token).await;
    let (client_id, client_secret) = create_app(&app_fx, &token, &org_id).await;

    let code = mint_code(&fixture, &client_id, &user_id, vec!["profile".into()]).await;
    let first = exchange(&fixture, &client_id, &client_secret, &code).await;

    // Refresh rotates: new access + new refresh token.
    let refreshed = fixture
        .oauth_apps()
        .refresh_o_auth_token(authed(
            SA,
            RefreshOAuthTokenRequest {
                client_id: client_id.clone(),
                client_secret: client_secret.clone(),
                refresh_token: first.refresh_token.clone(),
            },
        ))
        .await
        .expect("refresh")
        .into_inner()
        .tokens
        .expect("tokens");
    assert!(refreshed.access_token.starts_with("forest_oat_"));
    assert_ne!(refreshed.refresh_token, first.refresh_token);

    // The new access token resolves via userinfo.
    let info = fixture
        .oauth_apps()
        .get_o_auth_userinfo(authed(
            SA,
            GetOAuthUserinfoRequest {
                access_token: refreshed.access_token.clone(),
            },
        ))
        .await
        .expect("userinfo")
        .into_inner()
        .userinfo
        .expect("userinfo");
    assert_eq!(info.sub, user_id);

    // Reusing the OLD (rotated) refresh token is rejected AND kills the family:
    // the new refresh token is now also invalid.
    let reuse = fixture
        .oauth_apps()
        .refresh_o_auth_token(authed(
            SA,
            RefreshOAuthTokenRequest {
                client_id: client_id.clone(),
                client_secret: client_secret.clone(),
                refresh_token: first.refresh_token,
            },
        ))
        .await
        .expect_err("reuse rejected");
    assert_eq!(reuse.code(), tonic::Code::FailedPrecondition);

    let after_family_revoke = fixture
        .oauth_apps()
        .refresh_o_auth_token(authed(
            SA,
            RefreshOAuthTokenRequest {
                client_id,
                client_secret,
                refresh_token: refreshed.refresh_token,
            },
        ))
        .await
        .expect_err("family revoked");
    assert_eq!(after_family_revoke.code(), tonic::Code::FailedPrecondition);
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_refresh_only_one_succeeds() {
    let app_fx = fixture().await.unwrap();
    let fixture = restricted_fixture().await.unwrap();
    let (user_id, token) = registered_user(&app_fx).await;
    let org_id = create_org(&app_fx, &token).await;
    let (client_id, client_secret) = create_app(&app_fx, &token, &org_id).await;

    let code = mint_code(&fixture, &client_id, &user_id, vec!["profile".into()]).await;
    let tokens = exchange(&fixture, &client_id, &client_secret, &code).await;

    // Fire two refreshes with the SAME refresh token concurrently. The atomic
    // single-use consume must let exactly one win.
    let req = |rt: String| {
        let (cid, cs) = (client_id.clone(), client_secret.clone());
        let fx = fixture.clone();
        async move {
            fx.oauth_apps()
                .refresh_o_auth_token(authed(
                    SA,
                    RefreshOAuthTokenRequest {
                        client_id: cid,
                        client_secret: cs,
                        refresh_token: rt,
                    },
                ))
                .await
        }
    };
    let (a, b) = tokio::join!(req(tokens.refresh_token.clone()), req(tokens.refresh_token));
    let successes = [a.is_ok(), b.is_ok()].iter().filter(|x| **x).count();
    assert_eq!(successes, 1, "exactly one concurrent refresh must succeed");
}

#[tokio::test(flavor = "multi_thread")]
async fn revoke_grant_invalidates_access_token() {
    let app_fx = fixture().await.unwrap();
    let fixture = restricted_fixture().await.unwrap();
    let (user_id, token) = registered_user(&app_fx).await;
    let org_id = create_org(&app_fx, &token).await;
    let (client_id, client_secret) = create_app(&app_fx, &token, &org_id).await;

    let code = mint_code(&fixture, &client_id, &user_id, vec!["profile".into()]).await;
    let tokens = exchange(&fixture, &client_id, &client_secret, &code).await;

    // The app_id for the grant.
    let app_id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM oauth_apps WHERE client_id = $1")
        .bind(&client_id)
        .fetch_one(&fixture.db)
        .await
        .expect("app id");

    let resp = fixture
        .oauth_apps()
        .revoke_o_auth_grant(authed(
            SA,
            RevokeOAuthGrantRequest {
                user_id: user_id.clone(),
                app_id: app_id.to_string(),
            },
        ))
        .await
        .expect("revoke")
        .into_inner();
    assert!(resp.revoked_count >= 1);

    // The access token no longer resolves.
    let err = fixture
        .oauth_apps()
        .get_o_auth_userinfo(authed(
            SA,
            GetOAuthUserinfoRequest {
                access_token: tokens.access_token,
            },
        ))
        .await
        .expect_err("revoked token");
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
}

#[tokio::test(flavor = "multi_thread")]
async fn reaper_prunes_dead_rows_but_keeps_live_token() {
    use forest_server::OAuthAppRepository;

    let app_fx = fixture().await.unwrap();
    let fixture = restricted_fixture().await.unwrap();
    let (user_id, token) = registered_user(&app_fx).await;
    let org_id = create_org(&app_fx, &token).await;
    let (client_id, client_secret) = create_app(&app_fx, &token, &org_id).await;
    let user_uuid = uuid::Uuid::parse_str(&user_id).unwrap();
    let app_uuid: uuid::Uuid = sqlx::query_scalar("SELECT id FROM oauth_apps WHERE client_id = $1")
        .bind(&client_id)
        .fetch_one(&fixture.db)
        .await
        .unwrap();

    // A genuine live token (must survive reaping).
    let code = mint_code(&fixture, &client_id, &user_id, vec!["profile".into()]).await;
    let live = exchange(&fixture, &client_id, &client_secret, &code).await;

    // Inject an already-expired authorization code and an expired access token.
    sqlx::query(
        "INSERT INTO oauth_authorization_codes (code_hash, app_id, user_id, redirect_uri, scopes, expires_at) \
         VALUES ($1, $2, $3, $4, $5, now() - interval '1 hour')",
    )
    .bind(format!("dead-code-{user_id}").into_bytes())
    .bind(app_uuid)
    .bind(user_uuid)
    .bind(REDIRECT)
    .bind(vec!["profile".to_string()])
    .execute(&fixture.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO oauth_access_tokens (token_hash, app_id, user_id, scopes, refresh_hash, expires_at, refresh_expires_at) \
         VALUES ($1, $2, $3, $4, $5, now() - interval '2 hours', now() - interval '1 hour')",
    )
    .bind(format!("dead-token-{user_id}").into_bytes())
    .bind(app_uuid)
    .bind(user_uuid)
    .bind(vec!["profile".to_string()])
    .bind(format!("dead-refresh-{user_id}").into_bytes())
    .execute(&fixture.db)
    .await
    .unwrap();

    // Run the reaper.
    let repo = OAuthAppRepository::new(fixture.db.clone());
    let (codes, tokens) = repo.reap_expired(&fixture.db).await.unwrap();
    assert!(codes >= 1, "expired code reaped");
    assert!(tokens >= 1, "expired token reaped");

    // The live token still resolves — it was not collateral damage.
    let info = fixture
        .oauth_apps()
        .get_o_auth_userinfo(authed(
            SA,
            GetOAuthUserinfoRequest {
                access_token: live.access_token,
            },
        ))
        .await
        .expect("live token survives")
        .into_inner()
        .userinfo
        .expect("userinfo");
    assert_eq!(info.sub, user_id);
}

#[tokio::test(flavor = "multi_thread")]
async fn openid_scope_issues_verifiable_id_token() {
    use hmac::{Hmac, Mac};
    use jwt::VerifyWithKey;
    use sha2::Sha256;
    use std::collections::BTreeMap;

    let app_fx = fixture().await.unwrap();
    let fixture = restricted_fixture().await.unwrap();
    let (user_id, token) = registered_user(&app_fx).await;
    let org_id = create_org(&app_fx, &token).await;

    // App registered with the openid + profile scopes.
    let created = app_fx
        .oauth_apps()
        .create_o_auth_app(authed(
            &token,
            CreateOAuthAppRequest {
                organisation_id: org_id,
                name: "OIDC App".into(),
                description: String::new(),
                homepage_url: String::new(),
                redirect_uris: vec![REDIRECT.into()],
                scopes: vec!["openid".into(), "profile".into()],
            },
        ))
        .await
        .expect("create app")
        .into_inner();
    let app = created.app.expect("app");
    let client_id = app.client_id;
    let client_secret = created.client_secret;

    let code = mint_code(
        &fixture,
        &client_id,
        &user_id,
        vec!["openid".into(), "profile".into()],
    )
    .await;
    let tokens = exchange(&fixture, &client_id, &client_secret, &code).await;

    // The id_token is present and verifies with the client_secret (HS256).
    assert!(!tokens.id_token.is_empty(), "openid → id_token issued");
    let key: Hmac<Sha256> = Hmac::new_from_slice(client_secret.as_bytes()).unwrap();
    let claims: BTreeMap<String, String> = tokens
        .id_token
        .verify_with_key(&key)
        .expect("id_token verifies with client_secret");
    assert_eq!(claims["sub"], user_id);
    assert_eq!(claims["aud"], client_id);
    assert_eq!(claims["iss"], "http://forage.test.invalid"); // restricted fixture web_app_url
    assert!(
        claims.contains_key("preferred_username"),
        "profile scope → username"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn id_token_echoes_oidc_nonce() {
    use hmac::{Hmac, Mac};
    use jwt::VerifyWithKey;
    use sha2::Sha256;
    use std::collections::BTreeMap;

    let app_fx = fixture().await.unwrap();
    let fixture = restricted_fixture().await.unwrap();
    let (user_id, token) = registered_user(&app_fx).await;
    let org_id = create_org(&app_fx, &token).await;
    let created = app_fx
        .oauth_apps()
        .create_o_auth_app(authed(
            &token,
            CreateOAuthAppRequest {
                organisation_id: org_id,
                name: "OIDC Nonce App".into(),
                description: String::new(),
                homepage_url: String::new(),
                redirect_uris: vec![REDIRECT.into()],
                scopes: vec!["openid".into()],
            },
        ))
        .await
        .expect("create app")
        .into_inner();
    let app = created.app.expect("app");
    let client_id = app.client_id;
    let client_secret = created.client_secret;

    // Mint a code carrying an OIDC nonce.
    let code = fixture
        .oauth_apps()
        .create_o_auth_authorization_code(authed(
            SA,
            CreateOAuthAuthorizationCodeRequest {
                client_id: client_id.clone(),
                user_id: user_id.clone(),
                redirect_uri: REDIRECT.into(),
                scopes: vec!["openid".into()],
                code_challenge: String::new(),
                code_challenge_method: String::new(),
                nonce: "n0nce-xyz".into(),
            },
        ))
        .await
        .expect("mint code")
        .into_inner()
        .code;

    let tokens = exchange(&fixture, &client_id, &client_secret, &code).await;
    let key: Hmac<Sha256> = Hmac::new_from_slice(client_secret.as_bytes()).unwrap();
    let claims: BTreeMap<String, String> = tokens.id_token.verify_with_key(&key).unwrap();
    assert_eq!(
        claims["nonce"], "n0nce-xyz",
        "id_token echoes the request nonce"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn consent_is_remembered_and_cleared_on_revoke() {
    let app_fx = fixture().await.unwrap();
    let fixture = restricted_fixture().await.unwrap();
    let (user_id, token) = registered_user(&app_fx).await;
    let org_id = create_org(&app_fx, &token).await;
    let (client_id, _secret) = create_app(&app_fx, &token, &org_id).await;

    let get_consent = |client_id: String, user_id: String| {
        let fx = fixture.clone();
        async move {
            fx.oauth_apps()
                .get_o_auth_consent(authed(SA, GetOAuthConsentRequest { client_id, user_id }))
                .await
                .expect("get consent")
                .into_inner()
                .scopes
        }
    };

    // No consent on record yet.
    assert!(
        get_consent(client_id.clone(), user_id.clone())
            .await
            .is_empty()
    );

    // Approving (minting a code) records consent.
    mint_code(&fixture, &client_id, &user_id, vec!["profile".into()]).await;
    let scopes = get_consent(client_id.clone(), user_id.clone()).await;
    assert_eq!(scopes, vec!["profile"]);

    // A second approval with another scope widens (unions) the consent.
    mint_code(&fixture, &client_id, &user_id, vec!["email".into()]).await;
    let mut scopes = get_consent(client_id.clone(), user_id.clone()).await;
    scopes.sort();
    assert_eq!(scopes, vec!["email", "profile"]);

    // Revoking the grant forgets the consent.
    let app_id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM oauth_apps WHERE client_id = $1")
        .bind(&client_id)
        .fetch_one(&fixture.db)
        .await
        .unwrap();
    fixture
        .oauth_apps()
        .revoke_o_auth_grant(authed(
            SA,
            RevokeOAuthGrantRequest {
                user_id: user_id.clone(),
                app_id: app_id.to_string(),
            },
        ))
        .await
        .expect("revoke");
    assert!(get_consent(client_id, user_id).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn list_grants_reflects_authorize_and_revoke() {
    let app_fx = fixture().await.unwrap();
    let fixture = restricted_fixture().await.unwrap();
    let (user_id, token) = registered_user(&app_fx).await;
    let org_id = create_org(&app_fx, &token).await;
    let (client_id, client_secret) = create_app(&app_fx, &token, &org_id).await;

    // No grants yet.
    let grants = fixture
        .oauth_apps()
        .list_o_auth_grants(authed(
            SA,
            ListOAuthGrantsRequest {
                user_id: user_id.clone(),
            },
        ))
        .await
        .expect("list")
        .into_inner()
        .grants;
    assert!(grants.is_empty());

    // Authorize → one grant appears with the granted scopes.
    let code = mint_code(
        &fixture,
        &client_id,
        &user_id,
        vec!["profile".into(), "email".into()],
    )
    .await;
    exchange(&fixture, &client_id, &client_secret, &code).await;

    let grants = fixture
        .oauth_apps()
        .list_o_auth_grants(authed(
            SA,
            ListOAuthGrantsRequest {
                user_id: user_id.clone(),
            },
        ))
        .await
        .expect("list")
        .into_inner()
        .grants;
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].name, "Flow App");
    assert!(grants[0].scopes.contains(&"profile".to_string()));
    assert!(grants[0].scopes.contains(&"email".to_string()));

    // Revoke → grant disappears.
    let app_id = grants[0].app_id.clone();
    fixture
        .oauth_apps()
        .revoke_o_auth_grant(authed(
            SA,
            RevokeOAuthGrantRequest {
                user_id: user_id.clone(),
                app_id,
            },
        ))
        .await
        .expect("revoke");

    let grants = fixture
        .oauth_apps()
        .list_o_auth_grants(authed(SA, ListOAuthGrantsRequest { user_id }))
        .await
        .expect("list")
        .into_inner()
        .grants;
    assert!(grants.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_secret_is_invalid_client_and_wrong_redirect_is_invalid_grant() {
    let app_fx = fixture().await.unwrap();
    let fixture = restricted_fixture().await.unwrap();
    let (user_id, token) = registered_user(&app_fx).await;
    let org_id = create_org(&app_fx, &token).await;
    let (client_id, client_secret) = create_app(&app_fx, &token, &org_id).await;

    // Wrong secret → invalid_client.
    let code = mint_code(&fixture, &client_id, &user_id, vec!["profile".into()]).await;
    let err = fixture
        .oauth_apps()
        .exchange_o_auth_code(authed(
            SA,
            ExchangeOAuthCodeRequest {
                client_id: client_id.clone(),
                client_secret: "forest_oas_wrong".into(),
                code,
                redirect_uri: REDIRECT.into(),
                code_verifier: String::new(),
            },
        ))
        .await
        .expect_err("bad secret");
    assert_eq!(err.code(), tonic::Code::Unauthenticated);
    assert_eq!(err.message(), "invalid_client");

    // Wrong redirect_uri at exchange → invalid_grant (fresh code).
    let code = mint_code(&fixture, &client_id, &user_id, vec!["profile".into()]).await;
    let err = fixture
        .oauth_apps()
        .exchange_o_auth_code(authed(
            SA,
            ExchangeOAuthCodeRequest {
                client_id,
                client_secret,
                code,
                redirect_uri: "https://app.example/other".into(),
                code_verifier: String::new(),
            },
        ))
        .await
        .expect_err("redirect mismatch");
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    assert_eq!(err.message(), "invalid_grant");
}

#[tokio::test(flavor = "multi_thread")]
async fn unregistered_redirect_or_scope_is_rejected_at_code_creation() {
    let app_fx = fixture().await.unwrap();
    let fixture = restricted_fixture().await.unwrap();
    let (user_id, token) = registered_user(&app_fx).await;
    let org_id = create_org(&app_fx, &token).await;
    let (client_id, _secret) = create_app(&app_fx, &token, &org_id).await;

    // Redirect not in the allowlist.
    let err = fixture
        .oauth_apps()
        .create_o_auth_authorization_code(authed(
            SA,
            CreateOAuthAuthorizationCodeRequest {
                client_id: client_id.clone(),
                user_id: user_id.clone(),
                redirect_uri: "https://evil.example/cb".into(),
                scopes: vec!["profile".into()],
                code_challenge: String::new(),
                code_challenge_method: String::new(),
                nonce: String::new(),
            },
        ))
        .await
        .expect_err("bad redirect");
    assert_eq!(err.code(), tonic::Code::InvalidArgument);

    // Scope the app didn't register for.
    let err = fixture
        .oauth_apps()
        .create_o_auth_authorization_code(authed(
            SA,
            CreateOAuthAuthorizationCodeRequest {
                client_id,
                user_id,
                redirect_uri: REDIRECT.into(),
                scopes: vec!["admin".into()],
                code_challenge: String::new(),
                code_challenge_method: String::new(),
                nonce: String::new(),
            },
        ))
        .await
        .expect_err("bad scope");
    assert_eq!(err.code(), tonic::Code::InvalidArgument);
    assert_eq!(err.message(), "invalid_scope");
}

#[tokio::test(flavor = "multi_thread")]
async fn pkce_s256_required_verifier_must_match() {
    let app_fx = fixture().await.unwrap();
    let fixture = restricted_fixture().await.unwrap();
    let (user_id, token) = registered_user(&app_fx).await;
    let org_id = create_org(&app_fx, &token).await;
    let (client_id, client_secret) = create_app(&app_fx, &token, &org_id).await;

    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(sha2::Sha256::digest(verifier.as_bytes()));

    // Mint a code bound to the S256 challenge.
    let mint = |challenge: String| {
        let client_id = client_id.clone();
        let user_id = user_id.clone();
        let fixture = fixture.clone();
        async move {
            fixture
                .oauth_apps()
                .create_o_auth_authorization_code(authed(
                    SA,
                    CreateOAuthAuthorizationCodeRequest {
                        client_id,
                        user_id,
                        redirect_uri: REDIRECT.into(),
                        scopes: vec!["profile".into()],
                        code_challenge: challenge,
                        code_challenge_method: "S256".into(),
                        nonce: String::new(),
                    },
                ))
                .await
                .expect("mint code")
                .into_inner()
                .code
        }
    };

    // Wrong verifier → invalid_grant.
    let code = mint(challenge.clone()).await;
    let err = fixture
        .oauth_apps()
        .exchange_o_auth_code(authed(
            SA,
            ExchangeOAuthCodeRequest {
                client_id: client_id.clone(),
                client_secret: client_secret.clone(),
                code,
                redirect_uri: REDIRECT.into(),
                code_verifier: "wrong-verifier".into(),
            },
        ))
        .await
        .expect_err("pkce mismatch");
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);

    // Correct verifier → success.
    let code = mint(challenge).await;
    let tokens = fixture
        .oauth_apps()
        .exchange_o_auth_code(authed(
            SA,
            ExchangeOAuthCodeRequest {
                client_id,
                client_secret,
                code,
                redirect_uri: REDIRECT.into(),
                code_verifier: verifier.into(),
            },
        ))
        .await
        .expect("pkce ok")
        .into_inner()
        .tokens
        .expect("tokens");
    assert!(tokens.access_token.starts_with("forest_oat_"));
}
