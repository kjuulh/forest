//! One-shot project description + blessed metadata pusher.
//!
//! Exercises the spec 009 path (UpdateProject with description +
//! metadata field-mask) without uploading a binary. Useful for proving
//! the round-trip on an existing project, or pre-populating a project
//! that hasn't been published from CUE yet.
//!
//! Env:
//!   FOREST_SERVER          gRPC endpoint (http://localhost:4040)
//!   FOREST_ACCESS_TOKEN    bearer token (`forest auth token create`)
//!   ORG_NAME, PROJECT_NAME
//!   PROJECT_DESCRIPTION    (optional)
//!   PROJECT_GIT_URL, PROJECT_HOMEPAGE, PROJECT_DOCS_URL,
//!   PROJECT_SUPPORT_URL, PROJECT_DOMAIN, PROJECT_OWNER  (all optional)

use forest_grpc_interface::release_service_client::ReleaseServiceClient;
use forest_grpc_interface::{
    CreateProjectRequest, ProjectMetadata, UpdateProjectRequest,
};
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let server = std::env::var("FOREST_SERVER")?;
    let token = std::env::var("FOREST_ACCESS_TOKEN")?;
    let org = std::env::var("ORG_NAME")?;
    let project = std::env::var("PROJECT_NAME")?;

    let env_or_empty = |k: &str| std::env::var(k).unwrap_or_default();
    let metadata = ProjectMetadata {
        git_url: env_or_empty("PROJECT_GIT_URL"),
        homepage: env_or_empty("PROJECT_HOMEPAGE"),
        docs_url: env_or_empty("PROJECT_DOCS_URL"),
        support_url: env_or_empty("PROJECT_SUPPORT_URL"),
        domain: env_or_empty("PROJECT_DOMAIN"),
        owner: env_or_empty("PROJECT_OWNER"),
    };
    let description = env_or_empty("PROJECT_DESCRIPTION");

    let channel = Channel::from_shared(server)?.connect().await?;
    let auth_token_meta: MetadataValue<_> = format!("Bearer {token}").parse()?;
    let mut release = ReleaseServiceClient::with_interceptor(
        channel,
        move |mut req: tonic::Request<()>| {
            req.metadata_mut()
                .insert("authorization", auth_token_meta.clone());
            Ok(req)
        },
    );

    // Idempotent — ON CONFLICT DO NOTHING on the server.
    release
        .create_project(CreateProjectRequest {
            organisation: org.clone(),
            project: project.clone(),
        })
        .await?;

    let resp = release
        .update_project(UpdateProjectRequest {
            organisation: org.clone(),
            project: project.clone(),
            readme: None,
            description: Some(description.clone()),
            metadata: Some(metadata.clone()),
        })
        .await?
        .into_inner();

    println!("OK — pushed project description + metadata for {org}/{project}");
    if let Some(p) = resp.project {
        println!("server returned:");
        println!("  description: {:?}", p.description);
        if let Some(m) = p.metadata {
            println!("  git_url:     {:?}", m.git_url);
            println!("  homepage:    {:?}", m.homepage);
            println!("  docs_url:    {:?}", m.docs_url);
            println!("  support_url: {:?}", m.support_url);
            println!("  domain:      {:?}", m.domain);
            println!("  owner:       {:?}", m.owner);
        }
    }

    Ok(())
}
