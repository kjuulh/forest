//! The terraform@1 state backend refuses anything without a credential signed
//! for that exact state (understory-io/forest#281).
//!
//! Driven through the real router in-process against the test database, so
//! these exercise the handlers, the credential check and the lock table
//! together — the same path a `tofu init`/`plan`/`apply` takes.

use axum::body::{Body, to_bytes};
use base64::Engine as _;
use forest_server::destinations::terraform_credentials::Scope;
use forest_server::destinations::terraformv1::{TerraformStateStoreState, TerraformV1Server};
use http::{Method, Request, StatusCode};
use tower::ServiceExt;

use super::fixtures::fixture;

fn state_id() -> String {
    format!("test.{}", uuid::Uuid::new_v4())
}

fn basic(password: &str) -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("forest-terraform-v1:{password}"))
    )
}

async fn call(
    state: &forest_server::State,
    method: Method,
    uri: &str,
    auth: Option<&str>,
    body: &str,
) -> (StatusCode, http::HeaderMap, String) {
    // As terraform's HTTP backend sends it.
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(http::header::CONTENT_TYPE, "application/json");
    if let Some(auth) = auth {
        req = req.header(http::header::AUTHORIZATION, basic(auth));
    }
    let response = TerraformV1Server::router(state.clone())
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

fn operator(state: &forest_server::State, id: &str, scope: Scope) -> String {
    state
        .terraform_state_store()
        .operator_credential(id, scope, "user:test", std::time::Duration::from_secs(300))
        .0
}

#[tokio::test(flavor = "multi_thread")]
async fn every_route_refuses_a_request_with_no_credential() {
    let f = fixture().await.unwrap();
    let id = state_id();
    let lock = r#"{"ID":"l1"}"#;

    for (method, uri, body) in [
        (Method::GET, format!("/{id}"), ""),
        (Method::POST, format!("/{id}?ID=l1"), "{}"),
        (Method::POST, format!("/{id}/lock"), lock),
        (Method::POST, format!("/{id}/unlock"), lock),
    ] {
        let (status, headers, _) = call(&f.state, method.clone(), &uri, None, body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
        assert!(
            headers.contains_key(http::header::WWW_AUTHENTICATE),
            "{method} {uri} should challenge"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_credential_for_another_state_is_refused() {
    let f = fixture().await.unwrap();
    let mine = state_id();
    let theirs = state_id();
    let cred = operator(&f.state, &mine, Scope::ReadWrite);

    let (status, _, _) = call(
        &f.state,
        Method::GET,
        &format!("/{theirs}"),
        Some(&cred),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // And the old shape of password — a random UUID — opens nothing.
    let (status, _, _) = call(
        &f.state,
        Method::GET,
        &format!("/{mine}"),
        Some(&uuid::Uuid::new_v4().to_string()),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_read_only_credential_reads_and_cannot_write_lock_or_unlock() {
    let f = fixture().await.unwrap();
    let id = state_id();
    let ro = operator(&f.state, &id, Scope::ReadOnly);

    // Authorised to read; there is simply no state yet.
    let (status, _, _) = call(&f.state, Method::GET, &format!("/{id}"), Some(&ro), "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    for (uri, body) in [
        (format!("/{id}?ID=l1"), "{}"),
        (format!("/{id}/lock"), r#"{"ID":"l1"}"#),
        (format!("/{id}/unlock"), r#"{"ID":"l1"}"#),
    ] {
        let (status, _, _) = call(&f.state, Method::POST, &uri, Some(&ro), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_read_write_credential_does_the_whole_terraform_cycle() {
    let f = fixture().await.unwrap();
    let id = state_id();
    let rw = operator(&f.state, &id, Scope::ReadWrite);
    let state_body = r#"{"version":4,"serial":1,"resources":[]}"#;

    let (status, _, _) = call(
        &f.state,
        Method::POST,
        &format!("/{id}/lock"),
        Some(&rw),
        r#"{"ID":"first"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "lock");

    let (status, _, _) = call(
        &f.state,
        Method::POST,
        &format!("/{id}?ID=first"),
        Some(&rw),
        state_body,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "write under the lock");

    let (status, _, body) = call(&f.state, Method::GET, &format!("/{id}"), Some(&rw), "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, state_body);

    // A second locker is told who holds it, as terraform's LockInfo — which is
    // what an operator needs for `terraform force-unlock <ID>`.
    let (status, _, body) = call(
        &f.state,
        Method::POST,
        &format!("/{id}/lock"),
        Some(&rw),
        r#"{"ID":"second"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let info: serde_json::Value = serde_json::from_str(&body).expect("lock info json");
    assert_eq!(info["ID"], "first");

    let (status, _, _) = call(
        &f.state,
        Method::POST,
        &format!("/{id}/unlock"),
        Some(&rw),
        r#"{"ID":"first"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "unlock");
}

#[tokio::test(flavor = "multi_thread")]
async fn force_unlock_breaks_a_lock_whoever_holds_it() {
    let f = fixture().await.unwrap();
    let id = state_id();
    let rw = operator(&f.state, &id, Scope::ReadWrite);

    call(
        &f.state,
        Method::POST,
        &format!("/{id}/lock"),
        Some(&rw),
        r#"{"ID":"crashed-runner"}"#,
    )
    .await;

    let store = f.state.terraform_state_store();
    assert_eq!(
        store.force_unlock(&id).await.unwrap().as_deref(),
        Some("crashed-runner")
    );
    assert_eq!(store.force_unlock(&id).await.unwrap(), None, "idempotent");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_runner_credential_is_accepted_for_its_own_state() {
    // What `TerraformV1Destination::run` hands to tofu.
    let f = fixture().await.unwrap();
    let id = state_id();
    let destination = forest_models::Destination {
        organisation: "understory".into(),
        name: "dev/eu-west-1/core".into(),
        environment: "dev".into(),
        metadata: Default::default(),
        sensitive_keys: vec![],
        destination_type: forest_models::DestinationType {
            organisation: "forest".into(),
            name: "terraform".into(),
            version: 1,
            description: String::new(),
            fields: vec![],
        },
    };
    let cred = f
        .state
        .terraform_state_store()
        .runner_credential(&id, &destination);

    let (status, _, _) = call(
        &f.state,
        Method::POST,
        &format!("/{id}/lock"),
        Some(&cred),
        r#"{"ID":"run"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    f.state
        .terraform_state_store()
        .force_unlock(&id)
        .await
        .unwrap();
}

/// Real OpenTofu against the real backend, through the credential: init, apply,
/// state list, a lock conflict that names its holder, force-unlock, and a
/// read-only credential that can plan but not apply.
///
/// Ignored by default because CI has no `tofu`; run with
/// `cargo test -p forest-server --test tests tofu_ -- --ignored`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs tofu on PATH"]
async fn tofu_drives_the_backend_with_a_forest_credential() {
    let f = fixture().await.unwrap();
    let id = state_id();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = TerraformV1Server::router(f.state.clone());
    let server = tokio::spawn(async move { axum::serve(listener, router).await });

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("main.tf"),
        "terraform {\n  backend \"http\" {}\n}\n\nresource \"terraform_data\" \"probe\" {\n  input = \"forest\"\n}\n",
    )
    .unwrap();

    let base = format!("http://{addr}/{id}");
    let tofu = |password: &str, args: &[&str]| {
        std::process::Command::new("tofu")
            .args(args)
            .arg("-no-color")
            .current_dir(dir.path())
            .env("TF_HTTP_ADDRESS", &base)
            .env("TF_HTTP_LOCK_ADDRESS", format!("{base}/lock"))
            .env("TF_HTTP_UNLOCK_ADDRESS", format!("{base}/unlock"))
            .env("TF_HTTP_LOCK_METHOD", "POST")
            .env("TF_HTTP_UNLOCK_METHOD", "POST")
            .env("TF_HTTP_USERNAME", "forest-terraform-v1")
            .env("TF_HTTP_PASSWORD", password)
            .env("TF_IN_AUTOMATION", "1")
            .output()
            .expect("run tofu")
    };
    let ok = |out: &std::process::Output, what: &str| {
        assert!(
            out.status.success(),
            "{what} failed:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    };
    let text = |out: &std::process::Output| {
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    };

    let rw = operator(&f.state, &id, Scope::ReadWrite);
    let ro = operator(&f.state, &id, Scope::ReadOnly);

    ok(&tofu(&rw, &["init", "-input=false"]), "init");
    ok(
        &tofu(&rw, &["apply", "-auto-approve", "-input=false"]),
        "apply",
    );
    let listed = tofu(&rw, &["state", "list"]);
    ok(&listed, "state list");
    assert!(
        text(&listed).contains("terraform_data.probe"),
        "{}",
        text(&listed)
    );

    // A wrong credential cannot even read.
    let other = operator(&f.state, &state_id(), Scope::ReadWrite);
    assert!(!tofu(&other, &["state", "list"]).status.success());

    // Read-only: a plan without locking works, an apply does not.
    ok(
        &tofu(&ro, &["plan", "-lock=false", "-input=false"]),
        "read-only plan",
    );
    assert!(
        !tofu(&ro, &["apply", "-auto-approve", "-input=false"])
            .status
            .success(),
        "a read-only credential must not apply"
    );

    // A crashed run's lock: tofu is told the holder's ID...
    sqlx::query(
        "INSERT INTO terraform_state_locks (state_id, lock_id) VALUES ($1, 'crashed-apply')",
    )
    .bind(&id)
    .execute(&f.db)
    .await
    .unwrap();
    let blocked = tofu(&rw, &["plan", "-input=false", "-lock-timeout=0s"]);
    assert!(!blocked.status.success());
    assert!(
        text(&blocked).contains("crashed-apply"),
        "{}",
        text(&blocked)
    );

    // ...and `force-unlock <ID>` with a read-write credential clears it.
    ok(
        &tofu(&rw, &["force-unlock", "-force", "crashed-apply"]),
        "force-unlock",
    );
    ok(&tofu(&rw, &["plan", "-input=false"]), "plan after unlock");

    server.abort();
}
