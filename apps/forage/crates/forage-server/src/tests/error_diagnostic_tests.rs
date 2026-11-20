use super::redact_diagnostic;

#[test]
fn diagnostic_chains_keep_operational_context_without_known_credential_formats() {
    let diagnostic = r#"database request failed: SQLSTATE 42P01 at db.internal.test;
password=canary_password password='canary spaced password'
{\"client_secret\":\"canary_json_secret\",\"access_token\":\"canary_json_token\"}
authorization: Bearer canary_bearer
Authorization: Basic Y2FuYXJ5OmJhc2lj
cookie: session=canary_cookie;
postgresql://operator:canary_url_password@db.internal.test:5432/private?token=canary_query#canary_fragment
https://canary_webhook_token@hooks.internal.test/secret/canary_path?key=canary_key
forest_oat_canary_opaque ghp_canary_github xoxb-canary-slack eyJjYW5hcnk.eyJ0b2tlbiI.canary_jwt
caused by: internal gRPC dependency failure"#;
    let safe = redact_diagnostic(diagnostic.into());
    for secret in [
        "canary_password",
        "canary spaced password",
        "canary_json_secret",
        "canary_json_token",
        "canary_bearer",
        "Y2FuYXJ5OmJhc2lj",
        "canary_cookie",
        "canary_url_password",
        "canary_query",
        "canary_fragment",
        "canary_webhook_token",
        "canary_path",
        "canary_key",
        "canary_opaque",
        "canary_github",
        "canary-slack",
        "canary_jwt",
    ] {
        assert!(
            !safe.contains(secret),
            "credential marker survived redaction: {secret}"
        );
    }
    assert!(safe.contains("SQLSTATE 42P01"));
    assert!(safe.contains("db.internal.test"));
    assert!(safe.contains("caused by: internal gRPC dependency failure"));
}

#[test]
fn quoted_secret_fields_with_spaces_and_case_variations_are_redacted() {
    for diagnostic in [
        r#"{"PASSWORD": "canary quoted value"}"#,
        "client_secret='canary quoted value'",
        "refresh-token = canary_value",
        "api-key: canary_value",
        "AWS_SECRET_ACCESS_KEY=canary_aws_secret",
        "AWS_ACCESS_KEY_ID=canary_aws_id",
        "Set-Cookie: session=canary_value; HttpOnly",
        "https://operator:canary_value@internal.test/path",
    ] {
        assert!(!redact_diagnostic(diagnostic.into()).contains("canary"));
    }
}
