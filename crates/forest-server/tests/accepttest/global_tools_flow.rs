//! Acceptance tests for the global-tools registry contract.
//!
//! Exercises TASKS/018-global-tools.md §1a.2 (server-side validation) and
//! §1a.2e (shape persistence) end-to-end against the live gRPC server +
//! Postgres dev database. Each test creates a unique org, publishes a
//! manifest, and inspects the server's stored state.

use forest_grpc_interface::*;
use serde_json::json;
use tonic::metadata::MetadataValue;

use crate::accepttest::fixtures::{Given, testcase};

// ---------------------------------------------------------------------------
// Shared scaffolding
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct GlobalToolsData {
    auth_token: String,
    organisation: String,
    component_name: String,
    component_version: String,
    upload_context: String,
}

fn authed_request<T>(token: &str, inner: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(inner);
    let val: MetadataValue<_> = format!("Bearer {token}")
        .parse()
        .expect("valid metadata value");
    req.metadata_mut().insert("authorization", val);
    req
}

async fn register_user(g: &Given<GlobalToolsData>) -> String {
    let mut users = g.fixture().users();
    let username = format!("gt-{}", uuid::Uuid::now_v7());
    let email = format!("gt-{}@example.com", uuid::Uuid::now_v7());
    let resp = users
        .register(RegisterRequest {
            username,
            email,
            password: "TestPassword123!".into(),
        })
        .await
        .expect("register");
    resp.into_inner().tokens.expect("tokens").access_token
}

async fn create_org(g: &Given<GlobalToolsData>, token: &str, org: &str) {
    g.fixture()
        .organisations()
        .create_organisation(authed_request(
            token,
            CreateOrganisationRequest {
                name: org.to_string(),
            },
        ))
        .await
        .expect("create org");
}

async fn begin_upload(
    g: &Given<GlobalToolsData>,
    token: &str,
    org: &str,
    name: &str,
    version: &str,
) -> String {
    let resp = g
        .fixture()
        .registry()
        .begin_upload(authed_request(
            token,
            BeginUploadRequest {
                name: name.to_string(),
                organisation: org.to_string(),
                version: version.to_string(),
            },
        ))
        .await
        .expect("begin_upload");
    resp.into_inner().upload_context
}

async fn publish_manifest(
    g: &Given<GlobalToolsData>,
    token: &str,
    upload_context: &str,
    manifest_json: &str,
) -> Result<(), tonic::Status> {
    g.fixture()
        .registry()
        .publish_manifest(authed_request(
            token,
            PublishManifestRequest {
                upload_context: upload_context.to_string(),
                manifest_json: manifest_json.to_string(),
            },
        ))
        .await
        .map(|_| ())
}

/// Stream-upload a binary artifact for an in-flight upload.
/// Mirrors the protocol-level path that `forest components publish` uses.
async fn upload_binary(
    g: &Given<GlobalToolsData>,
    token: &str,
    upload_context: &str,
    os: &str,
    arch: &str,
    binary: &[u8],
) {
    use sha2::{Digest, Sha256};
    let sha = hex::encode(Sha256::digest(binary));

    let metadata_msg = UploadBinaryRequest {
        msg: Some(upload_binary_request::Msg::Metadata(UploadBinaryMetadata {
            upload_context: upload_context.to_string(),
            os: os.to_string(),
            arch: arch.to_string(),
            sha256: sha.clone(),
        })),
    };
    let chunk_msg = UploadBinaryRequest {
        msg: Some(upload_binary_request::Msg::Chunk(binary.to_vec())),
    };

    let stream = tokio_stream::iter(vec![metadata_msg, chunk_msg]);
    let mut req = tonic::Request::new(stream);
    let val: tonic::metadata::MetadataValue<_> =
        format!("Bearer {token}").parse().unwrap();
    req.metadata_mut().insert("authorization", val);
    g.fixture()
        .registry()
        .upload_binary(req)
        .await
        .expect("upload_binary");
}

async fn commit_upload(g: &Given<GlobalToolsData>, token: &str, upload_context: &str) {
    g.fixture()
        .registry()
        .commit_upload(authed_request(
            token,
            CommitUploadRequest {
                upload_context: upload_context.to_string(),
            },
        ))
        .await
        .expect("commit_upload");
}

fn unique_org() -> String {
    format!("gtorg{}", uuid::Uuid::now_v7().simple())
}

