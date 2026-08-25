//! Sensitive destination metadata: values must not cross the wire unless a
//! caller asks for one specific key.
//!
//! Covers DATA-575, where `destination list` printed live AWS and Cloudflare
//! credentials. The keys that leaked there were free-form (terraform forwards
//! anything it does not declare as a `TF_VAR_*`), so a type-schema flag alone
//! would not have covered them — hence both declaration paths are tested.

use forest_grpc_interface::*;
use tonic::metadata::MetadataValue;

use crate::accepttest::fixtures::fixture;

fn authed_request<T>(token: &str, inner: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(inner);
    let val: MetadataValue<_> = format!("Bearer {}", token).parse().expect("valid metadata");
    req.metadata_mut().insert("authorization", val);
    req
}

async fn register_user(fixture: &crate::accepttest::fixtures::Fixture) -> String {
    let mut users = fixture.users();
    let resp = users
        .register(RegisterRequest {
            username: format!("user-{}", uuid::Uuid::now_v7()),
            email: format!("{}@test.com", uuid::Uuid::now_v7()),
            password: "TestPassword123!".into(),
        })
        .await
        .expect("register user");

    resp.into_inner().tokens.expect("tokens").access_token
}

async fn create_org(fixture: &crate::accepttest::fixtures::Fixture, token: &str) -> String {
    let name = format!("org-{}", uuid::Uuid::now_v7());
    fixture
        .organisations()
        .create_organisation(authed_request(token, CreateOrganisationRequest { name: name.clone() }))
        .await
        .expect("create org");
    name
}

async fn create_env(
    fixture: &crate::accepttest::fixtures::Fixture,
    token: &str,
    org: &str,
) -> String {
    let name = format!("env-{}", uuid::Uuid::now_v7());
    fixture
        .environments()
        .create_environment(authed_request(
            token,
            CreateEnvironmentRequest {
                organisation: org.to_string(),
                name: name.clone(),
                description: None,
                sort_order: 0,
            },
        ))
        .await
        .expect("create environment");
    name
}

fn flux_type() -> DestinationType {
    DestinationType {
        organisation: "forest".into(),
        name: "flux".into(),
        version: 1,
        description: String::new(),
        fields: vec![],
    }
}

fn terraform_type() -> DestinationType {
    DestinationType {
        organisation: "forest".into(),
        name: "terraform".into(),
        version: 1,
        description: String::new(),
        fields: vec![],
    }
}

async fn fetch_destination(
    fixture: &crate::accepttest::fixtures::Fixture,
    token: &str,
    org: &str,
    name: &str,
) -> Destination {
    let resp = fixture
        .destinations()
        .get_destinations(authed_request(
            token,
            GetDestinationsRequest {
                organisation: org.to_string(),
            },
        ))
        .await
        .expect("get destinations")
        .into_inner();

    resp.destinations
        .into_iter()
        .find(|d| d.name == name)
        .expect("destination present in list")
}

