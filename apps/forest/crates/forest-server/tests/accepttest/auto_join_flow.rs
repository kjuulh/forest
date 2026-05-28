//! Acceptance tests for the org auto-invite-by-verified-email-domain flow
//! (DATA-252).
//!
//! Each test boots two real users (admin + joiner) against the default
//! fixture, exercises the gRPC RPCs end-to-end against a real Postgres,
//! and asserts both success and failure modes.

use forest_grpc_interface::*;
use tonic::metadata::MetadataValue;

use crate::accepttest::fixtures::{
    Fixture, RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY, fixture, mark_email_verified,
    restricted_fixture,
};

fn authed_request<T>(token: &str, inner: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(inner);
    let val: MetadataValue<_> = format!("Bearer {token}").parse().expect("valid metadata");
    req.metadata_mut().insert("authorization", val);
    req
}

fn unique_username() -> String {
    format!("user-{}", uuid::Uuid::now_v7())
}

fn unique_email_at(domain: &str) -> String {
    format!("user-{}@{}", uuid::Uuid::now_v7(), domain)
}

fn unique_org_name() -> String {
    format!("autojoin-org-{}", uuid::Uuid::now_v7())
}

/// Register a user, immediately mark the email verified in-db (the
/// magic-link flow is exercised elsewhere — here we just need a verified
/// account quickly), log in, and return (token, user_id).
async fn register_and_verify(fixture: &Fixture, email: &str) -> (String, uuid::Uuid) {
    let mut users = fixture.users();
    let username = unique_username();

    let registered = users
        .register(RegisterRequest {
            username,
            email: email.into(),
            password: "TestPassword123!".into(),
        })
        .await
        .expect("register")
        .into_inner();

    let user_id_str = registered.user.expect("user").user_id;
    let user_id: uuid::Uuid = user_id_str.parse().unwrap();

    mark_email_verified(&fixture.db, user_id, email)
        .await
        .expect("mark verified");

    // If tokens were issued at register-time (verification not required on
    // the default fixture) use them; otherwise log in.
    let token = if let Some(t) = registered.tokens {
        t.access_token
    } else {
        users
            .login(LoginRequest {
                identifier: Some(login_request::Identifier::Email(email.into())),
                password: "TestPassword123!".into(),
            })
            .await
            .expect("login")
            .into_inner()
            .tokens
            .expect("tokens")
            .access_token
    };

    (token, user_id)
}

/// Create an org owned by the given user, return its UUID.
async fn create_org(fixture: &Fixture, token: &str, name: &str) -> uuid::Uuid {
    let resp = fixture
        .organisations()
        .create_organisation(authed_request(
            token,
            CreateOrganisationRequest { name: name.into() },
        ))
        .await
        .expect("create org")
        .into_inner();
    resp.organisation_id.parse().unwrap()
}

/// Helper: add the domain to the org and verify it via the mock DNS
/// resolver (preload the expected TXT record before calling
/// `verify_allowed_domain`).
async fn add_and_verify_domain(
    fixture: &Fixture,
    admin_token: &str,
    org_id: uuid::Uuid,
    domain: &str,
) {
    let add_resp = fixture
        .organisations()
        .add_allowed_domain(authed_request(
            admin_token,
            AddAllowedDomainRequest {
                organisation_id: org_id.to_string(),
                domain: domain.into(),
                policy: String::new(),
            },
        ))
        .await
        .expect("add allowed domain")
        .into_inner();
    let token = add_resp
        .domain
        .expect("returned domain")
        .dns_verification_token;

    fixture
        .dns
        .set_txt(&format!("_forest-verify.{domain}"), &token);

    let verify = fixture
        .organisations()
        .verify_allowed_domain(authed_request(
            admin_token,
            VerifyAllowedDomainRequest {
                organisation_id: org_id.to_string(),
                domain: domain.into(),
            },
        ))
        .await
        .expect("verify allowed domain")
        .into_inner();
    assert_eq!(
        verify.status,
        verify_allowed_domain_response::Status::Verified as i32,
        "expected first-time Verified, got {}",
        verify.status
    );
}

