//! One-shot tool_binary publisher for live verification.
//!
//! Env:
//!   FOREST_SERVER          gRPC endpoint (http://localhost:4040)
//!   FOREST_ACCESS_TOKEN    bearer token (forest auth token create)
//!   ORG_NAME, COMP_NAME, COMP_VERSION
//!   BINARY_PATH, OS, ARCH
//!   DESCRIPTION (optional)
//!   README_PATH (optional — uploads as component file `README.md`)

use forest_grpc_interface::registry_service_client::RegistryServiceClient;
use forest_grpc_interface::{
    BeginUploadRequest, CommitUploadRequest, PublishManifestRequest, UploadBinaryMetadata,
    UploadBinaryRequest, UploadFileRequest, upload_binary_request::Msg,
};
use sha2::{Digest, Sha256};
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let server = std::env::var("FOREST_SERVER")?;
    let token = std::env::var("FOREST_ACCESS_TOKEN")?;
    let org = std::env::var("ORG_NAME")?;
    let name = std::env::var("COMP_NAME")?;
    let version = std::env::var("COMP_VERSION")?;
    let bin_path = std::env::var("BINARY_PATH")?;
    let os = std::env::var("OS")?;
    let arch = std::env::var("ARCH")?;
    let description = std::env::var("DESCRIPTION")
        .unwrap_or_else(|_| "Print a friendly greeting (Playwright seed)".into());
    let readme_path = std::env::var("README_PATH").ok();

    let bytes = std::fs::read(&bin_path)?;
    let sha = hex::encode(Sha256::digest(&bytes));
    println!("binary {bin_path}: {} bytes, sha256={sha}", bytes.len());

    let channel = Channel::from_shared(server.clone())?.connect().await?;
    let auth_token_meta: MetadataValue<_> = format!("Bearer {token}").parse()?;
    let mut client = RegistryServiceClient::with_interceptor(channel, move |mut req: tonic::Request<()>| {
        req.metadata_mut().insert("authorization", auth_token_meta.clone());
        Ok(req)
    });

    let upload_ctx = client
        .begin_upload(BeginUploadRequest {
            name: name.clone(),
            organisation: org.clone(),
            version: version.clone(),
        })
        .await?
        .into_inner()
        .upload_context;

    let chunk_size = 1024 * 1024;
    let mut msgs: Vec<UploadBinaryRequest> = vec![UploadBinaryRequest {
        msg: Some(Msg::Metadata(UploadBinaryMetadata {
            upload_context: upload_ctx.clone(),
            os: os.clone(),
            arch: arch.clone(),
            sha256: sha.clone(),
        })),
    }];
    for c in bytes.chunks(chunk_size) {
        msgs.push(UploadBinaryRequest {
            msg: Some(Msg::Chunk(c.to_vec())),
        });
    }
    client.upload_binary(futures::stream::iter(msgs)).await?;

    // Upload README.md as a component file (alongside the binary). The
    // server's `GetComponentDetail` returns it under `readme`; forage's
    // project Overview renders it.
    if let Some(path) = readme_path {
        let content = std::fs::read(&path)?;
        println!("README {path}: {} bytes", content.len());
        client
            .upload_file(UploadFileRequest {
                upload_context: upload_ctx.clone(),
                file_path: "README.md".into(),
                file_content: content,
            })
            .await?;
    }

    let manifest = serde_json::json!({
        "kind": "binary",
        "tool": {"name": name, "argv_passthrough": true, "description": description},
        "platforms": {
            format!("{os}_{arch}"): {"sha256": sha, "size": bytes.len()}
        }
    });
    client
        .publish_manifest(PublishManifestRequest {
            upload_context: upload_ctx.clone(),
            manifest_json: manifest.to_string(),
        })
        .await?;
    client
        .commit_upload(CommitUploadRequest { upload_context: upload_ctx })
        .await?;

    println!("OK — {org}/{name}@{version} published");
    Ok(())
}
