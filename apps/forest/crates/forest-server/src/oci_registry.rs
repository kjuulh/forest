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
    Router,
    body::Body,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use sha2::{Digest, Sha256};

use crate::object_store::ObjectStore;

/// Create the OCI registry router.
pub fn oci_routes(object_store: ObjectStore) -> Router {
    Router::new()
        .route("/v2/", get(version_check))
        .route(
            "/v2/{*name_and_ref}",
            get(route_dispatch).head(route_dispatch_head),
        )
        .with_state(object_store)
}

/// GET /v2/ — OCI version check. Must return 200.
async fn version_check() -> impl IntoResponse {
    (StatusCode::OK, "")
}

/// Route dispatcher — parse the path to determine if it's a manifest or blob request.
async fn route_dispatch(State(store): State<ObjectStore>, Path(path): Path<String>) -> Response {
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
/// Path CUE requires a module file to live at inside the zip.
const MODULE_CUE_PATH: &str = "cue.mod/module.cue";

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
    //
    // The component may now carry its own: since components started publishing
    // `cue.mod/module.cue` as one of their files, it arrives in `cue_files` too.
    // Writing both put the same name into the zip twice, which the writer
    // rejects — and because the caller treats a failed OCI publish as
    // non-fatal, the component published fine while its CUE module silently
    // did not, leaving every consumer's `import` unresolvable. Prefer the
    // component's own module file, which is the one its authors wrote and
    // which carries its dependencies; synthesise one only when it has none.
    let uploaded_module_cue = cue_files
        .iter()
        .find(|(file_name, _)| file_name == MODULE_CUE_PATH)
        .map(|(_, content)| content.clone());

    let module_cue_in_zip = uploaded_module_cue.unwrap_or_else(|| {
        format!(
            "module: \"forest.sh/{organisation}/{name}@v0\"\nlanguage: {{\n\tversion: \"v0.16.1\"\n}}\nsource: {{\n\tkind: \"self\"\n}}\n"
        )
        .into_bytes()
    });

    let mut zip_data = Vec::new();
    {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut zip_data));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        // Include cue.mod/module.cue — required by CUE's module resolver
        zip.start_file(MODULE_CUE_PATH, options)?;
        zip.write_all(&module_cue_in_zip)?;
        for (file_name, content) in &cue_files {
            if file_name == MODULE_CUE_PATH {
                continue; // already written above
            }
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
        "module: \"forest.sh/{organisation}/{name}@v0\"\nlanguage: {{\n\tversion: \"v0.16.1\"\n}}\nsource: {{\n\tkind: \"self\"\n}}\n"
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

    let registry_domain =
        std::env::var("FOREST_CUE_DOMAIN").unwrap_or_else(|_| "forest.sh".to_string());
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

#[cfg(test)]
mod publish_cue_module_tests {
    use super::*;
    use std::io::Read;

    /// Build the zip exactly as `publish_cue_module` does, so the test exercises
    /// the archive shape without needing an object store.
    fn build_zip(
        organisation: &str,
        name: &str,
        cue_files: &[(String, Vec<u8>)],
    ) -> anyhow::Result<Vec<u8>> {
        let uploaded_module_cue = cue_files
            .iter()
            .find(|(file_name, _)| file_name == MODULE_CUE_PATH)
            .map(|(_, content)| content.clone());

        let module_cue_in_zip = uploaded_module_cue.unwrap_or_else(|| {
            format!(
                "module: \"forest.sh/{organisation}/{name}@v0\"\nlanguage: {{\n\tversion: \"v0.16.1\"\n}}\nsource: {{\n\tkind: \"self\"\n}}\n"
            )
            .into_bytes()
        });

        let mut zip_data = Vec::new();
        {
            use std::io::Write;
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut zip_data));
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zip.start_file(MODULE_CUE_PATH, options)?;
            zip.write_all(&module_cue_in_zip)?;
            for (file_name, content) in cue_files {
                if file_name == MODULE_CUE_PATH {
                    continue;
                }
                zip.start_file(file_name, options)?;
                zip.write_all(content)?;
            }
            zip.finish()?;
        }
        Ok(zip_data)
    }

    fn entries(zip_data: &[u8]) -> Vec<String> {
        let mut archive =
            zip::ZipArchive::new(std::io::Cursor::new(zip_data)).expect("readable zip");
        (0..archive.len())
            .map(|i| archive.by_index(i).expect("entry").name().to_string())
            .collect()
    }

    fn read_entry(zip_data: &[u8], name: &str) -> String {
        let mut archive =
            zip::ZipArchive::new(std::io::Cursor::new(zip_data)).expect("readable zip");
        let mut f = archive.by_name(name).expect("entry present");
        let mut s = String::new();
        f.read_to_string(&mut s).expect("utf8");
        s
    }

    /// A component that ships its own module file must still produce a valid
    /// archive. It did not: the packager wrote its synthesised
    /// `cue.mod/module.cue` and then the uploaded one under the same name, and
    /// the zip writer refused the duplicate. Because the caller logs a failed
    /// OCI publish and carries on, the component published while its CUE module
    /// did not — so `import "forest.sh/<org>/<name>@v0"` could not resolve for
    /// any consumer, with nothing failing to say so.
    #[test]
    fn a_component_shipping_its_own_module_file_still_packages() {
        let files = vec![
            (
                MODULE_CUE_PATH.to_string(),
                b"module: \"forest.sh/forest/sdk@v0\"\n".to_vec(),
            ),
            ("spec.cue".to_string(), b"package sdk\n".to_vec()),
        ];

        let zip_data = build_zip("forest", "sdk", &files).expect("packages");
        let names = entries(&zip_data);

        assert_eq!(
            names.iter().filter(|n| *n == MODULE_CUE_PATH).count(),
            1,
            "the module file must appear exactly once; got {names:?}"
        );
        assert!(names.contains(&"spec.cue".to_string()));
    }

    /// And the one that survives is the component's own, not a synthesised
    /// stand-in — the uploaded file is what its authors wrote and is the only
    /// copy carrying the component's own `deps`.
    #[test]
    fn the_components_own_module_file_wins() {
        let authored = "module: \"forest.sh/forest/deployment@v0\"\ndeps: {\n\t\"forest.sh/forest/sdk@v0\": {\n\t\tv: \"v0.7.0\"\n\t}\n}\n";
        let files = vec![
            (MODULE_CUE_PATH.to_string(), authored.as_bytes().to_vec()),
            ("forest.cue".to_string(), b"package deployment\n".to_vec()),
        ];

        let zip_data = build_zip("forest", "deployment", &files).expect("packages");

        assert_eq!(
            read_entry(&zip_data, MODULE_CUE_PATH),
            authored,
            "the uploaded module file carries the component's deps; a synthesised \
             one would silently drop them"
        );
    }

    /// The long-standing case: a component with no module file of its own still
    /// gets a synthesised one, because CUE cannot resolve the module without it.
    #[test]
    fn a_component_without_a_module_file_still_gets_one() {
        let files = vec![("spec.cue".to_string(), b"package sdk\n".to_vec())];

        let zip_data = build_zip("forest", "sdk", &files).expect("packages");

        assert!(
            read_entry(&zip_data, MODULE_CUE_PATH).contains("forest.sh/forest/sdk@v0"),
            "a synthesised module file should name the module"
        );
    }
}
