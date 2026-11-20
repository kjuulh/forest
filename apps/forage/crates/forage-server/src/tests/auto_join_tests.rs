use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::platform::{
    AllowedDomain, JoinOffer, OrgMember, PlatformError, VerifyDomainOutcome,
};
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

fn body_to_string(body: Body) -> impl std::future::Future<Output = String> {
    async {
        let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }
}

#[tokio::test]
async fn access_page_renders_existing_domains() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_allowed_domains_result: Some(Ok(vec![AllowedDomain {
            domain: "understory.io".into(),
            policy: "auto_invite_any_verified".into(),
            dns_verified: false,
            dns_verification_token: "tok-abc123".into(),
            created_at: None,
        }])),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/settings/access")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let html = body_to_string(response.into_body()).await;
    assert!(html.contains("understory.io"), "html should list the domain");
    assert!(html.contains("Auto-invite"), "page heading should appear");
    assert!(
        html.contains("Add domain"),
        "admin should see the add-domain form"
    );
    assert!(
        html.contains("_forest-verify.understory.io"),
        "should show the TXT record name"
    );
    assert!(
        html.contains("tok-abc123"),
        "should show the verification token"
    );
    assert!(
        html.contains("Awaiting verification"),
        "unverified row should be tagged"
    );
}

#[tokio::test]
async fn access_page_verified_row_hides_dns_instructions() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_allowed_domains_result: Some(Ok(vec![AllowedDomain {
            domain: "understory.io".into(),
            policy: "auto_invite_any_verified".into(),
            dns_verified: true,
            dns_verification_token: "tok-secret".into(),
            created_at: None,
        }])),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/settings/access")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let html = body_to_string(response.into_body()).await;
    assert!(html.contains("DNS verified"));
    assert!(
        !html.contains("Verify DNS"),
        "verified row should not show the verify button"
    );
}

#[tokio::test]
async fn access_page_non_admin_hides_add_form() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_allowed_domains_result: Some(Ok(vec![])),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session_member(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/settings/access")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let html = body_to_string(response.into_body()).await;
    assert!(
        !html.contains("Add domain"),
        "non-admin should not see add-domain form"
    );
}

#[tokio::test]
async fn add_domain_redirects_back_on_success() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        add_allowed_domain_result: Some(Ok(AllowedDomain {
            domain: "understory.io".into(),
            policy: "auto_invite_any_verified".into(),
            dns_verified: false,
            dns_verification_token: "tok-abc".into(),
            created_at: None,
        })),
        list_allowed_domains_result: Some(Ok(vec![])),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/settings/access")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("domain=understory.io&_csrf=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response.headers().get("location").unwrap();
    assert_eq!(location, "/orgs/testorg/settings/access");
}

#[tokio::test]
async fn verify_domain_success_renders_info_flash() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        verify_allowed_domain_result: Some(Ok(VerifyDomainOutcome::Verified)),
        list_allowed_domains_result: Some(Ok(vec![])),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/settings/access/verify")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("domain=understory.io&_csrf=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_to_string(response.into_body()).await;
    assert!(html.contains("Verified ownership"), "should show info flash");
}

#[tokio::test]
async fn verify_domain_missing_renders_error_flash() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        verify_allowed_domain_result: Some(Ok(VerifyDomainOutcome::Missing)),
        list_allowed_domains_result: Some(Ok(vec![])),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/settings/access/verify")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("domain=understory.io&_csrf=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_to_string(response.into_body()).await;
    assert!(
        html.contains("not found yet"),
        "missing-TXT message should appear"
    );
}

#[tokio::test]
async fn verify_domain_non_admin_is_forbidden() {
    let (state, sessions) = test_state();
    let cookie = create_test_session_member(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/settings/access/verify")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("domain=understory.io&_csrf=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn add_domain_csrf_invalid_is_rejected() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/settings/access")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("domain=understory.io&_csrf=WRONG"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn non_admin_cannot_add_domain() {
    let (state, sessions) = test_state();
    let cookie = create_test_session_member(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/settings/access")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("domain=understory.io&_csrf=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn remove_domain_redirects_back() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/settings/access/remove")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("domain=understory.io&_csrf=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn onboarding_dashboard_no_orgs_shows_join_offer_banner() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_orgs_result: Some(Ok(vec![])),
        list_join_offers_result: Some(Ok(vec![JoinOffer {
            organisation_id: "org-acme".into(),
            organisation_name: "Acme Corp".into(),
            matched_domain: "acme.example".into(),
        }])),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session_no_orgs(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let html = body_to_string(response.into_body()).await;
    assert!(
        html.contains("Welcome to Forest"),
        "onboarding page should still render"
    );
    assert!(
        html.contains("Acme Corp"),
        "join-offer banner should appear above the create-org form"
    );
    assert!(
        html.contains("/join-offers/org-acme/accept"),
        "Join button should POST to the accept route"
    );
}

#[tokio::test]
async fn dashboard_shows_join_offer_banner() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_join_offers_result: Some(Ok(vec![JoinOffer {
            organisation_id: "org-acme".into(),
            organisation_name: "Acme Corp".into(),
            matched_domain: "acme.example".into(),
        }])),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let html = body_to_string(response.into_body()).await;
    assert!(
        html.contains("Acme Corp"),
        "banner should mention the org name"
    );
    assert!(
        html.contains("acme.example"),
        "banner should mention the matched domain"
    );
    assert!(
        html.contains("/join-offers/org-acme/accept"),
        "banner should POST to the accept route"
    );
}

#[tokio::test]
async fn dashboard_without_offers_hides_banner() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let html = body_to_string(response.into_body()).await;
    assert!(
        !html.contains("/join-offers/"),
        "no banner should render when there are no offers"
    );
}

#[tokio::test]
async fn accept_offer_redirects_to_dashboard() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        accept_join_offer_result: Some(Ok(OrgMember {
            user_id: "user-123".into(),
            username: "kasper".into(),
            role: "member".into(),
            joined_at: None,
        })),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/join-offers/org-acme/accept")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("_csrf=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response.headers().get("location").unwrap();
    assert_eq!(location, "/");
}

#[tokio::test]
async fn accept_offer_no_longer_eligible_shows_403() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        accept_join_offer_result: Some(Err(PlatformError::PermissionDenied(
            "you are not currently eligible to join this organisation".into(),
        ))),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/join-offers/org-acme/accept")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("_csrf=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn accept_offer_csrf_invalid_is_rejected() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/join-offers/org-acme/accept")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("_csrf=WRONG"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
