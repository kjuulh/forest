//! Built-in OCI Distribution Spec registry for CUE module distribution.
//!
//! This implements the minimum OCI endpoints that CUE's module resolver needs:
//! - GET /v2/ (version check)
//! - GET /v2/{name}/manifests/{reference} (get manifest by tag)
//! - GET /v2/{name}/blobs/{digest} (get blob by sha256)
//! - HEAD /v2/{name}/manifests/{reference} (check manifest exists)
//!
//! CUE modules are published automatically when a component is committed
//! via `forest publish`. The server packages the component's CUE files
//! into OCI artifacts and serves them from S3.

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use sha2::{Digest, Sha256};

use crate::object_store::ObjectStore;

/// Create the OCI registry router.
pub fn oci_routes(object_store: ObjectStore) -> Router {
    Router::new()
        .route("/v2/", get(version_check))
        .route("/v2/{*name_and_ref}", get(route_dispatch).head(route_dispatch_head))
        .with_state(object_store)
}

/// GET /v2/ — OCI version check. Must return 200.
async fn version_check() -> impl IntoResponse {
    (StatusCode::OK, "")
}

/// Route dispatcher — parse the path to determine if it's a manifest or blob request.
async fn route_dispatch(
    State(store): State<ObjectStore>,
    Path(path): Path<String>,
) -> Response {
    if let Some((name, reference)) = parse_manifest_path(&path) {
        get_manifest(store, &name, &reference).await
    } else if let Some((name, digest)) = parse_blob_path(&path) {
        get_blob(store, &name, &digest).await
    } else {
        (StatusCode::NOT_FOUND, "not found").into_response()
    }
}