// ---------------------------------------------------------------------------
// Happy paths — one per shape
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn publish_tool_external_manifest_succeeds_and_persists_shape() {
    let (g, _w, _t) = testcase::<GlobalToolsData>().await.unwrap();

    let token = register_user(&g).await;
    let org = unique_org();
    create_org(&g, &token, &org).await;

    let upload = begin_upload(&g, &token, &org, "ripgrep-ext", "14.1.1").await;

    let manifest = json!({
        "kind": "external",
        "tool": {"name": "rg", "argv_passthrough": true, "description": "fast grep"},
        "platforms": {
            "linux_amd64": {
                "sha256": "ad3a44e3d8b8a9d39c1f7b4d1a9b9e3a5e7c2f6c8b4f3a1d2e9c8b7a6e5d4c3b",
                "url": "https://github.com/BurntSushi/ripgrep/releases/download/14.1.1/ripgrep-14.1.1-x86_64-unknown-linux-musl.tar.gz",
                "archive": "tar.gz",
                "binary_in_archive": "ripgrep-14.1.1-x86_64-unknown-linux-musl/rg"
            }
        }
    });

    publish_manifest(&g, &token, &upload, &manifest.to_string())
        .await
        .expect("publish_manifest must succeed for valid external manifest");
    commit_upload(&g, &token, &upload).await;

    // Verify the manifest round-trips via GetComponentManifest.
    let resp = g
        .fixture()
        .registry()
        .get_component_manifest(authed_request(
            &token,
            GetComponentManifestRequest {
                organisation: org.clone(),
                name: "ripgrep-ext".into(),
                version: "14.1.1".into(),
            },
        ))
        .await
        .expect("get manifest");
    let returned: serde_json::Value =
        serde_json::from_str(&resp.into_inner().manifest_json).unwrap();
    assert_eq!(returned["kind"], "external");
    assert_eq!(returned["tool"]["name"], "rg");

    // Verify the shape persisted on `components`.
    let shape: String =
        sqlx::query_scalar("SELECT shape FROM components WHERE organisation = $1 AND name = $2")
            .bind(&org)
            .bind("ripgrep-ext")
            .fetch_one(&g.fixture().db)
            .await
            .expect("query shape");
    assert_eq!(shape, "tool_external");

    // And that `kind` is also set to "external".
    let kind: String =
        sqlx::query_scalar("SELECT kind FROM components WHERE organisation = $1 AND name = $2")
            .bind(&org)
            .bind("ripgrep-ext")
            .fetch_one(&g.fixture().db)
            .await
            .expect("query kind");
    assert_eq!(kind, "external");
}

#[tokio::test(flavor = "multi_thread")]
async fn publish_tool_binary_manifest_succeeds_and_derives_shape() {
    let (g, _w, _t) = testcase::<GlobalToolsData>().await.unwrap();

    let token = register_user(&g).await;
    let org = unique_org();
    create_org(&g, &token, &org).await;
    let upload = begin_upload(&g, &token, &org, "hello-tool", "0.1.0").await;

    let manifest = json!({
        "kind": "binary",
        "tool": {"name": "hello", "argv_passthrough": true},
        "platforms": {
            "linux_amd64": {
                "sha256": "4f9c3a4f9c3a4f9c3a4f9c3a4f9c3a4f9c3a4f9c3a4f9c3a4f9c3a4f9c3a4f9c"
            }
        }
    });

    publish_manifest(&g, &token, &upload, &manifest.to_string())
        .await
        .expect("publish_manifest for tool_binary");
    commit_upload(&g, &token, &upload).await;

    let shape: String =
        sqlx::query_scalar("SELECT shape FROM components WHERE organisation = $1 AND name = $2")
            .bind(&org)
            .bind("hello-tool")
            .fetch_one(&g.fixture().db)
            .await
            .expect("query shape");
    assert_eq!(shape, "tool_binary");
}

