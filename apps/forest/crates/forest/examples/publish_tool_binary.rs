//! One-shot tool_binary publisher for live verification.
//!
//! Env:
//!   FOREST_SERVER          gRPC endpoint (http://localhost:4040)
//!   FOREST_ACCESS_TOKEN    bearer token (forest auth token create)
//!   ORG_NAME, COMP_NAME, COMP_VERSION
//!   BINARY_PATH, OS, ARCH
//!   DESCRIPTION                  (optional — used both for the tool facet
//!                                 AND pushed to the project as `project.description`)
//!   README_PATH                  (optional — uploads as component file `README.md`)
//!
//! Project-level (spec 009) — pushed via UpdateProject:
//!   PROJECT_GIT_URL, PROJECT_HOMEPAGE, PROJECT_DOCS_URL,
//!   PROJECT_SUPPORT_URL, PROJECT_DOMAIN, PROJECT_OWNER

use forest_grpc_interface::registry_service_client::RegistryServiceClient;
use forest_grpc_interface::release_service_client::ReleaseServiceClient;
use forest_grpc_interface::{
    BeginUploadRequest, CommitUploadRequest, CreateProjectRequest, ProjectMetadata,
    PublishManifestRequest, UpdateProjectRequest, UploadBinaryMetadata, UploadBinaryRequest,
    UploadFileRequest, upload_binary_request::Msg,
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

    // Push project-level description + blessed metadata (spec 009).
    // Mirrors what `forest publish` does for real publishes: read CUE,
    // upsert the project, push UpdateProject with field-mask.
    let env_or_empty = |k: &str| std::env::var(k).unwrap_or_default();
    let metadata = ProjectMetadata {
        git_url: env_or_empty("PROJECT_GIT_URL"),
        homepage: env_or_empty("PROJECT_HOMEPAGE"),
        docs_url: env_or_empty("PROJECT_DOCS_URL"),
        support_url: env_or_empty("PROJECT_SUPPORT_URL"),
        domain: env_or_empty("PROJECT_DOMAIN"),
        owner: env_or_empty("PROJECT_OWNER"),
    };
    let project_description = description.clone();

    let auth_token_meta2: MetadataValue<_> = format!("Bearer {token}").parse()?;
    let channel2 = Channel::from_shared(server)?.connect().await?;
    let mut release =
        ReleaseServiceClient::with_interceptor(channel2, move |mut req: tonic::Request<()>| {
            req.metadata_mut().insert("authorization", auth_token_meta2.clone());
            Ok(req)
        });

    // Idempotent — server upserts on conflict.
    release
        .create_project(CreateProjectRequest {
            organisation: org.clone(),
            project: name.clone(),
        })
        .await?;

    release
        .update_project(UpdateProjectRequest {
            organisation: org.clone(),
            project: name.clone(),
            readme: None,
            description: Some(project_description),
            metadata: Some(metadata),
        })
        .await?;

    println!("OK — {org}/{name} project description + metadata pushed");
    Ok(())
}