/// The flux type declares `git_token` and `webhook_secret` sensitive, so those
/// values must never appear in a GetDestinations response.
#[tokio::test(flavor = "multi_thread")]
async fn type_declared_sensitive_fields_are_withheld_from_the_list_response() {
    let fixture = fixture().await.expect("fixture");
    let token = register_user(&fixture).await;
    let org = create_org(&fixture, &token).await;
    let env = create_env(&fixture, &token, &org).await;
    let dest = format!("flux-{}", uuid::Uuid::now_v7());

    let local_path = format!("/tmp/forest-sensitive-test-{}", uuid::Uuid::now_v7());
    std::fs::create_dir_all(&local_path).expect("create local path");

    fixture
        .destinations()
        .create_destination(authed_request(
            &token,
            CreateDestinationRequest {
                organisation: org.clone(),
                name: dest.clone(),
                environment: env,
                metadata: [
                    ("cluster_name".to_string(), "test-cluster".to_string()),
                    ("namespace".to_string(), "test-namespace".to_string()),
                    ("local_path".to_string(), local_path),
                    ("git_token".to_string(), "ghp_live_token".to_string()),
                    ("webhook_secret".to_string(), "hmac-live".to_string()),
                    (
                        "reconcile_url".to_string(),
                        "http://webhook-receiver.flux-system/hook/live-webhook-path".to_string(),
                    ),
                    (
                        "forest_webhook_url".to_string(),
                        "https://forest.example.com/webhooks/flux".to_string(),
                    ),
                ]
                .into(),
                r#type: Some(flux_type()),
                sensitive_keys: vec![],
            },
        ))
        .await
        .expect("create destination");

    let found = fetch_destination(&fixture, &token, &org, &dest).await;

    // Non-sensitive fields still show.
    assert_eq!(
        found.metadata.get("cluster_name").map(String::as_str),
        Some("test-cluster")
    );

    // Sensitive fields: key names only, values gone.
    let mut withheld = found.sensitive_keys.clone();
    withheld.sort();
    assert_eq!(withheld, vec!["git_token", "reconcile_url", "webhook_secret"]);
    assert!(!found.metadata.contains_key("git_token"));
    assert!(!found.metadata.contains_key("webhook_secret"));
    // The Receiver webhook path is a capability, not configuration: holding the
    // URL is enough to trigger reconciliation, so it withholds like a credential.
    assert!(!found.metadata.contains_key("reconcile_url"));

    let serialised = format!("{found:?}");
    assert!(
        !serialised.contains("ghp_live_token")
            && !serialised.contains("hmac-live")
            && !serialised.contains("live-webhook-path"),
        "credential survived onto the wire: {serialised}"
    );
}

/// DATA-575's actual shape: credentials living in free-form terraform keys, so
/// no type schema mentions them. They are hidden because the destination itself
/// declares them, and they survive create -> persist -> gRPC -> read back.
#[tokio::test(flavor = "multi_thread")]
async fn destination_declared_sensitive_keys_round_trip_and_are_withheld() {
    let fixture = fixture().await.expect("fixture");
    let token = register_user(&fixture).await;
    let org = create_org(&fixture, &token).await;
    let env = create_env(&fixture, &token, &org).await;
    let dest = format!("tf-{}", uuid::Uuid::now_v7());

    fixture
        .destinations()
        .create_destination(authed_request(
            &token,
            CreateDestinationRequest {
                organisation: org.clone(),
                name: dest.clone(),
                environment: env,
                metadata: [
                    ("tf_workspace".to_string(), "platform-dev".to_string()),
                    ("infra_environment".to_string(), "dev".to_string()),
                    ("aws_account_id".to_string(), "123456789012".to_string()),
                    ("aws_access_key_id".to_string(), "AKIAEXAMPLE".to_string()),
                    ("aws_secret_access_key".to_string(), "wJalrXUtn".to_string()),
                    ("cloudflare_token".to_string(), "cf_live_token".to_string()),
                ]
                .into(),
                r#type: Some(terraform_type()),
                sensitive_keys: vec![
                    "aws_access_key_id".into(),
                    "aws_secret_access_key".into(),
                    "cloudflare_token".into(),
                ],
            },
        ))
        .await
        .expect("create destination");

    let found = fetch_destination(&fixture, &token, &org, &dest).await;

    let mut withheld = found.sensitive_keys.clone();
    withheld.sort();
    assert_eq!(
        withheld,
        vec!["aws_access_key_id", "aws_secret_access_key", "cloudflare_token"]
    );

    // Declared-benign keys keep their values, including the one DATA-575
    // explicitly called fine to show.
    assert_eq!(
        found.metadata.get("aws_account_id").map(String::as_str),
        Some("123456789012")
    );
    assert_eq!(
        found.metadata.get("infra_environment").map(String::as_str),
        Some("dev")
    );
    assert_eq!(
        found.metadata.get("tf_workspace").map(String::as_str),
        Some("platform-dev")
    );

    let serialised = format!("{found:?}");
    for secret in ["AKIAEXAMPLE", "wJalrXUtn", "cf_live_token"] {
        assert!(
            !serialised.contains(secret),
            "credential {secret} survived onto the wire: {serialised}"
        );
    }
}