/// HEAD dispatcher for manifest existence checks.
async fn route_dispatch_head(
    State(store): State<ObjectStore>,
    Path(path): Path<String>,
) -> Response {
    if let Some((name, reference)) = parse_manifest_path(&path) {
        head_manifest(store, &name, &reference).await
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

/// Parse `/v2/{name}/manifests/{reference}` from the wildcard path.
fn parse_manifest_path(path: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = path.rsplitn(3, '/').collect();
    if parts.len() >= 3 && parts[1] == "manifests" {
        let reference = parts[0].to_string();
        let name = parts[2].to_string();
        Some((name, reference))
    } else {
        None
    }
}

/// Parse `/v2/{name}/blobs/{digest}` from the wildcard path.
fn parse_blob_path(path: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = path.rsplitn(3, '/').collect();
    if parts.len() >= 3 && parts[1] == "blobs" {
        let digest = parts[0].to_string();
        let name = parts[2].to_string();
        Some((name, digest))
    } else {
        None
    }
}

/// GET /v2/{name}/manifests/{reference}
async fn get_manifest(store: ObjectStore, name: &str, reference: &str) -> Response {
    let key = format!("oci/{name}/manifests/{reference}");

    match store.get(&key).await {
        Ok(data) => {
            let digest = format!("sha256:{}", hex::encode(Sha256::digest(&data)));
            // Detect content type from the manifest's mediaType field
            let content_type = detect_manifest_media_type(&data);
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, content_type)
                .header("Docker-Content-Digest", &digest)
                .header(header::CONTENT_LENGTH, data.len())
                .body(Body::from(data))
                .unwrap()
        }
        Err(_) => (StatusCode::NOT_FOUND, "manifest not found").into_response(),
    }
}

/// HEAD /v2/{name}/manifests/{reference}
async fn head_manifest(store: ObjectStore, name: &str, reference: &str) -> Response {
    let key = format!("oci/{name}/manifests/{reference}");

    match store.get(&key).await {
        Ok(data) => {
            let digest = format!("sha256:{}", hex::encode(Sha256::digest(&data)));
            let content_type = detect_manifest_media_type(&data);
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, content_type)
                .header("Docker-Content-Digest", &digest)
                .header(header::CONTENT_LENGTH, data.len())
                .body(Body::empty())
                .unwrap()
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// GET /v2/{name}/blobs/{digest}
async fn get_blob(store: ObjectStore, _name: &str, digest: &str) -> Response {
    // Digest format: "sha256:hexstring"
    let key = format!("oci/blobs/{digest}");

    match store.get(&key).await {
        Ok(data) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header("Docker-Content-Digest", digest)
            .header(header::CONTENT_LENGTH, data.len())
            .body(Body::from(data))
            .unwrap(),
        Err(_) => (StatusCode::NOT_FOUND, "blob not found").into_response(),
    }
}

/// Detect the media type from a manifest's JSON content.
fn detect_manifest_media_type(data: &[u8]) -> &'static str {
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(data) {
        if let Some(mt) = v.get("mediaType").and_then(|m| m.as_str()) {
            if mt.contains("index") {
                return "application/vnd.oci.image.index.v1+json";
            }
        }
    }
    "application/vnd.oci.image.manifest.v1+json"
}

/// Package CUE files into an OCI artifact and store in S3.
///
/// Called by the component service when a component is committed.
/// Creates an OCI image manifest with a single layer containing all CUE files
/// as a tar archive.
pub async fn publish_cue_module(
    store: &ObjectStore,
    organisation: &str,
    name: &str,
    version: &str,
    cue_files: Vec<(String, Vec<u8>)>,
) -> anyhow::Result<()> {
    if cue_files.is_empty() {
        return Ok(());
    }

    // Create a zip archive of the CUE files (CUE modules use zip, not tar)
    // CUE expects the zip to contain cue.mod/module.cue inside it.
    let module_cue_in_zip = format!(
        "module: \"forest.sh/{organisation}/{name}@v0\"\nlanguage: {{\n\tversion: \"v0.15.4\"\n}}\nsource: {{\n\tkind: \"self\"\n}}\n"
    );
    let mut zip_data = Vec::new();
    {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut zip_data));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        // Include cue.mod/module.cue — required by CUE's module resolver
        zip.start_file("cue.mod/module.cue", options)?;
        zip.write_all(module_cue_in_zip.as_bytes())?;
        for (file_name, content) in &cue_files {
            zip.start_file(file_name, options)?;
            zip.write_all(content)?;
        }
        zip.finish()?;
    }

    // Store the layer blob
    let layer_digest = format!("sha256:{}", hex::encode(Sha256::digest(&zip_data)));
    let layer_size = zip_data.len();
    store
        .put(&format!("oci/blobs/{layer_digest}"), &zip_data)
        .await?;

    // Create an empty config blob (required by OCI spec)
    let config_data = b"{}";
    let config_digest = format!("sha256:{}", hex::encode(Sha256::digest(config_data)));
    let config_size = config_data.len();
    store
        .put(&format!("oci/blobs/{config_digest}"), config_data)
        .await?;

    // Store the cue.mod/module.cue as a separate blob (CUE optimization for fast dep resolution)
    let module_cue_content = format!(
        "module: \"forest.sh/{organisation}/{name}@v0\"\nlanguage: {{\n\tversion: \"v0.15.4\"\n}}\nsource: {{\n\tkind: \"self\"\n}}\n"
    );
    let module_cue_bytes = module_cue_content.as_bytes();
    let module_cue_digest = format!("sha256:{}", hex::encode(Sha256::digest(module_cue_bytes)));
    let module_cue_size = module_cue_bytes.len();
    store
        .put(&format!("oci/blobs/{module_cue_digest}"), module_cue_bytes)
        .await?;

    // Create the OCI image manifest (matches cue mod publish format exactly)
    let manifest = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {
            "mediaType": "application/vnd.cue.module.v1+json",
            "digest": config_digest,
            "size": config_size,
        },
        "layers": [
            {
                "mediaType": "application/zip",
                "digest": layer_digest,
                "size": layer_size,
            },
            {
                "mediaType": "application/vnd.cue.modulefile.v1",
                "digest": module_cue_digest,
                "size": module_cue_size,
            }
        ]
    });

    let manifest_bytes = serde_json::to_vec(&manifest)?;
    let manifest_digest = format!("sha256:{}", hex::encode(Sha256::digest(&manifest_bytes)));

    let registry_domain = std::env::var("FOREST_CUE_DOMAIN").unwrap_or_else(|_| "forest.sh".to_string());
    let module_path = format!("{registry_domain}/{organisation}/{name}");

    // Store manifest under both tag and digest
    store
        .put(
            &format!("oci/{module_path}/manifests/v{version}"),
            &manifest_bytes,
        )
        .await?;

    store
        .put(
            &format!("oci/{module_path}/manifests/{manifest_digest}"),
            &manifest_bytes,
        )
        .await?;

    tracing::info!(
        "published OCI CUE module {module_path}:v{version} ({} files, {} bytes)",
        cue_files.len(),
        layer_size,
    );

    Ok(())
}
