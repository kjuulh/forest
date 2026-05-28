//! Authz coverage backstop.
//!
//! Defensive pattern: **every** gRPC handler in `src/grpc/*.rs` must
//! either call `authorize::*` (or a documented helper) somewhere in its
//! body, OR be listed by name in [`EXEMPT`] below with a comment
//! explaining why.
//!
//! This is the "opt out, not allow list" mechanism the team agreed on
//! after the destinations / release_pipelines / event_subscriptions
//! bugs: rather than relying on every reviewer to spot a missing
//! `require_org_access`, we make missing-authz a build failure unless
//! the author writes down the exemption.
//!
//! The check is intentionally text-based. It is not a sound
//! verification; it cannot stop a handler that calls `authorize::` but
//! against the wrong org. It is the cheapest possible safety net for
//! the most common failure mode we've actually shipped (no authz at
//! all). For deeper guarantees, see the per-resource tests in
//! `authz_flow.rs`.

use std::fs;
use std::path::{Path, PathBuf};

/// Handlers that legitimately do not need an `authorize::*` call.
/// Each entry MUST have a comment explaining why. Adding a handler
/// here is the explicit opt-out — code review must justify it.
const EXEMPT: &[(&str, &str)] = &[
    // ─── UsersService ────────────────────────────────────────────────
    ("users.rs::register", "anonymous account creation"),
    ("users.rs::login", "anonymous credential check"),
    ("users.rs::verify_login_mfa", "completes the login flow, no actor yet"),
    ("users.rs::refresh_token", "validates the refresh token itself, no actor needed"),
    ("users.rs::logout", "self-scoped via the refresh token in the request"),
    ("users.rs::token_info", "introspects the caller's own token"),
    ("users.rs::get_user", "public lookup by id/username/email — also used by login flows"),
    ("users.rs::update_user", "self-scoped — handler validates user_id matches actor"),
    ("users.rs::delete_user", "self-scoped — handler validates user_id matches actor"),
    ("users.rs::list_users", "admin-only via separate path-based check (see auth_layer)"),
    ("users.rs::add_email", "self-scoped — user_id parsed from request matches actor"),
    ("users.rs::remove_email", "self-scoped — user_id matches actor"),
    ("users.rs::list_personal_access_tokens", "self-scoped — user_id matches actor"),
    ("users.rs::create_personal_access_token", "self-scoped"),
    ("users.rs::delete_personal_access_token", "self-scoped"),
    ("users.rs::o_auth_login", "anonymous OAuth callback handler"),
    ("users.rs::link_o_auth_provider", "self-scoped — links to caller's user (handler must enforce)"),
    ("users.rs::unlink_o_auth_provider", "self-scoped — unlinks from caller's user (handler must enforce)"),
    ("users.rs::change_password", "self-scoped — current password proves identity"),
    ("users.rs::setup_mfa", "self-scoped"),
    ("users.rs::verify_mfa", "completes login challenge, no actor yet"),
    ("users.rs::disable_mfa", "self-scoped"),
    ("users.rs::get_user_stats", "self-scoped via the actor"),
    ("users.rs::verify_email", "email-verification token authenticates the action"),
    ("users.rs::confirm_email_verification", "service-account internal call, scoped at the route"),
    ("users.rs::initiate_device_login", "RFC 8628: CLI has no token yet — by design"),
    ("users.rs::poll_device_login", "RFC 8628: device_code is the bearer; auth happens out-of-band in browser"),

    // ─── OrganisationService ─────────────────────────────────────────
    ("organisations.rs::create_organisation", "anyone authenticated may create their own org"),
    ("organisations.rs::get_organisation", "public org lookup — names are public"),
    ("organisations.rs::search_organisations", "public org search"),
    ("organisations.rs::list_my_organisations", "scoped to caller via AppClaims.user_id"),
    ("organisations.rs::add_member", "scoped via organisation_id in request — handler must enforce"),
    ("organisations.rs::remove_member", "scoped via organisation_id in request — handler must enforce"),
    ("organisations.rs::update_member_role", "scoped via organisation_id in request — handler must enforce"),
    // DATA-252 — auto-invite. Both RPCs are self-scoped: the user_id is
    // taken from AppClaims, the org_id in the request is only used to
    // filter results that are already constrained by that user_id. The
    // accept_join_offer path re-validates eligibility inside the service
    // tx, so a forged org_id grants no access.
    ("organisations.rs::list_join_offers", "self-scoped via AppClaims.user_id"),
    ("organisations.rs::accept_join_offer", "self-scoped via AppClaims.user_id; service re-checks eligibility"),

    // ─── NotificationService ─────────────────────────────────────────
    // Notifications are entirely user-self-scoped; the handler module
    // uses its own `extract_actor_id` helper that takes the user_id
    // from the actor extension. No org/project scope exists.
    ("notifications.rs::list_notifications", "self-scoped via extract_actor_id"),
    ("notifications.rs::listen_notifications", "self-scoped via extract_actor_id"),
    ("notifications.rs::get_notification_preferences", "self-scoped"),
    ("notifications.rs::set_notification_preference", "self-scoped"),

    // ─── RunnerService ───────────────────────────────────────────────
    // Runners authenticate with release-scoped tokens validated in
    // auth_layer, not the JWT/Actor pipeline. Their authz model is
    // "the token only unlocks the release_id it was minted for", which
    // is checked inside the handler against the release context.
    ("runner.rs::register_runner", "release-scoped runner token, validated upstream"),
    ("runner.rs::get_release_files", "release-scoped runner token"),
    ("runner.rs::get_spec_files", "release-scoped runner token"),
    ("runner.rs::get_release_annotation", "release-scoped runner token"),
    ("runner.rs::get_project_info", "release-scoped runner token"),
    ("runner.rs::push_logs", "release-scoped runner token"),
    ("runner.rs::complete_release", "release-scoped runner token"),

    // ─── StatusService ───────────────────────────────────────────────
    ("status.rs::status", "unauthenticated liveness probe"),

    // ─── RegistryService — public listings ──────────────────────────
    // These read-only RPCs intentionally allow anonymous callers (set
    // to AuthMode::Optional in auth_layer) so unauthenticated users can
    // browse public components. Private components are filtered by the
    // handler if no actor is present.
    ("registry.rs::search_components", "public browse + optional actor for private filter"),
    ("registry.rs::get_component_detail", "public component detail + optional actor"),
    // Pre-existing drift caught by this DATA-252 PR: the public-only RPCs
    // introduced by PR #48 were never added to EXEMPT. Same shape as
    // search_components — anonymous-allowed, public-by-name.
    ("registry.rs::search_public_components", "public browse (public-only RPC, no actor required)"),
    ("registry.rs::get_public_component_detail", "public detail (public-only RPC, no actor required)"),
    ("registry.rs::get_public_component_manifest", "public manifest (public-only RPC, no actor required)"),

    // ─── DestinationService ─────────────────────────────────────────
    ("destinations.rs::list_destination_types", "static metadata, no org context"),

    // ─── ReleaseService — public/cross-org reads ────────────────────
    // Org names are public (used to populate the org picker in the UI).
    ("release.rs::get_organisations", "public list of org names for UI dropdowns"),
];

