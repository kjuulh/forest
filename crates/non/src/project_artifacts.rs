use std::collections::BTreeMap;

/// Artifacts are spread out in the directory like so
/// ```
/// .non/artifacts/<env>/<destination>/<files>
/// ```
///
/// - env is the logical environment, i.e. dev, staging, prod
/// - destination is a specific k8s, cluster, aws account for lambda ecs, and more.
/// - files are the specific manifest files required for the upload. It may be k8s manifest, lambda terraform etc.
///
/// Deployment can then happen on multiple levels
///
/// - Default would be deploy dev --artifact-id <uuid>, or deploy dev @latest, or deploy dev @main, or deploy dev #<commit-sha>.
/// - Another would be deploy dev/<destination> @latest, etc.
///
pub struct ProjectArtifacts {}

#[allow(dead_code)]
impl ProjectArtifacts {
    /// publish takes in a local dir, scans the destination for artifacts and uploads them to the artifacts registry
    ///
    /// Upload works like a database transaction, we prepare an upload, (begin transaction), then we upload files in parallel, until each of them is complete and we then commit. A rollback, can be performed, by simply letting the commit expire.
    pub async fn publish(
        &self,
        registry_url: impl Into<RegistryUrl>,
        _data: ArtifactData,
    ) -> anyhow::Result<()> {
        let _registry: RegistryUrl = registry_url.into();

        // 1. Stage the upload
        // 2. Upload files
        // 3. Annotate the artifact (this performs a commit), from this point on the artifact is immutable
        // 4. Return the artifact sha

        Ok(())
    }
}

pub struct RegistryUrl {
    pub url: String,
}

/// Each key is set based on
///
/// - git.commit_title: "item"
/// - forge.pull_request_url
/// - build.url
/// - local.title
/// - metadata.<item>: "something"
///
/// if more than one prefix shows up, it is turned into an aggregate, and certain items take precedence over others. All values are kept, but the ui may choose to show some values over others, if they've got semantic similarities.
pub enum ArtifactData {
    Git {
        commit_title: String,
        commit_description: String,

        /// commit_sha is the sha of the commit under which the artifact was produced
        commit_sha: String,

        /// The branch etc
        reference: String,

        time: jiff::Timestamp,

        /// author
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
