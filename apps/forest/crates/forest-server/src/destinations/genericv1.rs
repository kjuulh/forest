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

use std::{collections::HashMap, time::Duration};

use anyhow::Context;
use forest_grpc_interface::provider::{
    DescribeRequest, DescribeResponse, Destination as ProviderDestination, ExecuteRequest,
    Release as ProviderRelease, ReleaseMode, destination_provider_client::DestinationProviderClient,
    execute_event::Event,
};
use forest_models::Destination;

use crate::{
    destinations::{DestinationEdge, DestinationIndex, logger::DestinationLogger},
    services::{
        release_registry::ReleaseItem,
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
            .field("passthrough", &self.passthrough)
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

    let authority = authority_of(url)
        .with_context(|| format!("'{PROVIDER_URL}' is not a valid URL: {url}"))?;

    if allowed.iter().any(|pattern| matches_host(pattern, &authority)) {
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
    let host = authority.rsplit_once(':').map(|(h, _)| h).unwrap_or(authority);

    if let Some(suffix) = pattern.strip_prefix("*.") {
        // `*.internal` matches `a.internal` and `a.internal:4050`, not `internal`
        // itself and not `notinternal`.
        return host.ends_with(&format!(".{suffix}"));
    }

    // An exact pattern matches either with or without the port.
    pattern == authority || pattern == host
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
            .with_context(|| {
                format!("Describe RPC failed against {}", meta.provider_url)
            })?
            .into_inner())
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
                metadata: meta.passthrough.clone(),
            }),
            release: Some(ProviderRelease {
                release_id: release.id.to_string(),
                project: release.project.clone(),
                slug: String::new(),
                reference_commit_sha: String::new(),
                reference_commit_branch: String::new(),
                reference_version: String::new(),
            }),
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

        Ok(match mode {
            ReleaseMode::Plan if !outcome.plan_output.is_empty() => Some(outcome.plan_output),
            _ => None,
        })
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
        assert_eq!(authority_of("http://a.internal:4060").unwrap(), "a.internal:4060");
        assert_eq!(authority_of("https://a.internal/path").unwrap(), "a.internal");
        assert_eq!(authority_of("http://A.Internal").unwrap(), "a.internal");
        assert!(authority_of("a.internal:4060").is_err(), "a scheme is required");
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
                ("provider_url".to_string(), "http://ecs.internal:4060".to_string()),
                ("provider_token".to_string(), "s3cret".to_string()),
                ("cluster".to_string(), "infrastructure-platform".to_string()),
            ]);

            let meta = GenericMetadata::from_metadata(&metadata).unwrap();
            assert_eq!(meta.provider_token.as_deref(), Some("s3cret"));
            assert_eq!(meta.passthrough.len(), 1);
            assert_eq!(meta.passthrough.get("cluster").unwrap(), "infrastructure-platform");
            assert!(!meta.passthrough.contains_key("provider_token"));
        });
    }

    #[test]
    fn debug_output_never_contains_the_provider_token() {
        with_allowlist("*.internal", || {
            let metadata = HashMap::from([
                ("provider_url".to_string(), "http://ecs.internal:4060".to_string()),
                ("provider_token".to_string(), "s3cret".to_string()),
            ]);
            let rendered = format!("{:?}", GenericMetadata::from_metadata(&metadata).unwrap());
            assert!(!rendered.contains("s3cret"), "credential leaked: {rendered}");
            assert!(rendered.contains("<redacted>"), "got: {rendered}");
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
        assert!(!schema.iter().find(|f| f.name == PROVIDER_URL).unwrap().sensitive);
    }
}
