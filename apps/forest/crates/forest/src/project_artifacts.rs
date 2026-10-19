use std::collections::BTreeMap;

pub struct ProjectArtifacts {}

#[allow(dead_code)]
impl ProjectArtifacts {
    pub async fn publish(
        &self,
        registry_url: impl Into<RegistryUrl>,
        _data: ArtifactData,
    ) -> anyhow::Result<()> {
        let _registry: RegistryUrl = registry_url.into();

        Ok(())
    }
}

pub struct RegistryUrl {
    pub url: String,
}

pub enum ArtifactData {
    Git {
        commit_title: String,
        commit_description: String,

        commit_sha: String,

        reference: String,

        time: jiff::Timestamp,

        author_name: String,
        author_email: String,
    },
    Forge {
        pull_request_url: Option<String>,
        upstream: Option<String>,
    },
    Build {
        url: String,
    },
    Local {
        title: String,
        commit_description: String,
        time: jiff::Timestamp,
    },
    ManualAuthor {
        author_name: String,
        author_email: String,
    },
    Metadata {
        values: BTreeMap<String, String>,
    },
    Aggregate(Vec<ArtifactData>),
}