fn exempt_set() -> std::collections::HashSet<(&'static str, &'static str)> {
    EXEMPT.iter().copied().map(|(k, _)| (k.split("::").next().unwrap(), k.split("::").nth(1).unwrap())).collect()
}

fn grpc_dir() -> PathBuf {
    // tests run from the crate root
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src").join("grpc")
}

/// Files in src/grpc/ that aren't gRPC service impls (helpers, layers, etc.)
fn is_service_file(name: &str) -> bool {
    !matches!(
        name,
        "mod.rs"
            | "auth_layer.rs"
            | "authorize.rs"
            | "error.rs"
            | "log_layer.rs"
            | "artifacts.rs" // helper module, no service impl
    )
}

#[derive(Debug)]
struct Handler {
    file: String,
    method: String,
}

/// Extract every `async fn <ident>(` block inside the file, paired with
/// its body. Returns (method_name, body_text) tuples.
fn parse_handlers(source: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = source;
    while let Some(idx) = rest.find("async fn ") {
        let after = &rest[idx + "async fn ".len()..];
        let name_end = after
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .unwrap_or(after.len());
        let name = after[..name_end].to_string();

        // Find the opening brace of the body. Skip past parameter list
        // and return type by counting parens.
        let mut depth = 0usize;
        let mut i = name_end;
        let bytes = after.as_bytes();
        let mut body_start = None;
        while i < bytes.len() {
            match bytes[i] {
                b'(' => depth += 1,
                b')' => {
                    if depth > 0 {
                        depth -= 1;
                    }
                }
                b'{' if depth == 0 => {
                    body_start = Some(i);
                    break;
                }
                _ => {}
            }
            i += 1;
        }
        let Some(bs) = body_start else {
            break;
        };

        // Find the matching close brace.
        let mut brace_depth = 0i32;
        let mut j = bs;
        let mut body_end = None;
        while j < bytes.len() {
            match bytes[j] {
                b'{' => brace_depth += 1,
                b'}' => {
                    brace_depth -= 1;
                    if brace_depth == 0 {
                        body_end = Some(j);
                        break;
                    }
                }
                _ => {}
            }
            j += 1;
        }
        let Some(be) = body_end else {
            break;
        };

        let body = &after[bs..=be];
        out.push((name, body.to_string()));
        rest = &after[be + 1..];
    }
    out
}

