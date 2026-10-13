//! `forest/generic@1` — a destination type implemented by an external service.
//!
//! Most destination types are compiled into forest. Some should not be: a type
//! that talks to one vendor's API, needs its own identity, or is only wanted in
//! one deployment has no business being in every forest build.
//!
//! A `generic` destination carries a `provider_url` in its metadata. forest dials
//! it and speaks `forest.provider.v1.DestinationProvider`: `Describe` for the
//! metadata schema, `Execute` to run a release. Every other metadata key is
//! passed through untouched, so the provider sees only its own configuration.
//!
//! This is the same shape `forage/containers@1` already uses — a gRPC endpoint in
//! metadata, dialled at release time — generalised so the contract is a small
//! type-agnostic proto rather than one service's API.
//!
//! What that buys, concretely: the metadata schema lives with the implementation
//! instead of being mirrored into forest's configuration, and adding a type is
//! "deploy a service, create a destination" rather than "redeploy forest".
//!
//! **Providers are dialled by forest**, so they must be reachable from it. An
//! implementation that lives somewhere forest cannot reach wants the runner
//! (dial-in) model instead.

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use anyhow::Context;
use forest_grpc_interface::provider::{
    DescribeRequest, DescribeResponse, Destination as ProviderDestination, ExecuteRequest,
    Release as ProviderRelease, ReleaseMode,
    destination_provider_client::DestinationProviderClient, execute_event::Event,
};
use forest_models::Destination;

use crate::{
    destinations::{DestinationEdge, DestinationIndex, logger::DestinationLogger},
    services::{
        artifact_staging_registry::ArtifactStagingRegistry,
        release_pipeline::{ApprovalStatus, PipelineStages, StageConfig, StageStates, StageStatus},
        release_registry::{Reference, ReleaseItem},
        release_token_registry::{ReleaseTokenRegistry, ReleaseTokenScope},
    },
};

/// Hosts a `provider_url` may point at, comma-separated. Entries are either an
/// exact `host` / `host:port`, or a `*.suffix` wildcard.
///
/// Unset means **no provider is permitted**. That is deliberate: forest dials
/// this URL carrying a release-scoped token, so an unconstrained value lets any
/// org member point it wherever they like. Fail closed and make the deployment
/// say what it trusts.
pub const ALLOWED_HOSTS_ENV: &str = "FOREST_GENERIC_PROVIDER_ALLOWED_HOSTS";

/// How long a provider's release token stays valid. Matches the runner path.
const TOKEN_TTL: Duration = Duration::from_secs(3600);

const PROVIDER_URL: &str = "provider_url";
const PROVIDER_TOKEN: &str = "provider_token";

/// `generic`'s own configuration. The provider's own fields are not knowable
/// until there is a URL to ask, and come back from `Describe` at creation time.
pub fn metadata_schema() -> Vec<forest_models::MetadataFieldSchema> {
    vec![
            forest_models::MetadataFieldSchema {
            name: PROVIDER_URL.into(),
            label: "Provider URL".into(),
            description:
            "gRPC endpoint implementing forest.provider.v1.DestinationProvider (e.g. http://forest-ecs-provider.internal:4060)."
                .into(),
            required: true,
            field_type: "url".into(),
            default_value: String::new(),
            sensitive: false,
        },
        forest_models::MetadataFieldSchema {
            name: PROVIDER_TOKEN.into(),
            label: "Provider Token".into(),
            description:
            "Bearer token sent to the provider on every call. Optional for providers reachable only on an internal network."
                .into(),
            required: false,
            field_type: "password".into(),
            default_value: String::new(),
            sensitive: true,
        },
    ]
}

pub struct GenericV1Destination {
    pub artifact_files: ArtifactStagingRegistry,
    /// For reading the declaration `annotate` recorded. See `release_config`.
    pub db: sqlx::PgPool,
    /// For announcing signals a provider reports, so a gate waiting on one
    /// re-evaluates immediately rather than on the next sweep.
    pub nats: async_nats::Client,
    pub release_tokens: ReleaseTokenRegistry,
    /// forest's own externally-reachable address, handed to the provider so it
    /// can call back for artifacts if it needs them.
    pub external_host: String,
}

struct GenericMetadata {
    provider_url: String,
    provider_token: Option<String>,
    /// Everything that isn't `generic`'s own configuration.
    passthrough: HashMap<String, String>,
}

impl GenericMetadata {
    fn from_metadata(metadata: &HashMap<String, String>) -> anyhow::Result<Self> {
        let provider_url = metadata
            .get(PROVIDER_URL)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .context("metadata 'provider_url' is required for generic destinations")?;

        check_host_allowed(&provider_url)?;

        Ok(Self {
            provider_token: metadata
                .get(PROVIDER_TOKEN)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty()),
            provider_url,
            passthrough: metadata
                .iter()
                .filter(|(k, _)| k.as_str() != PROVIDER_URL && k.as_str() != PROVIDER_TOKEN)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        })
    }
}

/// Redacts `provider_token`. Derived `Debug` on a struct holding a credential
/// is one `tracing::debug!` away from putting it in the logs, which is exactly
/// what marking the field `sensitive` is supposed to prevent.
impl std::fmt::Debug for GenericMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GenericMetadata")
            .field("provider_url", &self.provider_url)
            .field(
                "provider_token",
                &self.provider_token.as_ref().map(|_| "<redacted>"),
            )
            .field("passthrough_keys", &self.passthrough.keys())
            .finish()
    }
}