// ── Happy path ───────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn admin_can_add_domain_and_user_can_join() {
    let fixture = fixture().await.unwrap();

    // Per-test domain so concurrent runs don't trample each other's
    // allowlist rows or DNS mock entries.
    let domain = format!("co-{}.example", uuid::Uuid::now_v7().simple());

    let (admin_token, _admin_id) =
        register_and_verify(&fixture, &unique_email_at(&domain)).await;
    let org_id = create_org(&fixture, &admin_token, &unique_org_name()).await;

    add_and_verify_domain(&fixture, &admin_token, org_id, &domain).await;

    // A second user signs up with the same domain and verifies.
    let (joiner_token, joiner_id) =
        register_and_verify(&fixture, &unique_email_at(&domain)).await;

    // Offer is surfaced.
    let offers = fixture
        .organisations()
        .list_join_offers(authed_request(&joiner_token, ListJoinOffersRequest {}))
        .await
        .expect("list offers")
        .into_inner();
    let our_offer = offers
        .offers
        .iter()
        .find(|o| o.organisation_id == org_id.to_string())
        .expect("offer for our org is present");
    assert_eq!(our_offer.matched_domain, domain.to_lowercase());

    // Accept.
    let accept_resp = fixture
        .organisations()
        .accept_join_offer(authed_request(
            &joiner_token,
            AcceptJoinOfferRequest {
                organisation_id: org_id.to_string(),
            },
        ))
        .await
        .expect("accept join offer")
        .into_inner();
    let member = accept_resp.member.expect("member");
    assert_eq!(member.user_id, joiner_id.to_string());
    assert_eq!(member.role, "member");

    // Subsequent list_join_offers should no longer include this org —
    // already-member filter kicks in.
    let after = fixture
        .organisations()
        .list_join_offers(authed_request(&joiner_token, ListJoinOffersRequest {}))
        .await
        .expect("list offers post-accept")
        .into_inner();
    assert!(
        !after
            .offers
            .iter()
            .any(|o| o.organisation_id == org_id.to_string()),
        "joined org should not reappear in offers"
    );
}

// ── Unverified domain grants nothing ─────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn unverified_domain_produces_no_offer() {
    let fixture = fixture().await.unwrap();
    let domain = format!("co-{}.example", uuid::Uuid::now_v7().simple());
    let (admin_token, _) = register_and_verify(&fixture, &unique_email_at(&domain)).await;
    let org_id = create_org(&fixture, &admin_token, &unique_org_name()).await;

    // Add — but do NOT verify via DNS.
    fixture
        .organisations()
        .add_allowed_domain(authed_request(
            &admin_token,
            AddAllowedDomainRequest {
                organisation_id: org_id.to_string(),
                domain: domain.clone(),
                policy: String::new(),
            },
        ))
        .await
        .expect("add domain");

    let (joiner_token, _) = register_and_verify(&fixture, &unique_email_at(&domain)).await;

    let offers = fixture
        .organisations()
        .list_join_offers(authed_request(&joiner_token, ListJoinOffersRequest {}))
        .await
        .expect("list offers")
        .into_inner();
    assert!(
        !offers
            .offers
            .iter()
            .any(|o| o.organisation_id == org_id.to_string()),
        "unverified domain must not produce an offer"
    );

    let err = fixture
        .organisations()
        .accept_join_offer(authed_request(
            &joiner_token,
            AcceptJoinOfferRequest {
                organisation_id: org_id.to_string(),
            },
        ))
        .await
        .expect_err("accept without DNS verification must fail");
    assert_eq!(err.code(), tonic::Code::PermissionDenied);
}

// ── Verify flow: missing TXT → Missing; correct TXT → Verified ───────────────