#[tokio::test(flavor = "multi_thread")]
async fn publish_hybrid_component_manifest_persists_hybrid_shape() {
    let (g, _w, _t) = testcase::<GlobalToolsData>().await.unwrap();
    let token = register_user(&g).await;
    let org = unique_org();
    create_org(&g, &token, &org).await;
    let upload = begin_upload(&g, &token, &org, "greet-hybrid", "0.1.0").await;

    let manifest = json!({
        "kind": "binary",
        "tool": {"name": "greet", "argv_passthrough": true},
        "methods": ["greet"],
        "platforms": {
            "linux_amd64": {
                "sha256": "7e21b87e21b87e21b87e21b87e21b87e21b87e21b87e21b87e21b87e21b87e21"
            }
        }
    });

    publish_manifest(&g, &token, &upload, &manifest.to_string())
        .await
        .expect("publish_manifest for hybrid");
    commit_upload(&g, &token, &upload).await;

    let shape: String =
        sqlx::query_scalar("SELECT shape FROM components WHERE organisation = $1 AND name = $2")
            .bind(&org)
            .bind("greet-hybrid")
            .fetch_one(&g.fixture().db)
            .await
            .expect("query shape");
    assert_eq!(shape, "hybrid_component");
}

// ---------------------------------------------------------------------------
// Negative cases — §1a.2 validation rules
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn rejects_external_manifest_without_tool_facet() {
    // §1a.2 rule 2: external ⇒ tool present.
    let (g, _w, _t) = testcase::<GlobalToolsData>().await.unwrap();
    let token = register_user(&g).await;
    let org = unique_org();
    create_org(&g, &token, &org).await;
    let upload = begin_upload(&g, &token, &org, "no-tool-external", "0.1.0").await;

    let manifest = json!({
        "kind": "external",
        "platforms": {
            "linux_amd64": {
                "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "url": "https://example.com/x",
                "archive": "none"
            }
        }
    });

    let err = publish_manifest(&g, &token, &upload, &manifest.to_string())
        .await
        .expect_err("must reject external without tool");
    assert!(
        err.message().contains("ExternalRequiresTool"),
        "expected ExternalRequiresTool, got: {}",
        err.message()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_binary_manifest_without_methods_or_tool() {
    // §1a.2 rule 7.
    let (g, _w, _t) = testcase::<GlobalToolsData>().await.unwrap();
    let token = register_user(&g).await;
    let org = unique_org();
    create_org(&g, &token, &org).await;
    let upload = begin_upload(&g, &token, &org, "empty-binary", "0.1.0").await;

    let manifest = json!({
        "kind": "binary",
        "platforms": {
            "linux_amd64": {"sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
        }
    });

    let err = publish_manifest(&g, &token, &upload, &manifest.to_string())
        .await
        .expect_err("must reject empty binary");
    assert!(
        err.message().contains("BinaryRequiresMethodsOrTool"),
        "got: {}",
        err.message()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_external_manifest_with_http_scheme() {
    // §1a.2 rule 4: external URLs must be https://.
    let (g, _w, _t) = testcase::<GlobalToolsData>().await.unwrap();
    let token = register_user(&g).await;
    let org = unique_org();
    create_org(&g, &token, &org).await;
    let upload = begin_upload(&g, &token, &org, "http-tool", "0.1.0").await;

    let manifest = json!({
        "kind": "external",
        "tool": {"name": "x", "argv_passthrough": true},
        "platforms": {
            "linux_amd64": {
                "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "url": "http://example.com/x",
                "archive": "none"
            }
        }
    });

    let err = publish_manifest(&g, &token, &upload, &manifest.to_string())
        .await
        .expect_err("must reject http://");
    assert!(
        err.message().contains("InvalidUrl"),
        "got: {}",
        err.message()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_external_manifest_with_methods_declared() {
    // §1a.2e: external manifests cannot declare methods (no describe).
    let (g, _w, _t) = testcase::<GlobalToolsData>().await.unwrap();
    let token = register_user(&g).await;
    let org = unique_org();
    create_org(&g, &token, &org).await;
    let upload = begin_upload(&g, &token, &org, "ext-with-methods", "0.1.0").await;

    let manifest = json!({
        "kind": "external",
        "tool": {"name": "x", "argv_passthrough": true},
        "methods": ["foo"],
        "platforms": {
            "linux_amd64": {
                "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "url": "https://example.com/x",
                "archive": "none"
            }
        }
    });

    let err = publish_manifest(&g, &token, &upload, &manifest.to_string())
        .await
        .expect_err("must reject external+methods");
    assert!(
        err.message().contains("ExternalCannotDeclareMethods"),
        "got: {}",
        err.message()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_invalid_tool_name() {
    // §1a.2 rule 3: tool.name regex.
    let (g, _w, _t) = testcase::<GlobalToolsData>().await.unwrap();
    let token = register_user(&g).await;
    let org = unique_org();
    create_org(&g, &token, &org).await;
    let upload = begin_upload(&g, &token, &org, "bad-name-tool", "0.1.0").await;

    let manifest = json!({
        "kind": "binary",
        "tool": {"name": "1bad", "argv_passthrough": true},
        "platforms": {
            "linux_amd64": {"sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
        }
    });

    let err = publish_manifest(&g, &token, &upload, &manifest.to_string())
        .await
        .expect_err("must reject invalid tool.name");
    assert!(
        err.message().contains("InvalidToolName"),
        "got: {}",
        err.message()
    );
}

async fn publish_tool(
    g: &Given<GlobalToolsData>,
    token: &str,
    org: &str,
    name: &str,
    version: &str,
    manifest: serde_json::Value,
) {
    let upload = begin_upload(g, token, org, name, version).await;
    publish_manifest(g, token, &upload, &manifest.to_string())
        .await
        .expect("publish_manifest");
    commit_upload(g, token, &upload).await;
}

// ---------------------------------------------------------------------------
// End-to-end: upload binary → lazy fetch via DownloadBinary → exec
// ---------------------------------------------------------------------------

/// The smallest valid Linux ELF that exits 0 — a minimal shell script that
/// happens to also satisfy `chmod +x` semantics. For the cache layer we
/// only need *bytes that round-trip with a stable sha*, not an executable
/// shape — the test asserts on cache content, not exec.
const STUB_BINARY: &[u8] = b"#!/bin/sh\necho hello, $1!\nexit 0\n";

#[tokio::test(flavor = "multi_thread")]
async fn end_to_end_tool_binary_upload_and_lazy_fetch() {
    // Verifies the full TOOL_BINARY lifecycle on the server side:
    //   publish → DownloadBinary → bytes match what we uploaded.
    // This is the contract the client's `resolve_to_cached_path` cold path
    // depends on.
    let (g, _w, _t) = testcase::<GlobalToolsData>().await.unwrap();
    let token = register_user(&g).await;
    let org = unique_org();
    create_org(&g, &token, &org).await;

    let upload = begin_upload(&g, &token, &org, "stub-tool", "0.1.0").await;
    upload_binary(&g, &token, &upload, "linux", "amd64", STUB_BINARY).await;

    use sha2::{Digest, Sha256};
    let expected_sha = hex::encode(Sha256::digest(STUB_BINARY));
    let manifest = json!({
        "kind": "binary",
        "tool": {"name": "stub", "argv_passthrough": true},
        "platforms": {
            "linux_amd64": {
                "sha256": expected_sha,
                "size": STUB_BINARY.len()
            }
        }
    });
    publish_manifest(&g, &token, &upload, &manifest.to_string())
        .await
        .expect("publish_manifest");
    commit_upload(&g, &token, &upload).await;

    // Now exercise the download path the way `forest global run` would.
    let mut stream = g
        .fixture()
        .registry()
        .download_binary(authed_request(
            &token,
            DownloadBinaryRequest {
                organisation: org.clone(),
                name: "stub-tool".into(),
                version: "0.1.0".into(),
                os: "linux".into(),
                arch: "amd64".into(),
            },
        ))
        .await
        .expect("download_binary")
        .into_inner();

    let mut fetched = Vec::new();
    while let Some(chunk) = stream.message().await.expect("download stream") {
        fetched.extend_from_slice(&chunk.chunk);
    }
    assert_eq!(fetched, STUB_BINARY);

    // Sha matches what we uploaded — the client cache layer's content-address
    // invariant (cache::finalize) accepts these bytes only if the manifest
    // claims this sha. End-to-end integrity verified.
    let downloaded_sha = hex::encode(Sha256::digest(&fetched));
    assert_eq!(downloaded_sha, expected_sha);

    // Shape persisted as tool_binary.
    let shape: String =
        sqlx::query_scalar("SELECT shape FROM components WHERE organisation = $1 AND name = $2")
            .bind(&org)
            .bind("stub-tool")
            .fetch_one(&g.fixture().db)
            .await
            .expect("query shape");
    assert_eq!(shape, "tool_binary");
}

#[tokio::test(flavor = "multi_thread")]
async fn end_to_end_hybrid_component_serves_methods_and_argv_passthrough() {
    // HYBRID_COMPONENT: same binary, two doorways. The describe protocol
    // (forest run <command>) reads `methods`; the shim path reads `tool`.
    // Server-side just stores everything in the manifest and the shape.
    let (g, _w, _t) = testcase::<GlobalToolsData>().await.unwrap();
    let token = register_user(&g).await;
    let org = unique_org();
    create_org(&g, &token, &org).await;

    let upload = begin_upload(&g, &token, &org, "greet-hyb", "0.2.0").await;
    upload_binary(&g, &token, &upload, "linux", "amd64", STUB_BINARY).await;

    use sha2::{Digest, Sha256};
    let sha = hex::encode(Sha256::digest(STUB_BINARY));
    let manifest = json!({
        "kind": "binary",
        "tool": {"name": "greet", "argv_passthrough": true, "description": "demo"},
        "methods": ["greet", "status"],
        "platforms": {
            "linux_amd64": {"sha256": sha, "size": STUB_BINARY.len()}
        }
    });
    publish_manifest(&g, &token, &upload, &manifest.to_string())
        .await
        .expect("publish_manifest");
    commit_upload(&g, &token, &upload).await;

    // Detail surface exposes both methods and tool facet.
    let detail = g
        .fixture()
        .registry()
        .get_component_detail(authed_request(
            &token,
            GetComponentDetailRequest {
                organisation: org.clone(),
                name: "greet-hyb".into(),
            },
        ))
        .await
        .expect("get_component_detail")
        .into_inner();
    let summary = detail.summary.expect("summary");
    assert_eq!(summary.shape, ComponentShape::Hybrid as i32);
    assert_eq!(summary.tool.as_ref().expect("tool").name, "greet");
    assert_eq!(summary.methods, vec!["greet".to_string(), "status".to_string()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn end_to_end_tool_external_no_binary_upload_required() {
    // TOOL_EXTERNAL: the manifest carries the URL + sha; the registry never
    // sees bytes. DownloadBinary will refuse (no artifact uploaded). The
    // client's resolver picks `FetchPlan::Url` instead.
    let (g, _w, _t) = testcase::<GlobalToolsData>().await.unwrap();
    let token = register_user(&g).await;
    let org = unique_org();
    create_org(&g, &token, &org).await;

    let upload = begin_upload(&g, &token, &org, "rg-ext", "14.1.1").await;
    let manifest = json!({
        "kind": "external",
        "tool": {"name": "rg", "argv_passthrough": true, "description": "ripgrep"},
        "platforms": {
            "linux_amd64": {
                "sha256": "ad3a44e3d8b8a9d39c1f7b4d1a9b9e3a5e7c2f6c8b4f3a1d2e9c8b7a6e5d4c3b",
                "url": "https://github.com/BurntSushi/ripgrep/releases/download/14.1.1/ripgrep-14.1.1-x86_64-unknown-linux-musl.tar.gz",
                "archive": "tar.gz",
                "binary_in_archive": "ripgrep-14.1.1-x86_64-unknown-linux-musl/rg"
            }
        }
    });
    publish_manifest(&g, &token, &upload, &manifest.to_string())
        .await
        .expect("publish external");
    commit_upload(&g, &token, &upload).await;

    // Fetching the manifest exposes the upstream URL for the client to GET.
    let resp = g
        .fixture()
        .registry()
        .get_component_manifest(authed_request(
            &token,
            GetComponentManifestRequest {
                organisation: org.clone(),
                name: "rg-ext".into(),
                version: "14.1.1".into(),
            },
        ))
        .await
        .expect("get_component_manifest");
    let v: serde_json::Value = serde_json::from_str(&resp.into_inner().manifest_json).unwrap();
    assert_eq!(v["kind"], "external");
    let url = v["platforms"]["linux_amd64"]["url"].as_str().unwrap();
    assert!(url.starts_with("https://"), "url must be https: {url}");

    // Server-side discovery view should only show the host, not the full URL.
    let detail = g
        .fixture()
        .registry()
        .get_component_detail(authed_request(
            &token,
            GetComponentDetailRequest {
                organisation: org.clone(),
                name: "rg-ext".into(),
            },
        ))
        .await
        .expect("get_component_detail")
        .into_inner();
    let summary = detail.summary.expect("summary");
    assert_eq!(summary.shape, ComponentShape::ToolExternal as i32);
    assert_eq!(summary.upstream_host, "github.com");

    // DownloadBinary against an external tool should fail — there's no
    // artifact, the binary lives upstream.
    let resp = g
        .fixture()
        .registry()
        .download_binary(authed_request(
            &token,
            DownloadBinaryRequest {
                organisation: org.clone(),
                name: "rg-ext".into(),
                version: "14.1.1".into(),
                os: "linux".into(),
                arch: "amd64".into(),
            },
        ))
        .await;
    if let Ok(stream) = resp {
        let mut s = stream.into_inner();
        let first = s.message().await;
        assert!(
            matches!(first, Err(_) | Ok(None)),
            "external manifests must not serve binary bytes; got {first:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn discovery_search_returns_shape_and_tool_facet() {
    // Verifies the §1a.2e ComponentShape + tool/methods/upstream_host
    // fields are populated on SearchComponents responses.
    let (g, _w, _t) = testcase::<GlobalToolsData>().await.unwrap();
    let token = register_user(&g).await;
    let org = unique_org();
    create_org(&g, &token, &org).await;

    publish_tool(
        &g,
        &token,
        &org,
        "discovery-tool",
        "0.1.0",
        json!({
            "kind": "binary",
            "tool": {"name": "disc", "argv_passthrough": true, "description": "discoverable"},
            "platforms": {
                "linux_amd64": {"sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
            }
        }),
    )
    .await;

    let resp = g
        .fixture()
        .registry()
        .search_components(authed_request(
            &token,
            SearchComponentsRequest {
                query: "discovery-tool".into(),
                organisation: org.clone(),
                page: 0,
                page_size: 10,
            },
        ))
        .await
        .expect("search")
        .into_inner();

    let found = resp
        .components
        .iter()
        .find(|c| c.name == "discovery-tool")
        .expect("discovery-tool missing");
    assert_eq!(found.shape, ComponentShape::ToolBinary as i32);
    let tool = found.tool.as_ref().expect("tool facet on search result");
    assert_eq!(tool.name, "disc");
    assert_eq!(tool.description, "discoverable");
}

#[tokio::test(flavor = "multi_thread")]
async fn list_org_tools_filters_by_shape_and_returns_latest_non_prerelease() {
    // §1a.2c: ListOrgTools streams components whose shape ∈ {hybrid, tool_*}.
    // Pure COMPONENTs are excluded. Prereleases are not chosen as latest.
    let (g, _w, _t) = testcase::<GlobalToolsData>().await.unwrap();
    let token = register_user(&g).await;
    let org = unique_org();
    create_org(&g, &token, &org).await;

    // 1. Pure component (no tool facet) — should NOT appear in ListOrgTools.
    publish_tool(
        &g,
        &token,
        &org,
        "pure-comp",
        "0.1.0",
        json!({
            "kind": "binary",
            "methods": ["status"],
            "platforms": {
                "linux_amd64": {"sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
            }
        }),
    )
    .await;

    // 2. Tool binary.
    publish_tool(
        &g,
        &token,
        &org,
        "hello",
        "0.1.0",
        json!({
            "kind": "binary",
            "tool": {"name": "hello", "argv_passthrough": true},
            "platforms": {
                "linux_amd64": {"sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}
            }
        }),
    )
    .await;

    // 3. Hybrid.
    publish_tool(
        &g,
        &token,
        &org,
        "greet",
        "0.2.0",
        json!({
            "kind": "binary",
            "tool": {"name": "greet", "argv_passthrough": true},
            "methods": ["greet"],
            "platforms": {
                "linux_amd64": {"sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"}
            }
        }),
    )
    .await;

    // 4. External tool.
    publish_tool(
        &g,
        &token,
        &org,
        "rgwrap",
        "1.0.0",
        json!({
            "kind": "external",
            "tool": {"name": "rg", "argv_passthrough": true, "description": "ripgrep wrapper"},
            "platforms": {
                "linux_amd64": {
                    "sha256": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
                    "url": "https://example.com/rg.tar.gz",
                    "archive": "tar.gz",
                    "binary_in_archive": "rg/rg"
                }
            }
        }),
    )
    .await;

    // 5. Newer prerelease of `hello` — must NOT win for latest_version.
    publish_tool(
        &g,
        &token,
        &org,
        "hello",
        "0.2.0-alpha.1",
        json!({
            "kind": "binary",
            "tool": {"name": "hello", "argv_passthrough": true},
            "platforms": {
                "linux_amd64": {"sha256": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"}
            }
        }),
    )
    .await;

    // Now call ListOrgTools and consume the stream.
    let mut stream = g
        .fixture()
        .registry()
        .list_org_tools(authed_request(
            &token,
            ListOrgToolsRequest {
                organisation: org.clone(),
            },
        ))
        .await
        .expect("list_org_tools")
        .into_inner();

    let mut tools = Vec::new();
    while let Some(entry) = stream.message().await.expect("stream") {
        tools.push(entry);
    }

    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(
        !names.contains(&"pure-comp"),
        "pure COMPONENT must be excluded from ListOrgTools, got {names:?}"
    );
    assert!(names.contains(&"hello"), "tool_binary 'hello' missing: {names:?}");
    assert!(names.contains(&"greet"), "hybrid 'greet' missing: {names:?}");
    assert!(names.contains(&"rgwrap"), "tool_external 'rgwrap' missing: {names:?}");

    let hello = tools.iter().find(|t| t.name == "hello").unwrap();
    assert_eq!(
        hello.latest_version, "0.1.0",
        "prerelease 0.2.0-alpha.1 must NOT win over 0.1.0; got {}",
        hello.latest_version
    );
    assert_eq!(
        hello.shape,
        ComponentShape::ToolBinary as i32,
        "hello shape should be TOOL_BINARY"
    );
    assert_eq!(
        hello.tool.as_ref().unwrap().name,
        "hello",
        "tool facet should be populated"
    );

    let greet = tools.iter().find(|t| t.name == "greet").unwrap();
    assert_eq!(greet.shape, ComponentShape::Hybrid as i32);

    let rgwrap = tools.iter().find(|t| t.name == "rgwrap").unwrap();
    assert_eq!(rgwrap.shape, ComponentShape::ToolExternal as i32);
    assert_eq!(rgwrap.upstream_host, "example.com");
    assert_eq!(
        rgwrap.tool.as_ref().unwrap().description,
        "ripgrep wrapper",
        "external tool description should be populated"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn list_org_tools_returns_empty_stream_for_org_with_no_tools() {
    let (g, _w, _t) = testcase::<GlobalToolsData>().await.unwrap();
    let token = register_user(&g).await;
    let org = unique_org();
    create_org(&g, &token, &org).await;

    let mut stream = g
        .fixture()
        .registry()
        .list_org_tools(authed_request(
            &token,
            ListOrgToolsRequest {
                organisation: org.clone(),
            },
        ))
        .await
        .expect("list_org_tools")
        .into_inner();

    let mut count = 0;
    while let Some(_) = stream.message().await.expect("stream") {
        count += 1;
    }
    assert_eq!(count, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn list_org_tools_requires_org_membership() {
    let (g, _w, _t) = testcase::<GlobalToolsData>().await.unwrap();

    let publisher = register_user(&g).await;
    let org = unique_org();
    create_org(&g, &publisher, &org).await;
    publish_tool(
        &g,
        &publisher,
        &org,
        "hidden-tool",
        "0.1.0",
        json!({
            "kind": "binary",
            "tool": {"name": "hidden", "argv_passthrough": true},
            "platforms": {
                "linux_amd64": {"sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
            }
        }),
    )
    .await;

    // Outsider user — not a member of `org`.
    let outsider = register_user(&g).await;
    let resp = g
        .fixture()
        .registry()
        .list_org_tools(authed_request(
            &outsider,
            ListOrgToolsRequest {
                organisation: org.clone(),
            },
        ))
        .await;

    assert!(
        resp.is_err(),
        "non-member must be refused; got Ok with tools"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_manifest_over_size_cap() {
    // §1a.2 rule 5.
    let (g, _w, _t) = testcase::<GlobalToolsData>().await.unwrap();
    let token = register_user(&g).await;
    let org = unique_org();
    create_org(&g, &token, &org).await;
    let upload = begin_upload(&g, &token, &org, "oversized", "0.1.0").await;

    // Build a >64 KiB manifest by padding the description field.
    let big_description: String = "x".repeat(70 * 1024);
    let manifest = json!({
        "kind": "binary",
        "tool": {"name": "ok", "argv_passthrough": true, "description": big_description},
        "platforms": {
            "linux_amd64": {"sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
        }
    });

    let err = publish_manifest(&g, &token, &upload, &manifest.to_string())
        .await
        .expect_err("must reject oversized manifest");
    assert!(
        err.message().contains("maximum size"),
        "got: {}",
        err.message()
    );
}