fn allowed_hosts() -> Vec<String> {
    std::env::var(ALLOWED_HOSTS_ENV)
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Check a provider URL against the allowlist.
pub fn check_host_allowed(url: &str) -> anyhow::Result<()> {
    let allowed = allowed_hosts();
    if allowed.is_empty() {
        anyhow::bail!(
            "no provider hosts are permitted: {ALLOWED_HOSTS_ENV} is unset. forest dials this \
             URL carrying a release-scoped token, so the deployment must say which hosts it \
             trusts before a generic destination can be used."
        );
    }

    let authority =
        authority_of(url).with_context(|| format!("'{PROVIDER_URL}' is not a valid URL: {url}"))?;

    if allowed
        .iter()
        .any(|pattern| matches_host(pattern, &authority))
    {
        return Ok(());
    }

    anyhow::bail!(
        "provider host '{authority}' is not in {ALLOWED_HOSTS_ENV} ({})",
        allowed.join(", "),
    )
}

/// Host (and port, if given) of a URL, lowercased.
fn authority_of(url: &str) -> anyhow::Result<String> {
    let (_scheme, rest) = url
        .split_once("://")
        .context("URL has no scheme (expected http:// or https://)")?;

    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        // Strip any userinfo — `user@host` must match on `host`, not on the
        // whole thing, or `evil.com@trusted.internal` style tricks slip past.
        .rsplit('@')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();

    if authority.is_empty() {
        anyhow::bail!("URL has no host");
    }

    Ok(authority)
}

fn matches_host(pattern: &str, authority: &str) -> bool {
    let pattern = pattern.to_ascii_lowercase();
    let host = authority
        .rsplit_once(':')
        .map(|(h, _)| h)
        .unwrap_or(authority);

    if let Some(suffix) = pattern.strip_prefix("*.") {
        // `*.internal` matches `a.internal` and `a.internal:4050`, not `internal`
        // itself and not `notinternal`.
        return host.ends_with(&format!(".{suffix}"));
    }

    // An exact pattern matches either with or without the port.
    pattern == authority || pattern == host
}

/// A project's config block, flattened for a provider that receives a flat
/// string map.
///
/// Nested structure has no meaning there, so it is skipped rather than encoded
/// as JSON into a value someone would have to guess the shape of.
fn flatten_config(config: &serde_json::Value) -> Option<HashMap<String, String>> {
    let serde_json::Value::Object(config) = config else {
        return None;
    };

    Some(
        config
            .iter()
            .filter_map(|(k, v)| {
                let value = match v {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    serde_json::Value::Number(n) => n.to_string(),
                    _ => return None,
                };
                Some((k.clone(), value))
            })
            .collect(),
    )
}

/// Destination-owned settings cannot be changed by a project's release config.
/// Reserved provider keys and credentials declared sensitive at destination
/// creation are never accepted from the project, even if unset on the destination.
fn provider_metadata(
    meta: &GenericMetadata,
    project_config: HashMap<String, String>,
    sensitive_keys: &[String],
) -> HashMap<String, String> {
    let mut metadata = meta.passthrough.clone();
    for (key, value) in project_config {
        if key != PROVIDER_URL
            && key != PROVIDER_TOKEN
            && !sensitive_keys.iter().any(|sensitive| sensitive == &key)
        {
            metadata.entry(key).or_insert(value);
        }
    }
    metadata
}

/// The execution ID changes per PLAN/DEPLOY; artifact and intent do not.
async fn provider_release(
    db: &sqlx::PgPool,
    release: &ReleaseItem,
) -> anyhow::Result<ProviderRelease> {
    let (slug, reference): (String, serde_json::Value) =
        sqlx::query_as("SELECT slug, ref FROM annotations WHERE artifact_id = $1")
            .bind(release.artifact)
            .fetch_one(db)
            .await
            .context("generic provider release requires an annotated artifact")?;
    let reference: Reference =
        serde_json::from_value(reference).context("invalid artifact source reference")?;
    Ok(ProviderRelease {
        release_id: release.id.to_string(),
        project: release.project.clone(),
        slug,
        reference_commit_sha: reference.commit_sha,
        reference_commit_branch: reference.commit_branch.unwrap_or_default(),
        reference_version: reference.version.unwrap_or_default(),
        release_intent_id: release.release_intent_id.to_string(),
        artifact_id: release.artifact.to_string(),
    })
}

fn sensitive_keys_from_describe(described: DescribeResponse) -> Vec<String> {
    let mut keys = vec![PROVIDER_TOKEN.to_string()];
    keys.extend(
        described
            .fields
            .into_iter()
            .filter(|field| field.sensitive)
            .map(|field| field.name),
    );
    keys
}

impl GenericV1Destination {
    async fn connect(
        &self,
        meta: &GenericMetadata,
    ) -> anyhow::Result<DestinationProviderClient<tonic::transport::Channel>> {
        DestinationProviderClient::connect(meta.provider_url.clone())
            .await
            .with_context(|| format!("failed to connect to provider at {}", meta.provider_url))
    }

    /// Attach the provider's bearer token, when the destination sets one.
    fn authed<T>(meta: &GenericMetadata, message: T) -> anyhow::Result<tonic::Request<T>> {
        let mut request = tonic::Request::new(message);
        if let Some(token) = &meta.provider_token {
            let value = format!("Bearer {token}")
                .parse()
                .context("provider_token is not a valid HTTP header value")?;
            request.metadata_mut().insert("authorization", value);
        }
        Ok(request)
    }

    async fn describe(&self, meta: &GenericMetadata) -> anyhow::Result<DescribeResponse> {
        let mut client = self.connect(meta).await?;
        Ok(client
            .describe(Self::authed(meta, DescribeRequest {})?)
            .await
            .with_context(|| format!("Describe RPC failed against {}", meta.provider_url))?
            .into_inner())
    }

    /// Per-release variables this project supplies for the destination.
    ///
    /// A destination describes a *place* — an account, a region, a cluster —
    /// and is shared by every project that releases there. What differs per
    /// project is small: which service, which container. Those come from the
    /// project's own `forest.cue`, rendered by `release prepare` into
    /// `forest/config.json` next to whatever manifests the destination needs.
    /// Returned flattened to strings so project-specific keys can be added to
    /// the provider request. Destination-owned metadata always takes precedence.
    /// Absent config is normal: most destination types carry everything they
    /// need on the destination itself.
    ///
    /// Read from the declaration `annotate` recorded, which is the same record
    /// the scheduler filtered this release's destinations with — so the config a
    /// destination receives and the reason it was chosen cannot come apart.
    /// Artifacts annotated before that column existed have nothing recorded and
    /// fall back to scanning the files, unchanged.
    async fn release_config(
        &self,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> HashMap<String, String> {
        match crate::services::destination_selector::recorded_declaration(
            &self.db,
            &release.artifact,
        )
        .await
        {
            Ok(Some(items)) => {
                return crate::services::destination_selector::config_for_destination(
                    &items,
                    &destination.environment,
                    &destination.name,
                    &destination.destination_type.qualified(),
                );
            }
            Ok(None) => {
                tracing::debug!("no recorded declaration for this artifact — scanning its files");
            }
            Err(e) => {
                // Fall through rather than fail: a release that cannot read its
                // config should still be able to run on the destination's own
                // metadata, which is what every release did before projects
                // could contribute config at all.
                tracing::warn!("could not read the recorded declaration: {e:#}");
            }
        }

        let files = match self
            .artifact_files
            .get_files_for_release(&release.artifact, &destination.environment)
            .await
        {
            Ok(files) => files,
            Err(e) => {
                tracing::debug!("no artifact files for this release: {e:#}");
                return HashMap::new();
            }
        };

        let destination_type = destination.destination_type.qualified();
        for (path, content) in files {
            if !path.ends_with("forest/config.json") {
                continue;
            }

            let Ok(item) = serde_json::from_str::<
                crate::services::destination_selector::DeploymentItem,
            >(&content) else {
                continue;
            };

            // Legacy file scans use the same type-and-name opt-in as the
            // scheduler; an untyped item cannot supply generic provider config.
            if item.env != destination.environment
                || !crate::services::destination_selector::selects(
                    &item,
                    &destination.name,
                    &destination_type,
                )
            {
                continue;
            }

            let Some(config) = item.config.as_ref() else {
                continue;
            };

            return flatten_config(config).unwrap_or_default();
        }

        HashMap::new()
    }

    /// Return only the exact output of an approved ancestor plan for this
    /// execution's intent, destination and artifact. No ancestor means a
    /// direct, ungated deploy; an ancestor without a valid seal must fail.
    async fn approved_plan_output(
        db: &sqlx::PgPool,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<String> {
        let (definitions, states, deploy_stage): (
            Option<serde_json::Value>,
            Option<serde_json::Value>,
            Option<String>,
        ) = sqlx::query_as(
            "SELECT ri.stages, ri.stage_states, rs.stage_id \
             FROM release_states rs JOIN release_intents ri ON ri.id = rs.release_intent_id \
             WHERE rs.release_id = $1 AND rs.release_intent_id = $2 \
               AND rs.destination_id = $3 AND rs.artifact_id = $4",
        )
        .bind(release.id)
        .bind(release.release_intent_id)
        .bind(release.destination_id)
        .bind(release.artifact)
        .fetch_one(db)
        .await
        .context("find generic provider deploy stage")?;

        let Some(deploy_stage) = deploy_stage else {
            anyhow::ensure!(definitions.is_none(), "pipeline deploy has no stage");
            return Ok(String::new());
        };
        let definitions: PipelineStages =
            serde_json::from_value(definitions.context("pipeline has no stages")?)?;
        let states: StageStates =
            serde_json::from_value(states.context("pipeline has no stage states")?)?;
        let deploy = definitions
            .get(&deploy_stage)
            .context("deploy stage is missing")?;
        anyhow::ensure!(
            matches!(deploy.config, StageConfig::Deploy { .. }),
            "release is not in a deploy stage"
        );

        let mut stack: Vec<&str> = deploy.depends_on.iter().map(String::as_str).collect();
        let mut visited = HashSet::new();
        let mut plan_stage = None;
        while let Some(stage_id) = stack.pop() {
            if !visited.insert(stage_id) {
                continue;
            }
            let stage = definitions
                .get(stage_id)
                .context("pipeline dependency is missing")?;
            if let StageConfig::Plan { environment, .. } = &stage.config
                && environment == &destination.environment
            {
                anyhow::ensure!(
                    plan_stage.replace(stage_id).is_none(),
                    "multiple ancestor plans target this destination"
                );
            }
            stack.extend(stage.depends_on.iter().map(String::as_str));
        }
        let Some(plan_stage) = plan_stage else {
            return Ok(String::new());
        };
        let state = states
            .get(plan_stage)
            .context("plan stage state is missing")?;
        anyhow::ensure!(
            state.status == StageStatus::Succeeded
                && state.approval_status == Some(ApprovalStatus::Approved),
            "generic provider deploy requires an approved plan"
        );
        let (plan_id, output): (uuid::Uuid, String) = sqlx::query_as(
            "SELECT release_id, plan_output FROM release_states \
             WHERE release_intent_id = $1 AND destination_id = $2 AND artifact_id = $3 \
               AND stage_id = $4 AND mode = 'plan' AND status = 'SUCCEEDED' \
               AND plan_output IS NOT NULL",
        )
        .bind(release.release_intent_id)
        .bind(release.destination_id)
        .bind(release.artifact)
        .bind(plan_stage)
        .fetch_one(db)
        .await
        .context("approved plan has no successful output for this destination and artifact")?;
        anyhow::ensure!(
            state
                .release_ids
                .as_ref()
                .is_some_and(|ids| ids.contains(&plan_id.to_string())),
            "approved plan does not belong to this stage"
        );
        anyhow::ensure!(!output.is_empty(), "approved plan output is empty");
        Ok(output)
    }

    /// Run `Execute`, streaming log lines into the release log.
    ///
    /// Returns the plan text for `RELEASE_MODE_PLAN`.
    async fn execute(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
        meta: &GenericMetadata,
        mode: ReleaseMode,
    ) -> anyhow::Result<Option<String>> {
        let mut client = self.connect(meta).await?;

        let metadata = provider_metadata(
            meta,
            self.release_config(release, destination).await,
            &destination.sensitive_keys,
        );
        let approved_plan_output = if mode == ReleaseMode::Deploy {
            Self::approved_plan_output(&self.db, release, destination).await?
        } else {
            String::new()
        };
        let provider_release = provider_release(&self.db, release).await?;

        // A provider that needs the release's artifacts calls back with this.
        // Most act on infrastructure that already exists and never use it.
        let release_token = self
            .release_tokens
            .create_token(
                ReleaseTokenScope {
                    release_id: release.id,
                    release_intent_id: release.release_intent_id,
                    artifact_id: release.artifact,
                    destination_id: release.destination_id,
                    project_id: release.project_id,
                    environment: destination.environment.clone(),
                    runner_id: format!("generic-provider:{}", destination.name),
                },
                TOKEN_TTL,
            )
            .await
            .context("failed to mint a release token for the provider")?;

        let request = ExecuteRequest {
            mode: mode as i32,
            destination: Some(ProviderDestination {
                name: destination.name.clone(),
                environment: destination.environment.clone(),
                organisation: destination.organisation.clone(),
                metadata,
            }),
            release: Some(provider_release),
            approved_plan_output,
            release_token,
            forest_addr: self.external_host.clone(),
        };

        let mut stream = client
            .execute(Self::authed(meta, request)?)
            .await
            .with_context(|| format!("Execute RPC failed against {}", meta.provider_url))?
            .into_inner();

        let mut outcome = None;

        while let Some(event) = stream
            .message()
            .await
            .context("provider stream failed mid-release")?
        {
            match event.event {
                Some(Event::Log(line)) => {
                    if line.channel == "stderr" {
                        logger.log_stderr(&line.line);
                    } else {
                        logger.log_stdout(&line.line);
                    }
                }
                Some(Event::Signal(sig)) => {
                    if mode == ReleaseMode::Plan {
                        // A dry-run is not evidence that a rollout is healthy.
                        continue;
                    }
                    // A provider says what it saw; forest knows which release
                    // it is about. Recorded here rather than by the provider
                    // calling back, so the provider protocol stays free of any
                    // forest dependency.
                    let status = sig.status.trim().to_ascii_uppercase();
                    if !crate::services::release_signals::is_valid_status(&status) {
                        // Not fatal to the release — a provider that
                        // mis-reports a signal has still deployed — but it must
                        // not be stored, or a gate would wait on a state
                        // nothing can ever match.
                        logger.log_stderr(&format!(
                            "provider reported signal '{}' with unknown status '{}'; ignoring \
                             (expected one of {:?})",
                            sig.name,
                            sig.status,
                            crate::services::release_signals::VALID_STATUSES,
                        ));
                        continue;
                    }

                    let observed_at = chrono::DateTime::parse_from_rfc3339(&sig.observed_at)
                        .map(|dt| dt.with_timezone(&chrono::Utc))
                        .unwrap_or_else(|_| chrono::Utc::now());

                    let row = crate::services::release_signals::SignalRow {
                        name: sig.name.clone(),
                        status,
                        detail: sig.detail.clone(),
                        destination_name: destination.name.clone(),
                        environment: destination.environment.clone(),
                        reported_by: meta.provider_url.clone(),
                        observed_at,
                    };

                    if let Err(e) = crate::services::release_signals::report(
                        &self.db,
                        &self.nats,
                        release.release_intent_id,
                        release.id,
                        &destination.organisation,
                        &release.project,
                        &row,
                    )
                    .await
                    {
                        // Same reasoning: the deploy is what matters, and a
                        // signal that failed to store is recoverable by the
                        // next report. Losing the release over it would not be.
                        logger
                            .log_stderr(&format!("failed to record signal '{}': {e:#}", sig.name));
                    }
                }
                Some(Event::Outcome(o)) => {
                    if outcome.is_some() {
                        anyhow::bail!(
                            "provider at {} sent more than one outcome; refusing to guess which \
                             one is authoritative",
                            meta.provider_url,
                        );
                    }
                    outcome = Some(o);
                }
                None => {}
            }
        }

        // A provider that dies mid-release has not succeeded, and forest does not
        // assume otherwise. This is the same reasoning as treating an ECS
        // rollout timeout as failure: absence of a result is not a good result.
        let outcome = outcome.with_context(|| {
            format!(
                "provider at {} closed the stream without reporting an outcome",
                meta.provider_url,
            )
        })?;

        if !outcome.success {
            anyhow::bail!(
                "provider reported failure: {}",
                if outcome.error_message.is_empty() {
                    "no reason given".to_string()
                } else {
                    outcome.error_message
                },
            );
        }

        if mode == ReleaseMode::Plan {
            anyhow::ensure!(
                !outcome.plan_output.is_empty(),
                "provider at {} returned no plan output to approve",
                meta.provider_url
            );
            Ok(Some(outcome.plan_output))
        } else {
            Ok(None)
        }
    }
}

#[async_trait::async_trait]
impl DestinationEdge for GenericV1Destination {
    fn name(&self) -> DestinationIndex {
        DestinationIndex {
            organisation: "forest".into(),
            name: "generic".into(),
            version: 1,
        }
    }

    fn description(&self) -> &str {
        "Delegate releases to an external gRPC provider implementing forest.provider.v1.DestinationProvider."
    }

    fn metadata_schema(&self) -> Vec<forest_models::MetadataFieldSchema> {
        metadata_schema()
    }

    async fn metadata_sensitive_keys(
        &self,
        metadata: &HashMap<String, String>,
    ) -> anyhow::Result<Vec<String>> {
        let meta = GenericMetadata::from_metadata(metadata)?;
        let described = self.describe(&meta).await?;
        Ok(sensitive_keys_from_describe(described))
    }

    /// Ask the provider what it accepts, then hold the destination to it.
    ///
    /// The schema lives with the implementation, so this is the single source of
    /// truth — there is no copy of it in forest's configuration to drift.
    async fn validate_metadata(&self, metadata: &HashMap<String, String>) -> anyhow::Result<()> {
        let meta = GenericMetadata::from_metadata(metadata)?;

        let described = self.describe(&meta).await.context(
            "could not reach the provider to read its metadata schema; a generic destination \
             cannot be created against a provider that is not answering",
        )?;

        let missing: Vec<_> = described
            .fields
            .iter()
            .filter(|f| f.required)
            .filter(|f| {
                meta.passthrough
                    .get(&f.name)
                    .map(|v| v.trim().is_empty())
                    .unwrap_or(true)
            })
            .map(|f| f.name.as_str())
            .collect();

        if !missing.is_empty() {
            anyhow::bail!(
                "provider at {} requires metadata: {}",
                meta.provider_url,
                missing.join(", "),
            );
        }

        Ok(())
    }

    async fn prepare(
        &self,
        logger: &DestinationLogger,
        _release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<()> {
        let meta = GenericMetadata::from_metadata(&destination.metadata)?;

        logger.log_stdout(&format!(
            "generic@1: describing provider at {}",
            meta.provider_url,
        ));

        let described = self.describe(&meta).await?;

        logger.log_stdout(&format!(
            "generic@1: provider ready — {} (plan {})",
            if described.type_name.is_empty() {
                described.description
            } else {
                described.type_name
            },
            if described.supports_plan {
                "supported"
            } else {
                "unsupported"
            },
        ));

        Ok(())
    }

    async fn release(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<()> {
        let meta = GenericMetadata::from_metadata(&destination.metadata)?;
        self.execute(logger, release, destination, &meta, ReleaseMode::Deploy)
            .await?;
        Ok(())
    }

    async fn plan(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<Option<String>> {
        let meta = GenericMetadata::from_metadata(&destination.metadata)?;

        let described = self.describe(&meta).await?;
        if !described.supports_plan {
            anyhow::bail!(
                "provider at {} does not support plan mode",
                meta.provider_url,
            );
        }

        self.execute(logger, release, destination, &meta, ReleaseMode::Plan)
            .await
    }

    fn supports_plan(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The allowlist is read from the process environment, which every test in
    /// this module shares. Without this they race and fail at random.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        // A panicking test poisons the lock; that test has already failed and
        // there is no shared state to corrupt, so don't fail the rest too.
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn with_allowlist<T>(value: &str, f: impl FnOnce() -> T) -> T {
        let _guard = lock_env();
        unsafe { std::env::set_var(ALLOWED_HOSTS_ENV, value) };
        let out = f();
        unsafe { std::env::remove_var(ALLOWED_HOSTS_ENV) };
        out
    }

    #[test]
    fn authority_is_extracted_from_a_url() {
        assert_eq!(
            authority_of("http://a.internal:4060").unwrap(),
            "a.internal:4060"
        );
        assert_eq!(
            authority_of("https://a.internal/path").unwrap(),
            "a.internal"
        );
        assert_eq!(authority_of("http://A.Internal").unwrap(), "a.internal");
        assert!(
            authority_of("a.internal:4060").is_err(),
            "a scheme is required"
        );
        assert!(authority_of("http://").is_err());
    }

    #[test]
    fn userinfo_cannot_smuggle_a_host_past_the_allowlist() {
        // `evil.com@trusted.internal` resolves to trusted.internal, and the
        // reverse ordering must not read as trusted either.
        assert_eq!(
            authority_of("http://evil.com@trusted.internal").unwrap(),
            "trusted.internal",
        );
        with_allowlist("trusted.internal", || {
            assert!(check_host_allowed("http://trusted.internal@evil.com").is_err());
        });
    }

    #[test]
    fn an_unset_allowlist_permits_nothing() {
        let _guard = lock_env();
        unsafe { std::env::remove_var(ALLOWED_HOSTS_ENV) };
        let err = check_host_allowed("http://a.internal:4060")
            .unwrap_err()
            .to_string();
        assert!(err.contains(ALLOWED_HOSTS_ENV), "got: {err}");
    }

    #[test]
    fn exact_hosts_match_with_or_without_a_port() {
        with_allowlist("a.internal", || {
            assert!(check_host_allowed("http://a.internal:4060").is_ok());
            assert!(check_host_allowed("http://a.internal").is_ok());
            assert!(check_host_allowed("http://b.internal").is_err());
        });

        // Pinning the port is stricter and must stay strict.
        with_allowlist("a.internal:4060", || {
            assert!(check_host_allowed("http://a.internal:4060").is_ok());
            assert!(check_host_allowed("http://a.internal:9999").is_err());
        });
    }

    #[test]
    fn wildcards_match_subdomains_only() {
        with_allowlist("*.internal", || {
            assert!(check_host_allowed("http://a.internal:4060").is_ok());
            assert!(check_host_allowed("http://a.b.internal").is_ok());
            // The bare suffix is not a subdomain of itself…
            assert!(check_host_allowed("http://internal").is_err());
            // …and neither is a host that merely ends with the same letters.
            assert!(check_host_allowed("http://notinternal").is_err());
            assert!(check_host_allowed("http://evil.com").is_err());
        });
    }

    #[test]
    fn provider_keys_are_not_passed_through_to_the_provider() {
        with_allowlist("*.internal", || {
            let metadata = HashMap::from([
                (
                    "provider_url".to_string(),
                    "http://ecs.internal:4060".to_string(),
                ),
                ("provider_token".to_string(), "s3cret".to_string()),
                ("cluster".to_string(), "infrastructure-platform".to_string()),
            ]);

            let meta = GenericMetadata::from_metadata(&metadata).unwrap();
            assert_eq!(meta.provider_token.as_deref(), Some("s3cret"));
            assert_eq!(meta.passthrough.len(), 1);
            assert_eq!(
                meta.passthrough.get("cluster").unwrap(),
                "infrastructure-platform"
            );
            assert!(!meta.passthrough.contains_key("provider_token"));
        });
    }

    #[test]
    fn debug_output_never_contains_the_provider_token() {
        with_allowlist("*.internal", || {
            let metadata = HashMap::from([
                (
                    "provider_url".to_string(),
                    "http://ecs.internal:4060".to_string(),
                ),
                ("provider_token".to_string(), "s3cret".to_string()),
            ]);
            let rendered = format!("{:?}", GenericMetadata::from_metadata(&metadata).unwrap());
            assert!(
                !rendered.contains("s3cret"),
                "credential leaked: {rendered}"
            );
            assert!(rendered.contains("<redacted>"), "got: {rendered}");
        });
    }

    #[test]
    fn project_config_cannot_override_destination_settings_or_provider_credentials() {
        with_allowlist("*.internal", || {
            let meta = GenericMetadata::from_metadata(&HashMap::from([
                ("provider_url".into(), "http://ecs.internal:4060".into()),
                ("provider_token".into(), "auth-token".into()),
                ("cluster".into(), "production".into()),
                ("grund_token".into(), "destination-secret".into()),
            ]))
            .unwrap();
            let metadata = provider_metadata(
                &meta,
                HashMap::from([
                    ("cluster".into(), "attacker-cluster".into()),
                    ("grund_token".into(), "attacker-secret".into()),
                    ("provider_url".into(), "http://attacker.invalid".into()),
                    ("provider_token".into(), "attacker-token".into()),
                    ("service".into(), "my-app".into()),
                ]),
                &["grund_token".into()],
            );
            assert_eq!(metadata["cluster"], "production");
            assert_eq!(metadata["grund_token"], "destination-secret");
            assert_eq!(metadata["service"], "my-app");
            assert!(!metadata.contains_key(PROVIDER_URL));
            assert!(!metadata.contains_key(PROVIDER_TOKEN));
            let debug = format!("{meta:?}");
            assert!(!debug.contains("destination-secret"), "{debug}");
        });
    }

    #[test]
    fn project_config_cannot_supply_an_unset_provider_credential() {
        with_allowlist("*.internal", || {
            let meta = GenericMetadata::from_metadata(&HashMap::from([(
                "provider_url".into(),
                "http://ecs.internal:4060".into(),
            )]))
            .unwrap();
            let metadata = provider_metadata(
                &meta,
                HashMap::from([("grund_token".into(), "attacker-secret".into())]),
                &["grund_token".into()],
            );
            assert!(!metadata.contains_key("grund_token"));
        });
    }

    #[test]
    fn a_missing_provider_url_is_rejected() {
        let _guard = lock_env();
        let err = GenericMetadata::from_metadata(&HashMap::new())
            .unwrap_err()
            .to_string();
        assert!(err.contains("provider_url"), "got: {err}");
    }

    #[test]
    fn the_provider_token_is_declared_sensitive() {
        let schema = metadata_schema();

        let token = schema.iter().find(|f| f.name == PROVIDER_TOKEN).unwrap();
        assert!(token.sensitive, "provider_token is a credential");
        assert!(
            !schema
                .iter()
                .find(|f| f.name == PROVIDER_URL)
                .unwrap()
                .sensitive
        );
    }

    #[test]
    fn provider_declared_credentials_are_withheld_from_destination_listing() {
        let described = DescribeResponse {
            fields: vec![
                forest_grpc_interface::provider::MetadataField {
                    name: "grund_token".into(),
                    sensitive: true,
                    ..Default::default()
                },
                forest_grpc_interface::provider::MetadataField {
                    name: "cluster".into(),
                    sensitive: false,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let keys = sensitive_keys_from_describe(described);
        let destination = Destination::new(
            "sample",
            "prod",
            "prod",
            HashMap::from([
                ("provider_token".into(), "provider-secret".into()),
                ("grund_token".into(), "grund-secret".into()),
                ("cluster".into(), "eu-west".into()),
            ]),
            forest_models::DestinationType {
                organisation: "forest".into(),
                name: "generic".into(),
                version: 1,
                description: String::new(),
                fields: metadata_schema(),
            },
        )
        .with_sensitive_keys(keys);
        let wire: forest_grpc_interface::Destination = destination.into();
        assert_eq!(
            wire.metadata.get("cluster").map(String::as_str),
            Some("eu-west")
        );
        assert!(!wire.metadata.contains_key("grund_token"));
        assert!(!wire.metadata.contains_key("provider_token"));
        assert!(wire.sensitive_keys.contains(&"grund_token".to_string()));
        assert!(!format!("{wire:?}").contains("grund-secret"));
    }

    #[sqlx::test]
    async fn annotated_source_and_stable_ids_reach_provider(pool: sqlx::PgPool) {
        let artifact_id = uuid::Uuid::new_v4();
        let intent_id = uuid::Uuid::new_v4();
        let release = ReleaseItem {
            id: uuid::Uuid::new_v4(),
            release_intent_id: intent_id,
            artifact: artifact_id,
            project_id: uuid::Uuid::new_v4(),
            destination_id: uuid::Uuid::new_v4(),
            status: "RUNNING".into(),
            project: "grund".into(),
        };
        sqlx::query(
            "INSERT INTO annotations (slug, artifact_id, metadata, source, context, project_id, ref) \
             VALUES ($1, $2, '{}'::jsonb, '{}'::jsonb, '{}'::jsonb, $3, $4)",
        )
        .bind("release-abc")
        .bind(artifact_id)
        .bind(release.project_id)
        .bind(serde_json::json!({
            "commit_sha": "deadbeef",
            "commit_branch": "main",
            "version": "v2.1"
        }))
        .execute(&pool)
        .await
        .unwrap();
        let provider = provider_release(&pool, &release).await.unwrap();
        assert_eq!(provider.slug, "release-abc");
        assert_eq!(provider.reference_commit_sha, "deadbeef");
        assert_eq!(provider.reference_commit_branch, "main");
        assert_eq!(provider.reference_version, "v2.1");
        assert_eq!(provider.release_intent_id, intent_id.to_string());
        assert_eq!(provider.artifact_id, artifact_id.to_string());
        assert_eq!(provider.release_id, release.id.to_string());

        let unannotated = ReleaseItem {
            artifact: uuid::Uuid::new_v4(),
            ..release
        };
        assert!(provider_release(&pool, &unannotated).await.is_err());
    }
    #[sqlx::test]
    async fn deploy_uses_only_its_approved_immutable_plan(pool: sqlx::PgPool) {
        use crate::services::release_pipeline::{StageDefinition, StageState};

        let intent_id = uuid::Uuid::new_v4();
        let artifact_id = uuid::Uuid::new_v4();
        let destination_id = uuid::Uuid::new_v4();
        let plan_id = uuid::Uuid::new_v4();
        let deploy_id = uuid::Uuid::new_v4();
        let project_id = uuid::Uuid::new_v4();
        let mut states = StageStates::new();
        let mut planned = StageState::pending();
        planned.status = StageStatus::Succeeded;
        planned.approval_status = Some(ApprovalStatus::Approved);
        planned.release_ids = Some(vec![plan_id.to_string()]);
        states.insert("plan-prod".into(), planned);
        let definitions = PipelineStages::from([
            (
                "plan-prod".into(),
                StageDefinition::plan("prod", false, vec![]),
            ),
            (
                "wait".into(),
                StageDefinition::wait(0, vec!["plan-prod".into()]),
            ),
            (
                "deploy-prod".into(),
                StageDefinition::deploy("prod", vec!["wait".into()]),
            ),
        ]);
        sqlx::query(
            "INSERT INTO release_intents (id, artifact, annotation_id, project_id, stages, stage_states) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(intent_id)
        .bind(artifact_id)
        .bind(uuid::Uuid::new_v4())
        .bind(project_id)
        .bind(serde_json::to_value(&definitions).unwrap())
        .bind(serde_json::to_value(&states).unwrap())
        .execute(&pool)
        .await
        .unwrap();
        let plan = r#"{"schema_version":1,"artifact_id":"pinned"}"#;
        sqlx::query(
            "INSERT INTO release_states (release_id, release_intent_id, project_id, \
             destination_id, artifact_id, stage_id, mode, status) \
             VALUES ($1, $2, $3, $4, $5, 'plan-prod', 'plan', 'SUCCEEDED')",
        )
        .bind(plan_id)
        .bind(intent_id)
        .bind(project_id)
        .bind(destination_id)
        .bind(artifact_id)
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            crate::services::release_pipeline::seal_plan_output(&pool, plan_id, plan)
                .await
                .unwrap()
        );
        sqlx::query(
            "INSERT INTO release_states (release_id, release_intent_id, project_id, \
             destination_id, artifact_id, stage_id, status) \
             VALUES ($1, $2, $3, $4, $5, 'deploy-prod', 'RUNNING')",
        )
        .bind(deploy_id)
        .bind(intent_id)
        .bind(project_id)
        .bind(destination_id)
        .bind(artifact_id)
        .execute(&pool)
        .await
        .unwrap();
        let mut release = ReleaseItem {
            id: deploy_id,
            release_intent_id: intent_id,
            artifact: artifact_id,
            project_id,
            destination_id,
            status: "RUNNING".into(),
            project: "sample".into(),
        };
        let destination = Destination::new(
            "sample",
            "prod",
            "prod",
            HashMap::new(),
            forest_models::DestinationType {
                organisation: "forest".into(),
                name: "generic".into(),
                version: 1,
                description: String::new(),
                fields: vec![],
            },
        );
        assert_eq!(
            GenericV1Destination::approved_plan_output(&pool, &release, &destination)
                .await
                .unwrap(),
            plan,
        );
        assert!(
            !crate::services::release_pipeline::seal_plan_output(&pool, plan_id, "replacement")
                .await
                .unwrap(),
            "approved output is immutable"
        );
        assert_eq!(
            GenericV1Destination::approved_plan_output(&pool, &release, &destination)
                .await
                .unwrap(),
            plan,
        );

        states.get_mut("plan-prod").unwrap().approval_status =
            Some(ApprovalStatus::AwaitingApproval);
        sqlx::query("UPDATE release_intents SET stage_states = $2 WHERE id = $1")
            .bind(intent_id)
            .bind(serde_json::to_value(&states).unwrap())
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            GenericV1Destination::approved_plan_output(&pool, &release, &destination)
                .await
                .is_err(),
            "unapproved plans cannot open deploy"
        );
        states.get_mut("plan-prod").unwrap().approval_status = Some(ApprovalStatus::Approved);
        sqlx::query("UPDATE release_intents SET stage_states = $2 WHERE id = $1")
            .bind(intent_id)
            .bind(serde_json::to_value(&states).unwrap())
            .execute(&pool)
            .await
            .unwrap();
        release.destination_id = uuid::Uuid::new_v4();
        sqlx::query("UPDATE release_states SET destination_id = $2 WHERE release_id = $1")
            .bind(deploy_id)
            .bind(release.destination_id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            GenericV1Destination::approved_plan_output(&pool, &release, &destination)
                .await
                .is_err(),
            "another destination cannot borrow this plan"
        );
        release.destination_id = destination_id;
        sqlx::query("UPDATE release_states SET destination_id = $2 WHERE release_id = $1")
            .bind(deploy_id)
            .bind(destination_id)
            .execute(&pool)
            .await
            .unwrap();
        release.artifact = uuid::Uuid::new_v4();
        sqlx::query("UPDATE release_states SET artifact_id = $2 WHERE release_id = $1")
            .bind(deploy_id)
            .bind(release.artifact)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            GenericV1Destination::approved_plan_output(&pool, &release, &destination)
                .await
                .is_err(),
            "another artifact cannot borrow this plan"
        );
        release.artifact = artifact_id;
        sqlx::query("UPDATE release_states SET artifact_id = $2 WHERE release_id = $1")
            .bind(deploy_id)
            .bind(artifact_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE release_states SET stage_id = NULL WHERE release_id = $1")
            .bind(deploy_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE release_intents SET stages = NULL WHERE id = $1")
            .bind(intent_id)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            GenericV1Destination::approved_plan_output(&pool, &release, &destination)
                .await
                .unwrap(),
            "",
            "ungated direct deploy has no approved plan"
        );
    }
}