fn handler_is_authorized(body: &str) -> bool {
    // Any of these tokens means the handler made an authorization
    // decision. The text scan is deliberately broad — false positives
    // are fine here, false negatives are the danger.
    body.contains("authorize::")
        || body.contains("authorize_upload")
        || body.contains("authorize_component")
        || body.contains("extract_actor_id") // notifications.rs helper
}

#[test]
fn every_grpc_handler_either_authorizes_or_is_explicitly_exempt() {
    let dir = grpc_dir();
    let exempt = exempt_set();
    let mut missing: Vec<Handler> = Vec::new();

    for entry in fs::read_dir(&dir).expect("read src/grpc") {
        let entry = entry.unwrap();
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".rs") || !is_service_file(name) {
            continue;
        }

        let source = fs::read_to_string(&path).expect("read service file");

        for (method, body) in parse_handlers(&source) {
            // Skip helper functions inside service files. Tonic service
            // methods take `tonic::Request<...>`; helpers usually don't.
            // This filter intentionally narrows to handler-shaped fns.
            if !body.contains("tonic::Request<") && !body.contains("tonic::Streaming<") {
                // Also accept handlers whose surrounding signature lives
                // earlier; recheck by looking at the call site in the
                // file for `request: tonic::Request` paired with this name.
                let sig_pattern = format!("async fn {method}(");
                let sig_idx = source.find(&sig_pattern);
                let sig_has_request = sig_idx
                    .and_then(|i| source.get(i..i + 600))
                    .map(|chunk| {
                        chunk.contains("tonic::Request<") || chunk.contains("tonic::Streaming<")
                    })
                    .unwrap_or(false);
                if !sig_has_request {
                    continue;
                }
            }

            if exempt.contains(&(name, method.as_str())) {
                continue;
            }
            if !handler_is_authorized(&body) {
                missing.push(Handler {
                    file: name.to_string(),
                    method,
                });
            }
        }
    }

    if !missing.is_empty() {
        let mut msg = String::from(
            "\nSECURITY: gRPC handler(s) appear to perform no authorization.\n\
             Each handler in src/grpc/*.rs must either call authorize::*\n\
             (require_org_access / require_project_access / etc.) OR be\n\
             listed in the EXEMPT array of authz_coverage.rs with a reason.\n\n\
             Missing:\n",
        );
        for h in &missing {
            msg.push_str(&format!("  - {}::{}\n", h.file, h.method));
        }
        panic!("{msg}");
    }
}

/// Make sure EXEMPT doesn't drift out of date — every exempted handler
/// must still exist in the source.
#[test]
fn every_exempt_entry_points_at_a_real_handler() {
    let dir = grpc_dir();
    let mut dangling = Vec::new();

    // Build {file -> set<method>}
    let mut by_file: std::collections::HashMap<String, std::collections::HashSet<String>> =
        std::collections::HashMap::new();
    for entry in fs::read_dir(&dir).expect("read src/grpc") {
        let path = entry.unwrap().path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".rs") {
            continue;
        }
        let source = fs::read_to_string(&path).unwrap_or_default();
        let methods: std::collections::HashSet<String> = parse_handlers(&source)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        by_file.insert(name.to_string(), methods);
    }

    for (key, _reason) in EXEMPT {
        let (file, method) = key
            .split_once("::")
            .unwrap_or_else(|| panic!("EXEMPT entry '{key}' missing '::'"));
        let exists = by_file
            .get(file)
            .map(|set| set.contains(method))
            .unwrap_or(false);
        if !exists {
            dangling.push(*key);
        }
    }

    if !dangling.is_empty() {
        panic!(
            "EXEMPT contains entries that no longer exist in src/grpc/:\n  {}\n\
             Remove them or rename so the audit list stays accurate.",
            dangling.join("\n  ")
        );
    }
}

/// Sanity: handler-coverage and exempt-validation tests above use the
/// same parser, so make sure the parser actually finds *something*. A
/// silent zero would be a regression.
#[test]
fn parser_finds_handlers() {
    let dir = grpc_dir();
    let mut total = 0;
    for entry in fs::read_dir(&dir).unwrap() {
        let p = entry.unwrap().path();
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".rs") {
            continue;
        }
        let src = fs::read_to_string(&p).unwrap_or_default();
        total += parse_handlers(&src).len();
    }
    assert!(
        total > 30,
        "parser found only {total} async fns across src/grpc — it likely regressed"
    );
}
