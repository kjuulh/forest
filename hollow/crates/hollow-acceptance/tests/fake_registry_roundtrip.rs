//! Wire-shape verification for the FakeRegistry. Doesn't run inside a
//! VM — tests the gRPC server in-process so we know the RegistryService
//! implementation matches what `forest-server` would expose, before we
//! point the in-VM `forest` CLI at it.
//!
//! Covers: GetComponentManifest, DownloadBinary streaming, the explicit
//! `Unimplemented` for everything else.

use forest_grpc_interface::registry_service_client::RegistryServiceClient;
use forest_grpc_interface::{
    DownloadBinaryRequest, GetComponentManifestRequest, ListComponentVersionsRequest,
    UploadFileRequest,
};
use futures::StreamExt;
use hollow_test_harness::fake_registry::FakeRegistry;

#[tokio::test]
async fn fake_registry_serves_v2_binary_rpcs() -> anyhow::Result<()> {
    let registry = FakeRegistry::start().await?;

    // Pre-load a fake "binary" with a recognisable shape — the kind of
    // payload `forest components publish` would upload after building.
    let binary: Vec<u8> = (0u8..=255).cycle().take(150_000).collect();
    let manifest_json = serde_json::json!({
        "name": "render-template",
        "organisation": "forest-contrib",
        "version": "0.1.0",
        "platforms": {
            "linux_amd64": {
                "sha256": "deadbeef".repeat(8),
                "size": binary.len(),
            }
        }
    })
    .to_string();
    registry.upload(
        "forest-contrib",
        "render-template",
        "0.1.0",
        binary.clone(),
        manifest_json.clone(),
    );

    let mut client = RegistryServiceClient::connect(registry.endpoint()).await?;

    // GetComponentManifest — round-trips the JSON we uploaded verbatim.
    let manifest = client
        .get_component_manifest(GetComponentManifestRequest {
            organisation: "forest-contrib".into(),
            name: "render-template".into(),
            version: "0.1.0".into(),
        })
        .await?
        .into_inner();
    assert_eq!(manifest.manifest_json, manifest_json);

    // DownloadBinary — server-streaming. Reassemble and check we got
    // the exact bytes back. Streaming is meaningful here because the
    // 150 KB payload spans more than one 64 KiB chunk.
    let mut stream = client
        .download_binary(DownloadBinaryRequest {
            organisation: "forest-contrib".into(),
            name: "render-template".into(),
            version: "0.1.0".into(),
            os: "linux".into(),
            arch: "amd64".into(),
        })
        .await?
        .into_inner();
    let mut received = Vec::with_capacity(binary.len());
    let mut frames = 0;
    while let Some(item) = stream.next().await {
        let chunk = item?;
        received.extend_from_slice(&chunk.chunk);
        frames += 1;
    }
    assert_eq!(received, binary, "downloaded bytes must match upload");
    assert!(
        frames >= 2,
        "150 KB payload should span at least 2 frames (got {frames})"
    );

    // Missing component → NotFound (production registry uses the same
    // shape; tests of the cache-miss path will rely on this).
    let err = client
        .get_component_manifest(GetComponentManifestRequest {
            organisation: "nope".into(),
            name: "missing".into(),
            version: "0.0.0".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);

    // ListComponentVersions returns empty (stub) — tests that need
    // populated lists will need to extend the FakeRegistry.
    let listed = client
        .list_component_versions(ListComponentVersionsRequest {
            organisation: "forest-contrib".into(),
            name: "render-template".into(),
        })
        .await?
        .into_inner();
    assert!(listed.versions.is_empty());

    // v1 file-based RPCs are explicitly Unimplemented so a misconfigured
    // tool fails loudly instead of silently passing.
    let err = client
        .upload_file(UploadFileRequest {
            upload_context: "x".into(),
            file_path: "x".into(),
            file_content: vec![],
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unimplemented);

    Ok(())
}