#[tokio::test(flavor = "multi_thread")]
async fn verify_returns_missing_when_txt_absent() {
    let fixture = fixture().await.unwrap();
    let domain = format!("co-{}.example", uuid::Uuid::now_v7().simple());
    let (admin_token, _) = register_and_verify(&fixture, &unique_email_at(&domain)).await;
    let org_id = create_org(&fixture, &admin_token, &unique_org_name()).await;

    fixture
        .organisations()
        .add_allowed_domain(authed_request(
            &admin_token,
            AddAllowedDomainRequest {
                organisation_id: org_id.to_string(),
                domain: domain.clone(),
                policy: String::new(),
            },
        ))
        .await
        .expect("add domain");

    // Deliberately do NOT preload the DNS record.
    let verify = fixture
        .organisations()
        .verify_allowed_domain(authed_request(
            &admin_token,
            VerifyAllowedDomainRequest {
                organisation_id: org_id.to_string(),
                domain: domain.clone(),
            },
        ))
        .await
        .expect("verify (no records)")
        .into_inner();
    assert_eq!(
        verify.status,
        verify_allowed_domain_response::Status::Missing as i32
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn verify_is_idempotent() {
    let fixture = fixture().await.unwrap();
    let domain = format!("co-{}.example", uuid::Uuid::now_v7().simple());
    let (admin_token, _) = register_and_verify(&fixture, &unique_email_at(&domain)).await;
    let org_id = create_org(&fixture, &admin_token, &unique_org_name()).await;

    add_and_verify_domain(&fixture, &admin_token, org_id, &domain).await;

    let again = fixture
        .organisations()
        .verify_allowed_domain(authed_request(
            &admin_token,
            VerifyAllowedDomainRequest {
                organisation_id: org_id.to_string(),
                domain: domain.clone(),
            },
        ))
        .await
        .expect("re-verify")
        .into_inner();
    assert_eq!(
        again.status,
        verify_allowed_domain_response::Status::AlreadyVerified as i32
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn verify_rejects_wrong_txt_token() {
    let fixture = fixture().await.unwrap();
    let domain = format!("co-{}.example", uuid::Uuid::now_v7().simple());
    let (admin_token, _) = register_and_verify(&fixture, &unique_email_at(&domain)).await;
    let org_id = create_org(&fixture, &admin_token, &unique_org_name()).await;

    fixture
        .organisations()
        .add_allowed_domain(authed_request(
            &admin_token,
            AddAllowedDomainRequest {
                organisation_id: org_id.to_string(),
                domain: domain.clone(),
                policy: String::new(),
            },
        ))
        .await
        .expect("add domain");

    // Attacker plants their *own* random token. Should NOT verify.
    fixture
        .dns
        .set_txt(&format!("_forest-verify.{domain}"), "attacker-supplied-bogus-value");

    let verify = fixture
        .organisations()
        .verify_allowed_domain(authed_request(
            &admin_token,
            VerifyAllowedDomainRequest {
                organisation_id: org_id.to_string(),
                domain: domain.clone(),
            },
        ))
        .await
        .expect("verify with wrong token")
        .into_inner();
    assert_eq!(
        verify.status,
        verify_allowed_domain_response::Status::Missing as i32
    );
}

// ── Multi-@ email bypass — must NOT produce an offer ─────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn multi_at_email_does_not_match_middle_segment() {
    // Belt-and-suspenders test for the SQL fix: even if an upstream
    // validation bug lets `attacker@victim.com@evil.com` into user_emails,
    // the rightmost-@ domain extraction in the JOIN means we'd match
    // 'evil.com' (which no legitimate org verifies), not 'victim.com'.
    let fixture = fixture().await.unwrap();
    let victim_domain = format!("victim-{}.example", uuid::Uuid::now_v7().simple());

    let (admin_token, _) =
        register_and_verify(&fixture, &unique_email_at(&victim_domain)).await;
    let org_id = create_org(&fixture, &admin_token, &unique_org_name()).await;
    add_and_verify_domain(&fixture, &admin_token, org_id, &victim_domain).await;

    // Build an account whose verified email has a multi-@ shape with the
    // victim domain in the *middle*. We insert directly into the DB
    // because the forage `validate_email` layer rejects this — exactly
    // what we want to prove also can't be reached via the SQL JOIN even
    // if a future code path forgot the validation.
    let attacker_email_local = unique_email_at("attacker.test");
    let (attacker_token, attacker_user_id) =
        register_and_verify(&fixture, &attacker_email_local).await;

    let weird_email = format!("attacker@{}@evil.example", &victim_domain);
    sqlx::query!(
        "INSERT INTO user_emails (user_id, email, verified, verification_source)
         VALUES ($1, $2, TRUE, 'magic_link')",
        attacker_user_id,
        weird_email,
    )
    .execute(&fixture.db)
    .await
    .expect("insert weird email");

    let offers = fixture
        .organisations()
        .list_join_offers(authed_request(&attacker_token, ListJoinOffersRequest {}))
        .await
        .expect("list offers")
        .into_inner();

    assert!(
        !offers
            .offers
            .iter()
            .any(|o| o.organisation_id == org_id.to_string()),
        "multi-@ email must not match the middle segment as a domain; \
         offers were: {:?}",
        offers.offers
    );
}

// (The free-mail denylist was removed — DNS verification is the boundary now.
//  Adding gmail.com or any other "public" mailbox is allowed; the row simply
//  grants nothing until DNS verification succeeds, which the org can't do.)

// ── Negative: non-admin cannot modify ────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn non_admin_cannot_add_allowed_domain() {
    let fixture = fixture().await.unwrap();
    let domain = format!("co-{}.example", uuid::Uuid::now_v7().simple());

    let (admin_token, _) = register_and_verify(&fixture, &unique_email_at(&domain)).await;
    let org_id = create_org(&fixture, &admin_token, &unique_org_name()).await;

    // A second user who is NOT a member of the org.
    let (outsider_token, _) =
        register_and_verify(&fixture, &unique_email_at("outsiders.test")).await;

    let err = fixture
        .organisations()
        .add_allowed_domain(authed_request(
            &outsider_token,
            AddAllowedDomainRequest {
                organisation_id: org_id.to_string(),
                domain: "some-other.test".into(),
                policy: String::new(),
            },
        ))
        .await
        .expect_err("non-admin must be rejected");

    assert_eq!(err.code(), tonic::Code::PermissionDenied);
}

// ── Negative: unverified email yields no offer ───────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn unverified_email_does_not_get_offer() {
    let fixture = fixture().await.unwrap();
    let domain = format!("co-{}.example", uuid::Uuid::now_v7().simple());

    let (admin_token, _) = register_and_verify(&fixture, &unique_email_at(&domain)).await;
    let org_id = create_org(&fixture, &admin_token, &unique_org_name()).await;

    add_and_verify_domain(&fixture, &admin_token, org_id, &domain).await;

    // Register a joiner WITHOUT verifying the email.
    let joiner_email = unique_email_at(&domain);
    let registered = fixture
        .users()
        .register(RegisterRequest {
            username: unique_username(),
            email: joiner_email.clone(),
            password: "TestPassword123!".into(),
        })
        .await
        .expect("register joiner")
        .into_inner();
    // The default (unrestricted) fixture issues tokens at register-time
    // without requiring verification.
    let joiner_token = registered.tokens.expect("tokens").access_token;

    let offers = fixture
        .organisations()
        .list_join_offers(authed_request(&joiner_token, ListJoinOffersRequest {}))
        .await
        .expect("list offers")
        .into_inner();
    assert!(
        !offers
            .offers
            .iter()
            .any(|o| o.organisation_id == org_id.to_string()),
        "unverified email must not surface an offer"
    );

    // Accepting anyway must be rejected.
    let err = fixture
        .organisations()
        .accept_join_offer(authed_request(
            &joiner_token,
            AcceptJoinOfferRequest {
                organisation_id: org_id.to_string(),
            },
        ))
        .await
        .expect_err("accept without verified email must fail");
    assert_eq!(err.code(), tonic::Code::PermissionDenied);
}

// ── Negative: domain removed before accept ───────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn accepting_after_domain_removed_fails() {
    let fixture = fixture().await.unwrap();
    let domain = format!("co-{}.example", uuid::Uuid::now_v7().simple());

    let (admin_token, _) = register_and_verify(&fixture, &unique_email_at(&domain)).await;
    let org_id = create_org(&fixture, &admin_token, &unique_org_name()).await;

    add_and_verify_domain(&fixture, &admin_token, org_id, &domain).await;

    let (joiner_token, _) = register_and_verify(&fixture, &unique_email_at(&domain)).await;

    // Confirm an offer exists at this point.
    let offers = fixture
        .organisations()
        .list_join_offers(authed_request(&joiner_token, ListJoinOffersRequest {}))
        .await
        .expect("list offers")
        .into_inner();
    assert!(offers
        .offers
        .iter()
        .any(|o| o.organisation_id == org_id.to_string()));

    // Admin removes the domain.
    let removed = fixture
        .organisations()
        .remove_allowed_domain(authed_request(
            &admin_token,
            RemoveAllowedDomainRequest {
                organisation_id: org_id.to_string(),
                domain: domain.clone(),
            },
        ))
        .await
        .expect("remove domain")
        .into_inner();
    assert!(removed.removed);

    // Joiner's accept now fails: re-check is run inside the accept tx.
    let err = fixture
        .organisations()
        .accept_join_offer(authed_request(
            &joiner_token,
            AcceptJoinOfferRequest {
                organisation_id: org_id.to_string(),
            },
        ))
        .await
        .expect_err("accept after removal must fail");
    assert_eq!(err.code(), tonic::Code::PermissionDenied);
}

