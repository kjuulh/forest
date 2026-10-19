use forest_grpc_interface::*;
use tonic::metadata::MetadataValue;

use crate::accepttest::fixtures::{
    Fixture, RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY, fixture, restricted_fixture,
};

const SA: &str = RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY;
const DEVICE: &str = "urn:ietf:params:oauth:grant-type:device_code";
const APPLY: &str = "datagateway:apply";
const READ: &str = "datagateway:read";

fn authed<T>(token: &str, inner: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(inner);
    let val: MetadataValue<_> = format!("Bearer {token}").parse().expect("valid metadata");
    req.metadata_mut().insert("authorization", val);
    req
}

async fn registered_user(fixture: &Fixture) -> (String, String) {
    let resp = fixture
        .users()
        .register(RegisterRequest {
            username: format!("dv-user-{}", uuid::Uuid::now_v7()),
            email: format!("dv-{}@test.com", uuid::Uuid::now_v7()),
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
    let name = format!("dv-org-{}", uuid::Uuid::now_v7());
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

fn app_request(org_id: &str, public: bool, grant_types: &[&str]) -> CreateOAuthAppRequest {
    CreateOAuthAppRequest {
        organisation_id: org_id.into(),
        name: "datactl".into(),
        description: String::new(),
        homepage_url: String::new(),
        redirect_uris: vec!["http://127.0.0.1/callback".into()],
        scopes: vec![APPLY.into(), READ.into()],
        grant_types: grant_types.iter().map(|s| s.to_string()).collect(),
        public,
    }
}

async fn device_app(fixture: &Fixture, token: &str, org_id: &str) -> (String, String) {
    let app = fixture
        .oauth_apps()
        .create_o_auth_app(authed(
            token,
            app_request(org_id, true, &["authorization_code", DEVICE]),
        ))
        .await
        .expect("create device app")
        .into_inner()
        .app
        .expect("app");
    assert_eq!(app.grant_types, vec!["authorization_code", DEVICE]);
    (app.app_id, app.client_id)
}

async fn initiate(
    sa: &Fixture,
    client_id: &str,
    scopes: &[&str],
) -> Result<InitiateOAuthDeviceAuthorizationResponse, tonic::Status> {
    sa.oauth_apps()
        .initiate_o_auth_device_authorization(authed(
            SA,
            InitiateOAuthDeviceAuthorizationRequest {
                client_id: client_id.into(),
                client_secret: String::new(),
                scopes: scopes.iter().map(|s| s.to_string()).collect(),
            },
        ))
        .await
        .map(|r| r.into_inner())
}

async fn exchange(
    sa: &Fixture,
    client_id: &str,
    device_code: &str,
) -> Result<ExchangeOAuthDeviceCodeResponse, tonic::Status> {
    sa.oauth_apps()
        .exchange_o_auth_device_code(authed(
            SA,
            ExchangeOAuthDeviceCodeRequest {
                client_id: client_id.into(),
                client_secret: String::new(),
                device_code: device_code.into(),
            },
        ))
        .await
        .map(|r| r.into_inner())
}

async fn approve(sa: &Fixture, user_code: &str, user_id: &str) -> Result<(), tonic::Status> {
    sa.oauth_apps()
        .approve_o_auth_device_authorization(authed(
            SA,
            ApproveOAuthDeviceAuthorizationRequest {
                user_code: user_code.into(),
                user_id: user_id.into(),
                approving_ip: "127.0.0.1".into(),
                approving_user_agent: "test-browser".into(),
            },
        ))
        .await
        .map(|_| ())
}

async fn lookup(
    sa: &Fixture,
    user_code: &str,
) -> Result<LookupOAuthDeviceAuthorizationResponse, tonic::Status> {
    sa.oauth_apps()
        .lookup_o_auth_device_authorization(authed(
            SA,
            LookupOAuthDeviceAuthorizationRequest {
                user_code: user_code.into(),
            },
        ))
        .await
        .map(|r| r.into_inner())
}

async fn introspect(sa: &Fixture, token: &str) -> IntrospectClientTokenResponse {
    sa.oauth_apps()
        .introspect_client_token(authed(
            SA,
            IntrospectClientTokenRequest {
                access_token: token.into(),
            },
        ))
        .await
        .expect("introspect")
        .into_inner()
}

fn status(resp: &ExchangeOAuthDeviceCodeResponse) -> OAuthDeviceTokenStatus {
    OAuthDeviceTokenStatus::try_from(resp.status).expect("known status")
}

#[tokio::test(flavor = "multi_thread")]
async fn an_approved_device_grant_yields_the_apps_scoped_token_once() {
    let app_fx = fixture().await.unwrap();
    let sa = restricted_fixture().await.unwrap();
    let (user_id, token) = registered_user(&app_fx).await;
    let org_id = create_org(&app_fx, &token).await;
    let (app_id, client_id) = device_app(&app_fx, &token, &org_id).await;

    let grant = initiate(&sa, &client_id, &[APPLY, READ])
        .await
        .expect("initiate");
    assert!(grant.verification_uri.ends_with("/device"));
    assert!(
        grant
            .verification_uri_complete
            .contains(&grant.user_code.replace('-', "")),
        "the complete URI pre-fills the user code, and nothing else secret"
    );
    assert!(!grant.verification_uri_complete.contains(&grant.device_code));
    assert_eq!(grant.interval_seconds, 5);
    assert_eq!(grant.expires_in_seconds, 900);

    let pending = exchange(&sa, &client_id, &grant.device_code).await.unwrap();
    assert_eq!(
        status(&pending),
        OAuthDeviceTokenStatus::AuthorizationPending
    );
    assert!(pending.tokens.is_none());
    let hurried = exchange(&sa, &client_id, &grant.device_code).await.unwrap();
    assert_eq!(status(&hurried), OAuthDeviceTokenStatus::SlowDown);

    let shown = lookup(&sa, &grant.user_code).await.expect("lookup");
    assert_eq!(shown.client_id, client_id);
    assert_eq!(shown.name, "datactl");
    assert_eq!(shown.scopes, vec![APPLY, READ]);

    approve(&sa, &grant.user_code, &user_id)
        .await
        .expect("approve");
    assert_eq!(
        lookup(&sa, &grant.user_code).await.unwrap_err().code(),
        tonic::Code::NotFound
    );

    let issued = exchange(&sa, &client_id, &grant.device_code).await.unwrap();
    assert_eq!(status(&issued), OAuthDeviceTokenStatus::Issued);
    let tokens = issued.tokens.expect("tokens");
    assert!(tokens.access_token.starts_with("forest_oat_"));
    assert!(tokens.refresh_token.starts_with("forest_ort_"));
    assert!(tokens.id_token.is_empty());
    assert_eq!(tokens.scopes, vec![APPLY, READ]);

    let seen = introspect(&sa, &tokens.access_token).await;
    assert!(seen.active);
    assert_eq!(seen.app_id, app_id);
    assert_eq!(seen.organisation_id, org_id);
    assert_eq!(seen.user_id, user_id);
    assert_eq!(seen.scopes, vec![APPLY, READ]);

    let consent = sa
        .oauth_apps()
        .get_o_auth_consent(authed(
            SA,
            GetOAuthConsentRequest {
                client_id: client_id.clone(),
                user_id: user_id.clone(),
            },
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(consent.scopes.len(), 2);

    let replay = exchange(&sa, &client_id, &grant.device_code).await.unwrap();
    assert_eq!(status(&replay), OAuthDeviceTokenStatus::ExpiredToken);
    assert!(replay.tokens.is_none());

    let refreshed = sa
        .oauth_apps()
        .refresh_o_auth_token(authed(
            SA,
            RefreshOAuthTokenRequest {
                client_id: client_id.clone(),
                client_secret: String::new(),
                refresh_token: tokens.refresh_token.clone(),
            },
        ))
        .await
        .expect("refresh")
        .into_inner()
        .tokens
        .expect("tokens");
    assert_eq!(refreshed.scopes, vec![APPLY, READ]);

    sa.oauth_apps()
        .revoke_o_auth_grant(authed(
            SA,
            RevokeOAuthGrantRequest {
                user_id: user_id.clone(),
                app_id: app_id.clone(),
            },
        ))
        .await
        .expect("revoke");
    assert!(!introspect(&sa, &refreshed.access_token).await.active);
    let err = sa
        .oauth_apps()
        .refresh_o_auth_token(authed(
            SA,
            RefreshOAuthTokenRequest {
                client_id,
                client_secret: String::new(),
                refresh_token: refreshed.refresh_token,
            },
        ))
        .await
        .expect_err("refresh after revoke");
    assert_eq!(err.message(), "invalid_grant");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_narrower_request_gets_only_what_it_asked_for() {
    let app_fx = fixture().await.unwrap();
    let sa = restricted_fixture().await.unwrap();
    let (user_id, token) = registered_user(&app_fx).await;
    let org_id = create_org(&app_fx, &token).await;
    let (_, client_id) = device_app(&app_fx, &token, &org_id).await;

    let grant = initiate(&sa, &client_id, &[READ]).await.unwrap();
    approve(&sa, &grant.user_code, &user_id).await.unwrap();
    let tokens = exchange(&sa, &client_id, &grant.device_code)
        .await
        .unwrap()
        .tokens
        .expect("tokens");
    assert_eq!(tokens.scopes, vec![READ]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_denied_grant_is_access_denied_and_cannot_then_be_approved() {
    let app_fx = fixture().await.unwrap();
    let sa = restricted_fixture().await.unwrap();
    let (user_id, token) = registered_user(&app_fx).await;
    let org_id = create_org(&app_fx, &token).await;
    let (_, client_id) = device_app(&app_fx, &token, &org_id).await;

    let grant = initiate(&sa, &client_id, &[APPLY, READ]).await.unwrap();
    sa.oauth_apps()
        .deny_o_auth_device_authorization(authed(
            SA,
            DenyOAuthDeviceAuthorizationRequest {
                user_code: grant.user_code.clone(),
                user_id: user_id.clone(),
            },
        ))
        .await
        .expect("deny");

    let denied = exchange(&sa, &client_id, &grant.device_code).await.unwrap();
    assert_eq!(status(&denied), OAuthDeviceTokenStatus::AccessDenied);
    assert!(denied.tokens.is_none());
    assert!(approve(&sa, &grant.user_code, &user_id).await.is_err());
    let still = exchange(&sa, &client_id, &grant.device_code).await.unwrap();
    assert_eq!(status(&still), OAuthDeviceTokenStatus::AccessDenied);
}

#[tokio::test(flavor = "multi_thread")]
async fn another_app_can_neither_redeem_nor_burn_the_grant() {
    let app_fx = fixture().await.unwrap();
    let sa = restricted_fixture().await.unwrap();
    let (user_id, token) = registered_user(&app_fx).await;
    let org_id = create_org(&app_fx, &token).await;
    let (_, client_id) = device_app(&app_fx, &token, &org_id).await;
    let (_, other_client) = device_app(&app_fx, &token, &org_id).await;

    let grant = initiate(&sa, &client_id, &[APPLY]).await.unwrap();
    approve(&sa, &grant.user_code, &user_id).await.unwrap();

    let stolen = exchange(&sa, &other_client, &grant.device_code)
        .await
        .unwrap();
    assert_eq!(status(&stolen), OAuthDeviceTokenStatus::ExpiredToken);
    assert!(stolen.tokens.is_none());

    let issued = exchange(&sa, &client_id, &grant.device_code).await.unwrap();
    assert_eq!(status(&issued), OAuthDeviceTokenStatus::Issued);
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_redemptions_mint_exactly_one_token_pair() {
    let app_fx = fixture().await.unwrap();
    let sa = restricted_fixture().await.unwrap();
    let (user_id, token) = registered_user(&app_fx).await;
    let org_id = create_org(&app_fx, &token).await;
    let (app_id, client_id) = device_app(&app_fx, &token, &org_id).await;

    let grant = initiate(&sa, &client_id, &[APPLY, READ]).await.unwrap();
    approve(&sa, &grant.user_code, &user_id).await.unwrap();

    let attempts: Vec<_> = (0..8)
        .map(|_| {
            let sa = sa.clone();
            let client_id = client_id.clone();
            let device_code = grant.device_code.clone();
            tokio::spawn(async move { exchange(&sa, &client_id, &device_code).await })
        })
        .collect();
    let mut issued = 0;
    for attempt in attempts {
        if let Ok(resp) = attempt.await.unwrap()
            && resp.tokens.is_some()
        {
            issued += 1;
        }
    }
    assert_eq!(issued, 1);

    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM oauth_access_tokens WHERE app_id = $1 AND user_id = $2",
    )
    .bind(uuid::Uuid::parse_str(&app_id).unwrap())
    .bind(uuid::Uuid::parse_str(&user_id).unwrap())
    .fetch_one(&sa.db)
    .await
    .unwrap();
    assert_eq!(live, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn only_registered_apps_and_their_scopes_may_start_a_grant() {
    let app_fx = fixture().await.unwrap();
    let sa = restricted_fixture().await.unwrap();
    let (_, token) = registered_user(&app_fx).await;
    let org_id = create_org(&app_fx, &token).await;
    let (_, client_id) = device_app(&app_fx, &token, &org_id).await;

    let err = initiate(&sa, "forest_oa_unknown", &[APPLY])
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unauthenticated);
    assert_eq!(err.message(), "invalid_client");

    let err = initiate(&sa, &client_id, &["profile"]).await.unwrap_err();
    assert_eq!(err.message(), "invalid_scope");
    let err = initiate(&sa, &client_id, &[]).await.unwrap_err();
    assert_eq!(err.message(), "invalid_scope");

    let code_only = app_fx
        .oauth_apps()
        .create_o_auth_app(authed(&token, app_request(&org_id, true, &[])))
        .await
        .unwrap()
        .into_inner()
        .app
        .unwrap();
    let err = initiate(&sa, &code_only.client_id, &[APPLY])
        .await
        .unwrap_err();
    assert_eq!(err.message(), "unsupported_grant_type");

    let confidential = app_fx
        .oauth_apps()
        .create_o_auth_app(authed(&token, app_request(&org_id, false, &[DEVICE])))
        .await
        .unwrap()
        .into_inner();
    let cid = confidential.app.unwrap().client_id;
    let err = initiate(&sa, &cid, &[APPLY]).await.unwrap_err();
    assert_eq!(err.message(), "invalid_client");
    sa.oauth_apps()
        .initiate_o_auth_device_authorization(authed(
            SA,
            InitiateOAuthDeviceAuthorizationRequest {
                client_id: cid,
                client_secret: confidential.client_secret,
                scopes: vec![APPLY.into()],
            },
        ))
        .await
        .expect("confidential app with its secret");

    let err = app_fx
        .oauth_apps()
        .initiate_o_auth_device_authorization(authed(
            &token,
            InitiateOAuthDeviceAuthorizationRequest {
                client_id,
                client_secret: String::new(),
                scopes: vec![APPLY.into()],
            },
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::PermissionDenied);
}

#[tokio::test(flavor = "multi_thread")]
async fn withdrawing_the_device_grant_stops_an_outstanding_one() {
    let app_fx = fixture().await.unwrap();
    let sa = restricted_fixture().await.unwrap();
    let (user_id, token) = registered_user(&app_fx).await;
    let org_id = create_org(&app_fx, &token).await;
    let (app_id, client_id) = device_app(&app_fx, &token, &org_id).await;

    let grant = initiate(&sa, &client_id, &[APPLY]).await.unwrap();
    let mut update = app_request(&org_id, true, &["authorization_code"]);
    app_fx
        .oauth_apps()
        .update_o_auth_app(authed(
            &token,
            UpdateOAuthAppRequest {
                organisation_id: org_id.clone(),
                app_id,
                name: update.name,
                description: update.description,
                homepage_url: update.homepage_url,
                redirect_uris: std::mem::take(&mut update.redirect_uris),
                scopes: update.scopes,
                grant_types: update.grant_types,
            },
        ))
        .await
        .expect("withdraw the device grant");

    assert_eq!(
        lookup(&sa, &grant.user_code).await.unwrap_err().code(),
        tonic::Code::NotFound
    );
    assert!(approve(&sa, &grant.user_code, &user_id).await.is_err());
    let err = exchange(&sa, &client_id, &grant.device_code)
        .await
        .unwrap_err();
    assert_eq!(err.message(), "unsupported_grant_type");
}

#[tokio::test(flavor = "multi_thread")]
async fn narrowing_the_app_stops_asking_for_scopes_it_no_longer_holds() {
    let app_fx = fixture().await.unwrap();
    let sa = restricted_fixture().await.unwrap();
    let (user_id, token) = registered_user(&app_fx).await;
    let org_id = create_org(&app_fx, &token).await;
    let (app_id, client_id) = device_app(&app_fx, &token, &org_id).await;

    let grant = initiate(&sa, &client_id, &[APPLY, READ]).await.unwrap();
    let narrowed = app_request(&org_id, true, &["authorization_code", DEVICE]);
    app_fx
        .oauth_apps()
        .update_o_auth_app(authed(
            &token,
            UpdateOAuthAppRequest {
                organisation_id: org_id.clone(),
                app_id,
                name: narrowed.name,
                description: narrowed.description,
                homepage_url: narrowed.homepage_url,
                redirect_uris: narrowed.redirect_uris,
                scopes: vec![READ.into()],
                grant_types: narrowed.grant_types,
            },
        ))
        .await
        .expect("narrow the app to read");

    assert_eq!(
        lookup(&sa, &grant.user_code).await.unwrap_err().code(),
        tonic::Code::NotFound
    );
    assert!(approve(&sa, &grant.user_code, &user_id).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn app_grants_and_forest_cli_logins_never_cross() {
    let app_fx = fixture().await.unwrap();
    let sa = restricted_fixture().await.unwrap();
    let (user_id, token) = registered_user(&app_fx).await;
    let org_id = create_org(&app_fx, &token).await;
    let (_, client_id) = device_app(&app_fx, &token, &org_id).await;
    let mut users = sa.users();

    let app_grant = initiate(&sa, &client_id, &[APPLY]).await.unwrap();
    let err = users
        .approve_device_login(authed(
            SA,
            ApproveDeviceLoginRequest {
                user_code: app_grant.user_code.clone(),
                user_id: user_id.clone(),
                approving_ip: String::new(),
                approving_user_agent: String::new(),
            },
        ))
        .await;
    assert!(
        err.is_err(),
        "the Forest CLI approval path must not approve an app grant"
    );
    approve(&sa, &app_grant.user_code, &user_id).await.unwrap();
    let polled = users
        .poll_device_login(PollDeviceLoginRequest {
            device_code: app_grant.device_code.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(polled.status, DeviceLoginStatus::Expired as i32);
    assert!(polled.tokens.is_none());
    let issued = exchange(&sa, &client_id, &app_grant.device_code)
        .await
        .unwrap();
    assert_eq!(status(&issued), OAuthDeviceTokenStatus::Issued);

    let cli = users
        .initiate_device_login(InitiateDeviceLoginRequest {
            client_name: "forest-cli".into(),
            client_version: "0.3.2".into(),
            scopes: vec![],
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        lookup(&sa, &cli.user_code).await.unwrap_err().code(),
        tonic::Code::NotFound
    );
    assert!(approve(&sa, &cli.user_code, &user_id).await.is_err());

    users
        .approve_device_login(authed(
            SA,
            ApproveDeviceLoginRequest {
                user_code: cli.user_code,
                user_id: user_id.clone(),
                approving_ip: String::new(),
                approving_user_agent: String::new(),
            },
        ))
        .await
        .expect("forest cli approval");
    let stolen = exchange(&sa, &client_id, &cli.device_code).await.unwrap();
    assert_eq!(status(&stolen), OAuthDeviceTokenStatus::ExpiredToken);
    let session = users
        .poll_device_login(PollDeviceLoginRequest {
            device_code: cli.device_code,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(session.status, DeviceLoginStatus::Approved as i32);

    let forest_session = session.tokens.expect("tokens").access_token;
    assert!(!introspect(&sa, &forest_session).await.active);
}