/// Reveal is per-key: asking for one key returns that key only.
#[tokio::test(flavor = "multi_thread")]
async fn reveal_returns_one_requested_value() {
    let fixture = fixture().await.expect("fixture");
    let token = register_user(&fixture).await;
    let org = create_org(&fixture, &token).await;
    let env = create_env(&fixture, &token, &org).await;
    let dest = format!("tf-{}", uuid::Uuid::now_v7());

    fixture
        .destinations()
        .create_destination(authed_request(
            &token,
            CreateDestinationRequest {
                organisation: org.clone(),
                name: dest.clone(),
                environment: env,
                metadata: [
                    ("cloudflare_token".to_string(), "cf_live_token".to_string()),
                    ("aws_secret_access_key".to_string(), "wJalrXUtn".to_string()),
                ]
                .into(),
                r#type: Some(terraform_type()),
                sensitive_keys: vec![
                    "cloudflare_token".into(),
                    "aws_secret_access_key".into(),
                ],
            },
        ))
        .await
        .expect("create destination");

    let revealed = fixture
        .destinations()
        .reveal_destination_metadata(authed_request(
            &token,
            RevealDestinationMetadataRequest {
                organisation: org.clone(),
                name: dest.clone(),
                key: "cloudflare_token".into(),
            },
        ))
        .await
        .expect("reveal")
        .into_inner();

    assert_eq!(revealed.key, "cloudflare_token");
    assert_eq!(revealed.value, "cf_live_token");

    // Revealing one key says nothing about the others.
    let still_hidden = fetch_destination(&fixture, &token, &org, &dest).await;
    assert!(
        !format!("{still_hidden:?}").contains("wJalrXUtn"),
        "the other credential must stay withheld"
    );

    // Unknown keys are a not-found, not an empty string.
    let missing = fixture
        .destinations()
        .reveal_destination_metadata(authed_request(
            &token,
            RevealDestinationMetadataRequest {
                organisation: org.clone(),
                name: dest.clone(),
                key: "no_such_key".into(),
            },
        ))
        .await;
    assert_eq!(
        missing.expect_err("must not succeed").code(),
        tonic::Code::NotFound
    );
}

/// A member of another org cannot reveal a credential.
#[tokio::test(flavor = "multi_thread")]
async fn reveal_is_denied_across_organisations() {
    let fixture = fixture().await.expect("fixture");
    let owner_token = register_user(&fixture).await;
    let org = create_org(&fixture, &owner_token).await;
    let env = create_env(&fixture, &owner_token, &org).await;
    let dest = format!("tf-{}", uuid::Uuid::now_v7());

    fixture
        .destinations()
        .create_destination(authed_request(
            &owner_token,
            CreateDestinationRequest {
                organisation: org.clone(),
                name: dest.clone(),
                environment: env,
                metadata: [("cloudflare_token".to_string(), "cf_live_token".to_string())].into(),
                r#type: Some(terraform_type()),
                sensitive_keys: vec!["cloudflare_token".into()],
            },
        ))
        .await
        .expect("create destination");

    let outsider_token = register_user(&fixture).await;

    let result = fixture
        .destinations()
        .reveal_destination_metadata(authed_request(
            &outsider_token,
            RevealDestinationMetadataRequest {
                organisation: org.clone(),
                name: dest.clone(),
                key: "cloudflare_token".into(),
            },
        ))
        .await;

    let err = result.expect_err("outsider must not reveal a credential");
    assert!(
        matches!(err.code(), tonic::Code::PermissionDenied | tonic::Code::NotFound),
        "unexpected status: {err:?}"
    );
    assert!(
        !format!("{err:?}").contains("cf_live_token"),
        "error must not carry the value"
    );

    // Unauthenticated calls are rejected too.
    let unauthed = fixture
        .destinations()
        .reveal_destination_metadata(tonic::Request::new(RevealDestinationMetadataRequest {
            organisation: org,
            name: dest,
            key: "cloudflare_token".into(),
        }))
        .await;
    assert!(unauthed.is_err(), "unauthenticated reveal must be rejected");
}