// ── Negative: malformed domain ───────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn malformed_domain_rejected() {
    let fixture = fixture().await.unwrap();
    let (admin_token, _) = register_and_verify(&fixture, &unique_email_at("admin.test")).await;
    let org_id = create_org(&fixture, &admin_token, &unique_org_name()).await;

    let err = fixture
        .organisations()
        .add_allowed_domain(authed_request(
            &admin_token,
            AddAllowedDomainRequest {
                organisation_id: org_id.to_string(),
                domain: "no-tld-here".into(),
                policy: String::new(),
            },
        ))
        .await
        .expect_err("malformed domain rejected");
    assert_eq!(err.code(), tonic::Code::InvalidArgument);
}

// ── Negative: auto_join_oauth policy gated until v1.1 ────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn auto_join_oauth_policy_rejected_in_v1() {
    let fixture = fixture().await.unwrap();
    let (admin_token, _) = register_and_verify(&fixture, &unique_email_at("admin.test")).await;
    let org_id = create_org(&fixture, &admin_token, &unique_org_name()).await;

    let err = fixture
        .organisations()
        .add_allowed_domain(authed_request(
            &admin_token,
            AddAllowedDomainRequest {
                organisation_id: org_id.to_string(),
                domain: "policy-test.example".into(),
                policy: "auto_join_oauth".into(),
            },
        ))
        .await
        .expect_err("auto_join_oauth gated in v1");
    assert_eq!(err.code(), tonic::Code::Unimplemented);
}

