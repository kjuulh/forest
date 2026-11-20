use forest_grpc_interface::release_service_client::ReleaseServiceClient;
use forest_grpc_interface::{CreateProjectRequest, ProjectMetadata, UpdateProjectRequest};
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
        tags: split_tags(&env_or_empty("PROJECT_TAGS")),
    };
    let description = env_or_empty("PROJECT_DESCRIPTION");

    let channel = Channel::from_shared(server)?.connect().await?;
    let auth_token_meta: MetadataValue<_> = format!("Bearer {token}").parse()?;
    let mut release =
        ReleaseServiceClient::with_interceptor(channel, move |mut req: tonic::Request<()>| {
            req.metadata_mut()
                .insert("authorization", auth_token_meta.clone());
            Ok(req)
        });

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
            visibility: None,
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
            println!("  tags:        {:?}", m.tags);
        }
    }

    Ok(())
}

fn split_tags(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}