/// `update` leaves the sensitive-key set alone unless the client says otherwise,
/// so a metadata-only edit cannot accidentally unhide a credential.
#[tokio::test(flavor = "multi_thread")]
async fn update_preserves_sensitive_keys_unless_explicitly_set() {
    let fixture = fixture().await.expect("fixture");
    let token = register_user(&fixture).await;
    let org = create_org(&fixture, &token).await;
    let env = create_env(&fixture, &token, &org).await;
    let dest = format!("tf-{}", uuid::Uuid::now_v7());

    fixture
        .destinations()
        .create_destination(authed_request(
            &token,
            CreateDestinationRequest {
                organisation: org.clone(),
                name: dest.clone(),
                environment: env,
                metadata: [("cloudflare_token".to_string(), "cf_live_token".to_string())].into(),
                r#type: Some(terraform_type()),
                sensitive_keys: vec!["cloudflare_token".into()],
            },
        ))
        .await
        .expect("create destination");

    // Metadata-only update, as an older client would send it.
    fixture
        .destinations()
        .update_destination(authed_request(
            &token,
            UpdateDestinationRequest {
                organisation: org.clone(),
                name: dest.clone(),
                metadata: [
                    ("cloudflare_token".to_string(), "cf_rotated".to_string()),
                    ("tf_workspace".to_string(), "platform-dev".to_string()),
                ]
                .into(),
                sensitive_keys: vec![],
                set_sensitive_keys: false,
            },
        ))
        .await
        .expect("update destination");

    let found = fetch_destination(&fixture, &token, &org, &dest).await;
    assert_eq!(found.sensitive_keys, vec!["cloudflare_token".to_string()]);
    assert!(!format!("{found:?}").contains("cf_rotated"));

    // Now explicitly narrow the set; the key becomes visible again.
    fixture
        .destinations()
        .update_destination(authed_request(
            &token,
            UpdateDestinationRequest {
                organisation: org.clone(),
                name: dest.clone(),
                metadata: [("cloudflare_token".to_string(), "cf_rotated".to_string())].into(),
                sensitive_keys: vec![],
                set_sensitive_keys: true,
            },
        ))
        .await
        .expect("update destination");

    let found = fetch_destination(&fixture, &token, &org, &dest).await;
    assert!(found.sensitive_keys.is_empty());
    assert_eq!(
        found.metadata.get("cloudflare_token").map(String::as_str),
        Some("cf_rotated")
    );
}

/// The type schema itself must carry the flag to clients, so a UI can render a
/// masked input before any destination exists.
#[tokio::test(flavor = "multi_thread")]
async fn list_destination_types_reports_which_fields_are_sensitive() {
    let fixture = fixture().await.expect("fixture");
    let token = register_user(&fixture).await;

    let types = fixture
        .destinations()
        .list_destination_types(authed_request(&token, ListDestinationTypesRequest {}))
        .await
        .expect("list destination types")
        .into_inner()
        .types;

    let flux = types
        .iter()
        .find(|t| t.organisation == "forest" && t.name == "flux")
        .expect("flux type present");

    let sensitive: Vec<&str> = {
        let mut v: Vec<&str> = flux
            .fields
            .iter()
            .filter(|f| f.sensitive)
            .map(|f| f.name.as_str())
            .collect();
        v.sort();
        v
    };
    assert_eq!(sensitive, vec!["git_token", "reconcile_url", "webhook_secret"]);

    // And plain config fields are not swept up.
    assert!(
        flux.fields
            .iter()
            .any(|f| f.name == "cluster_name" && !f.sensitive),
        "cluster_name must stay non-sensitive"
    );
}