// ── OAuth signup records `verification_source = oauth_<provider>` ────────────

#[tokio::test(flavor = "multi_thread")]
async fn oauth_signup_records_clean_verification_source() {
    // Regression test for the bug where verification_source was stored as
    // "oauth_oauth_provider_github" because the gRPC handler passes the
    // raw protobuf enum string. After the fix, the value must be
    // "oauth_github" (or "oauth_google") so future silent-JIT logic can
    // pattern-match cleanly.
    let fixture = restricted_fixture().await.unwrap();
    let email = format!("oauth-test-{}@understory.io", uuid::Uuid::now_v7());

    let _ = fixture
        .users()
        .o_auth_login(authed_request(
            RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY,
            OAuthLoginRequest {
                provider: OAuthProvider::OauthProviderGithub as i32,
                provider_user_id: format!("gh-{}", uuid::Uuid::now_v7()),
                provider_email: email.clone(),
                provider_display_name: "Test User".into(),
                provider_data_json: String::new(),
            },
        ))
        .await
        .expect("oauth login (new user)")
        .into_inner();

    let source: String = sqlx::query_scalar!(
        "SELECT verification_source FROM user_emails WHERE email = $1",
        email,
    )
    .fetch_one(&fixture.db)
    .await
    .expect("query verification_source");

    assert_eq!(
        source, "oauth_github",
        "expected oauth_github, got {source} — the protobuf enum prefix \
         should have been stripped before formatting"
    );
}
