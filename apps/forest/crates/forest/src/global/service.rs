//! Global-tools service — the effectful orchestrator.
//!
//! Lives between the pure-core modules (`resolver`, `manifest`, …) and the
//! CLI commands. Reads/writes the user config + lockfile, hits the registry,
//! manages shims, and dispatches `forest global run` to the right binary.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};

use crate::global::{
    cache::{BinaryCache, sha256_hex},
    cue_eval::CueEvaluator,
    extract,
    fs::{atomic_write, atomic_write_executable, ensure_dir, read_optional, remove_if_present},
    lockfile::{GlobalLockEntry, GlobalLockFile},
    manifest::{self, Archive, ComponentShape, Manifest, PlatformKey, ToolFacet},
    paths::GlobalPaths,
    platform,
    resolver::{self, FetchPlan, Plan, PlanError},
    shim::{QualifiedRef, shim_script_for},
    user_config::{Dependency, UserConfig, parse as parse_user_config},
};
use crate::grpc::{GrpcClient, GrpcClientState};
use crate::state::State;

/// Top-level service holding the resolved paths, the cue evaluator, the
/// gRPC client, and the binary cache.
pub struct GlobalService {
    pub paths: GlobalPaths,
    pub cue: CueEvaluator,
    pub cache: BinaryCache,
    pub grpc: GrpcClient,
}

impl GlobalService {
    pub fn from_state(state: &State) -> Result<Self> {
        let paths = GlobalPaths::from_env()?;
        Ok(Self {
            cache: BinaryCache::new(paths.clone()),
            cue: CueEvaluator::new(),
            grpc: state.grpc_client(),
            paths,
        })
    }

    // --- config I/O -------------------------------------------------------

    /// Load the user-global `forest.cue`. Returns an empty default when the
    /// file is missing (first-run case).
    pub async fn load_user_config(&self) -> Result<UserConfig> {
        let path = self.paths.user_config_cue();
        if read_optional(&path).await?.is_none() {
            return Ok(UserConfig::default());
        }
        let json = self
            .cue
            .eval_to_json(&path)
            .await
            .with_context(|| format!("evaluating {}", path.display()))?;
        // `cue eval` produces the package's top-level value. The schema
        // wraps everything in `config: sdk.#UserConfig`, so the emitted
        // JSON looks like `{"config": {...}}`.
        let cfg = parse_user_config(&json)
            .map_err(|e| anyhow!("parsing forest.cue: {e:?}"))?;
        Ok(cfg)
    }

    /// Persist a `UserConfig` by writing the CUE form (deterministic).
    ///
    /// Bootstraps `cue.mod/module.cue` next to `forest.cue` on first write
    /// so the `import sdk "forest.sh/forest/sdk@v0"` directive can resolve.
    pub async fn save_user_config(&self, cfg: &UserConfig) -> Result<()> {
        ensure_dir(self.paths.config_dir()).await?;

        // Ensure the cue.mod is present so `cue eval` can resolve the sdk
        // import. We can't write the SDK content itself (it lives in the
        // server's CUE registry); we just declare the module's identity +
        // language version. CUE_REGISTRY env var supplies the rest.
        let cue_mod_dir = self.paths.config_dir().join("cue.mod");
        let module_file = cue_mod_dir.join("module.cue");
        if read_optional(&module_file).await?.is_none() {
            ensure_dir(&cue_mod_dir).await?;
            atomic_write(
                &module_file,
                b"module: \"forest.sh/user-config\"\nlanguage: version: \"v0.10.0\"\n",
            )
            .await?;
        }

        let cue_text = render_user_config(cfg);
        atomic_write(&self.paths.user_config_cue(), cue_text.as_bytes()).await?;
        Ok(())
    }

    pub async fn load_lockfile(&self) -> Result<GlobalLockFile> {
        let text = match read_optional(&self.paths.lockfile()).await? {
            Some(t) => t,
            None => return Ok(GlobalLockFile::default()),
        };
        let lock = GlobalLockFile::parse(&text)
            .map_err(|e| anyhow!("parsing global lockfile: {e:?}"))?;
        Ok(lock)
    }

    pub async fn save_lockfile(&self, lock: &GlobalLockFile) -> Result<()> {
        ensure_dir(self.paths.state_dir()).await?;
        atomic_write(&self.paths.lockfile(), lock.serialize().as_bytes()).await?;
        Ok(())
    }

    // --- manifest fetch ---------------------------------------------------

    pub async fn fetch_manifest(
        &self,
        organisation: &str,
        name: &str,
        version: &str,
    ) -> Result<Manifest> {
        let raw = self
            .grpc
            .get_component_manifest(organisation, name, version)
            .await
            .with_context(|| {
                format!("fetching manifest for {organisation}/{name}@{version}")
            })?;
        // Pre-spec manifests omit `kind` — synthesize a `kind: "binary"`
        // when missing so the parser can succeed for legacy components.
        let raw = ensure_kind_field(&raw);
        let manifest = manifest::parse(&raw)
            .map_err(|e| anyhow!("parsing manifest for {organisation}/{name}@{version}: {e:?}"))?;
        Ok(manifest)
    }

    // --- shim management --------------------------------------------------

    pub fn shim_path(&self, shim_name: &str) -> PathBuf {
        self.paths.shims_dir().join(shim_name)
    }

    pub async fn write_shim(&self, shim_name: &str, qref: &QualifiedRef) -> Result<()> {
        ensure_dir(&self.paths.shims_dir()).await?;
        let body = shim_script_for(qref);
        atomic_write_executable(&self.shim_path(shim_name), body.as_bytes()).await?;
        Ok(())
    }

    pub async fn delete_shim(&self, shim_name: &str) -> Result<()> {
        remove_if_present(&self.shim_path(shim_name)).await
    }

    // --- the lazy resolve+run path ---------------------------------------

    /// Resolve a `(org, name, version)` ref to a cached binary, fetching
    /// on miss. Returns the on-disk path of the verified binary.
    ///
    /// **Offline-capable warm path (§1a.9):** if the lockfile already
    /// pins this `(org, name, version, os, arch)` to a sha AND that sha
    /// is present in the cache, return immediately without contacting
    /// the registry. This honours T1 (content-address trust) and means
    /// `forest global run` works fully offline once a tool is cached.
    pub async fn resolve_to_cached_path(
        &self,
        qref: &QualifiedRef,
        version: &str,
    ) -> Result<PathBuf> {
        let host = platform::current()
            .ok_or_else(|| anyhow!("unsupported host platform"))?;
        let lockfile = self.load_lockfile().await.unwrap_or_default();

        // Warm-path shortcut: cache hit on lockfile pin → never touch network.
        if let Some(pinned_sha) = lockfile.get(
            &qref.organisation,
            &qref.name,
            version,
            platform::os_str(host.os),
            platform::arch_str(host.arch),
        ) {
            if let Some(p) = self.cache.read_by_sha(pinned_sha).await? {
                return Ok(p);
            }
        }

        // Cold path: lockfile miss OR cache miss → need the manifest to
        // know how to fetch + what to verify against.
        let manifest = self
            .fetch_manifest(&qref.organisation, &qref.name, version)
            .await?;
        let user_config = self.load_user_config().await.unwrap_or_default();

        let plan = resolver::plan(&user_config, &lockfile, &manifest, qref, version, host);
        let (expected_sha, fetch) = match plan {
            Plan::Resolve {
                expected_sha,
                fetch_if_missing,
            } => (expected_sha, fetch_if_missing),
            Plan::Error(PlanError::PlatformNotAvailable {
                requested,
                available,
            }) => {
                let available_s = available
                    .iter()
                    .map(|p| {
                        format!("{}/{}", platform::os_str(p.os), platform::arch_str(p.arch))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                anyhow::bail!(
                    "tool {}/{}@{} not available for {}/{}; published for: {}",
                    qref.organisation,
                    qref.name,
                    version,
                    platform::os_str(requested.os),
                    platform::arch_str(requested.arch),
                    available_s,
                );
            }
            Plan::Error(PlanError::ShapeNotInstallable { shape }) => {
                anyhow::bail!(
                    "{}/{} cannot be installed as a global tool (shape={:?})",
                    qref.organisation,
                    qref.name,
                    shape,
                );
            }
        };

        // Cache hit by sha (e.g. same content under a different org/version)
        // OR cold fetch — either way we now know the sha and can pin the
        // lockfile so the next run takes the offline warm path.
        let cached_path = if let Some(p) = self.cache.read_by_sha(&expected_sha).await? {
            p
        } else {
            let bytes = match fetch {
                FetchPlan::Registry => {
                    self.grpc
                        .download_component_binary(
                            &qref.organisation,
                            &qref.name,
                            version,
                            platform::os_str(host.os),
                            platform::arch_str(host.arch),
                        )
                        .await
                        .with_context(|| {
                            format!("downloading {}/{}@{}", qref.organisation, qref.name, version)
                        })?
                }
                FetchPlan::Url {
                    url,
                    archive,
                    binary_in_archive,
                    archive_sha,
                } => {
                    let body = http_get(&url).await?;
                    if let Some(expected_archive_sha) = archive_sha {
                        let actual_archive_sha = sha256_hex(&body);
                        let want = expected_archive_sha
                            .strip_prefix("sha256:")
                            .unwrap_or(&expected_archive_sha);
                        if actual_archive_sha != want {
                            anyhow::bail!(
                                "archive_sha256 mismatch for {url}: expected={want} actual={actual_archive_sha}"
                            );
                        }
                    }
                    extract_from_archive(&body, archive, binary_in_archive.as_deref())?
                }
            };
            self.cache.finalize(&bytes, &expected_sha).await?
        };

        // Update the lockfile with the version actually executed. This must
        // run for the cache-hit-by-sha branch too — otherwise the next run
        // misses the warm path, refetches the manifest, hits the same sha,
        // and loops forever.
        let mut lock = self.load_lockfile().await.unwrap_or_default();
        lock.insert(GlobalLockEntry {
            organisation: qref.organisation.clone(),
            name: qref.name.clone(),
            version: version.to_string(),
            os: platform::os_str(host.os).to_string(),
            arch: platform::arch_str(host.arch).to_string(),
            sha256: format!(
                "sha256:{}",
                expected_sha
                    .strip_prefix("sha256:")
                    .unwrap_or(&expected_sha)
            ),
        });
        self.save_lockfile(&lock).await?;

        Ok(cached_path)
    }
}

// --- helpers --------------------------------------------------------------

/// Render a UserConfig to its CUE text form. Stable key order.
///
/// Intentionally avoids importing `sdk.#UserConfig`: the user's machine
/// may not have a CUE registry configured. The runtime cares about
/// *structure*, not schema enforcement — schema validation happens at
/// `forest global add` time when we know what we're writing.
pub fn render_user_config(cfg: &UserConfig) -> String {
    let mut out = String::from("package forest\n\nconfig: {\n");

    if !cfg.user.is_empty() {
        out.push_str("\tuser: {\n");
        for (k, v) in &cfg.user {
            out.push_str(&format!("\t\t{}: {}\n", cue_string(k), cue_string(v)));
        }
        out.push_str("\t}\n");
    }

    if !cfg.dependencies.is_empty() {
        out.push_str("\tdependencies: {\n");
        for (k, dep) in &cfg.dependencies {
            out.push_str(&format!("\t\t{}: {{\n", cue_string(k)));
            out.push_str(&format!(
                "\t\t\tversion: {}\n",
                cue_string(&dep.version)
            ));
            if let Some(shim) = &dep.shim_name {
                out.push_str(&format!("\t\t\tshim_name: {}\n", cue_string(shim)));
            }
            out.push_str("\t\t}\n");
        }
        out.push_str("\t}\n");
    }

    if !cfg.org_catalog.is_empty() {
        out.push_str("\torg_catalog: {\n");
        for (org, cat) in &cfg.org_catalog {
            out.push_str(&format!("\t\t{}: {{\n", cue_string(org)));
            out.push_str(&format!("\t\t\tenabled: {}\n", cat.enabled));
            if !cat.banned.is_empty() {
                let items = cat
                    .banned
                    .iter()
                    .map(|x| cue_string(x))
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push_str(&format!("\t\t\tbanned: [{items}]\n"));
            }
            if !cat.pins.is_empty() {
                out.push_str("\t\t\tpins: {\n");
                for (k, v) in &cat.pins {
                    out.push_str(&format!(
                        "\t\t\t\t{}: {}\n",
                        cue_string(k),
                        cue_string(v)
                    ));
                }
                out.push_str("\t\t\t}\n");
            }
            if !cat.aliases.is_empty() {
                out.push_str("\t\t\taliases: {\n");
                for (k, v) in &cat.aliases {
                    out.push_str(&format!(
                        "\t\t\t\t{}: {}\n",
                        cue_string(k),
                        cue_string(v)
                    ));
                }
                out.push_str("\t\t\t}\n");
            }
            out.push_str("\t\t}\n");
        }
        out.push_str("\t}\n");
    }

    out.push_str("}\n");
    out
}

fn cue_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn ensure_kind_field(raw: &str) -> String {
    // If `kind` is already present, leave the JSON alone.
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return r#"{"kind": "binary"}"#.to_string();
    }
    let Ok(v): Result<serde_json::Value, _> = serde_json::from_str(trimmed) else {
        return raw.to_string();
    };
    let serde_json::Value::Object(mut map) = v else {
        return raw.to_string();
    };
    if !map.contains_key("kind") {
        map.insert("kind".into(), serde_json::Value::String("binary".into()));
    }
    serde_json::to_string(&serde_json::Value::Object(map)).unwrap_or_else(|_| raw.to_string())
}

async fn http_get(url: &str) -> Result<Vec<u8>> {
    if !url.starts_with("https://") {
        anyhow::bail!("refusing non-https url: {url}");
    }
    let bytes = reqwest::Client::builder()
        .use_rustls_tls()
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.url().scheme() != "https" {
                attempt.error("non-https redirect refused")
            } else if attempt.previous().len() >= 5 {
                attempt.error("too many redirects")
            } else {
                attempt.follow()
            }
        }))
        .build()?
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()?
        .bytes()
        .await?
        .to_vec();
    Ok(bytes)
}

fn extract_from_archive(
    body: &[u8],
    archive: Archive,
    binary_in_archive: Option<&str>,
) -> Result<Vec<u8>> {
    use std::io::{Cursor, Read};
    match archive {
        Archive::None => Ok(body.to_vec()),
        Archive::TarGz => {
            let target = binary_in_archive
                .ok_or_else(|| anyhow!("archive=tar.gz requires binary_in_archive"))?;
            let gz = flate2::read::GzDecoder::new(body);
            let mut tar = tar::Archive::new(gz);
            let mut entries = Vec::new();
            for e in tar.entries()? {
                let mut e = e?;
                let path = e.path()?.to_string_lossy().into_owned();
                let mut buf = Vec::new();
                e.read_to_end(&mut buf)?;
                entries.push((path, buf));
            }
            let names: Vec<String> = entries.iter().map(|(p, _)| p.clone()).collect();
            let idx = extract::select(&names, target)
                .map_err(|e| anyhow!("select {target}: {e:?}"))?;
            Ok(entries.swap_remove(idx).1)
        }
        Archive::Zip => {
            let target = binary_in_archive
                .ok_or_else(|| anyhow!("archive=zip requires binary_in_archive"))?;
            let mut zip = zip::ZipArchive::new(Cursor::new(body))?;
            let mut entries = Vec::new();
            for i in 0..zip.len() {
                let mut f = zip.by_index(i)?;
                if f.is_dir() {
                    continue;
                }
                let name = f.name().to_string();
                let mut buf = Vec::new();
                f.read_to_end(&mut buf)?;
                entries.push((name, buf));
            }
            let names: Vec<String> = entries.iter().map(|(p, _)| p.clone()).collect();
            let idx = extract::select(&names, target)
                .map_err(|e| anyhow!("select {target}: {e:?}"))?;
            Ok(entries.swap_remove(idx).1)
        }
        other => anyhow::bail!("archive format {:?} not yet wired", other),
    }
}

/// Result of `forest global add <org>/<name>[@ver]`.
#[derive(Debug)]
pub struct AddOutcome {
    pub resolved_version: String,
    pub shim_name: Option<String>,
    pub shape: ComponentShape,
}

#[derive(Debug)]
pub struct OrgSubscribeOutcome {
    pub organisation: String,
    pub emitted: Vec<EmittedCatalogEntry>,
    pub banned_seen: Vec<String>,
    pub shadowed: Vec<String>,
}

#[derive(Debug)]
pub struct SyncOutcome {
    pub created: Vec<String>,
    pub deleted: Vec<String>,
}

#[derive(Debug)]
pub struct UpdateOutcome {
    pub bumps: Vec<VersionBump>,
    pub sync: SyncOutcome,
}

#[derive(Debug)]
pub struct VersionBump {
    pub qualified: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug)]
pub struct EmittedCatalogEntry {
    pub upstream_name: String,
    pub shim_name: String,
    pub qualified: String,
    pub resolved_version: String,
}

impl GlobalService {
    /// `forest global add <org>/<name>[@ver]`.
    /// If `version` is None, resolve latest from the registry.
    pub async fn add_dependency(
        &self,
        organisation: &str,
        name: &str,
        version: Option<&str>,
        as_shim_name: Option<&str>,
    ) -> Result<AddOutcome> {
        // Resolve version via existing get_component_version (None -> latest).
        let component = match version {
            Some(v) => self
                .grpc
                .get_component_version(name, organisation, v)
                .await?
                .ok_or_else(|| anyhow!("not found: {organisation}/{name}@{v}"))?,
            None => self
                .grpc
                .get_component(name, organisation)
                .await?
                .ok_or_else(|| anyhow!("not found: {organisation}/{name}"))?,
        };
        let resolved_version = component.version.to_string();

        let manifest = self
            .fetch_manifest(organisation, name, &resolved_version)
            .await?;

        let mut cfg = self.load_user_config().await?;
        let key = format!("{organisation}/{name}");
        cfg.dependencies.insert(
            key.clone(),
            Dependency {
                version: resolved_version.clone(),
                shim_name: as_shim_name.map(str::to_string),
            },
        );
        self.save_user_config(&cfg).await?;

        // Create a shim if the manifest carries a tool facet.
        let shim_name_emitted = match (&manifest.tool, as_shim_name) {
            (Some(facet), Some(alias)) => {
                let qref = QualifiedRef::new(organisation, name);
                self.write_shim(alias, &qref).await?;
                let _ = facet; // silence unused
                Some(alias.to_string())
            }
            (Some(facet), None) => {
                let qref = QualifiedRef::new(organisation, name);
                self.write_shim(&facet.name, &qref).await?;
                Some(facet.name.clone())
            }
            (None, _) => None,
        };

        Ok(AddOutcome {
            resolved_version,
            shim_name: shim_name_emitted,
            shape: manifest.shape,
        })
    }

    /// `forest global add <org>` — subscribe to an org's tool catalogue.
    ///
    /// Calls `ListOrgTools`, applies `banned`/`pins`/`aliases`, writes
    /// `config.org_catalog.<org>` to `forest.cue`, emits shims (lazy
    /// install — binaries are NOT downloaded eagerly).
    pub async fn subscribe_to_org(
        &self,
        organisation: &str,
        banned: &[String],
        pins: &[(String, String)],
        aliases: &[(String, String)],
    ) -> Result<OrgSubscribeOutcome> {
        // 1. Fetch catalogue.
        let entries = self
            .grpc
            .list_org_tools(organisation)
            .await
            .with_context(|| {
                format!(
                    "fetching tool catalogue for organisation '{organisation}' \
                     (does the org exist, and are you a member? `forest organisation get \
                     --name {organisation}` to check)"
                )
            })?;
        if entries.is_empty() {
            anyhow::bail!(
                "organisation '{organisation}' has no tools published yet (or none have a tool facet — \
                 pure components are not installable as global tools)"
            );
        }

        let pin_map: std::collections::BTreeMap<String, String> = pins.iter().cloned().collect();
        let alias_map: std::collections::BTreeMap<String, String> =
            aliases.iter().cloned().collect();
        let banned_set: std::collections::BTreeSet<&str> =
            banned.iter().map(String::as_str).collect();

        // 2. Persist subscription to forest.cue.
        let mut cfg = self.load_user_config().await?;
        cfg.org_catalog.insert(
            organisation.to_string(),
            crate::global::user_config::OrgCatalog {
                enabled: true,
                banned: banned.to_vec(),
                pins: pin_map.clone(),
                aliases: alias_map.clone(),
            },
        );
        self.save_user_config(&cfg).await?;

        // 3. Resolve + emit shims.
        let mut emitted = Vec::new();
        let mut banned_seen = Vec::new();
        let mut shadowed = Vec::new();
        for entry in &entries {
            let tool = match &entry.tool {
                Some(t) => t,
                None => continue, // server should never send these but be defensive
            };
            let upstream_name = &tool.name;
            if banned_set.contains(upstream_name.as_str()) {
                banned_seen.push(upstream_name.clone());
                continue;
            }
            // Per-tool pin under `dependencies` wins over catalogue (§1a.2c
            // conflict rules).
            let per_tool_key = format!("{}/{}", entry.organisation, entry.name);
            if cfg.dependencies.contains_key(&per_tool_key) {
                shadowed.push(per_tool_key);
                continue;
            }

            let shim_name = alias_map
                .get(upstream_name)
                .cloned()
                .unwrap_or_else(|| upstream_name.clone());

            self.write_shim(
                &shim_name,
                &QualifiedRef::new(&entry.organisation, &entry.name),
            )
            .await?;
            emitted.push(EmittedCatalogEntry {
                upstream_name: upstream_name.clone(),
                shim_name,
                qualified: format!("{}/{}", entry.organisation, entry.name),
                resolved_version: pin_map
                    .get(upstream_name)
                    .cloned()
                    .unwrap_or_else(|| entry.latest_version.clone()),
            });
        }

        Ok(OrgSubscribeOutcome {
            organisation: organisation.to_string(),
            emitted,
            banned_seen,
            shadowed,
        })
    }

    /// `forest global update` — re-resolve per-tool pins and catalogue
    /// subscriptions against the registry, bump versions, sync shims.
    pub async fn update_all(&self) -> Result<UpdateOutcome> {
        let mut cfg = self.load_user_config().await?;
        let mut bumps = Vec::new();

        // Re-resolve each per-tool dep to the registry's current latest.
        let keys: Vec<String> = cfg.dependencies.keys().cloned().collect();
        for key in keys {
            let (org, name) = key
                .split_once('/')
                .ok_or_else(|| anyhow!("malformed dep key {key}"))?;
            let current = cfg
                .dependencies
                .get(&key)
                .map(|d| d.version.clone())
                .unwrap_or_default();
            let latest = match self.grpc.get_component(name, org).await {
                Ok(Some(c)) => c.version.to_string(),
                _ => continue,
            };
            if latest != current {
                if let Some(dep) = cfg.dependencies.get_mut(&key) {
                    dep.version = latest.clone();
                }
                bumps.push(VersionBump {
                    qualified: key.clone(),
                    from: current,
                    to: latest,
                });
            }
        }

        self.save_user_config(&cfg).await?;
        let sync = self.sync_shims().await?;
        Ok(UpdateOutcome { bumps, sync })
    }

    /// `forest global sync` — reconcile shim dir vs forest.cue.
    ///
    /// Build the full expected shim set from `config.dependencies` +
    /// `config.org_catalog` (with bans/aliases/pins applied), create any
    /// missing shims, delete any orphan shims whose body marker shows
    /// Forest authored them. Idempotent.
    pub async fn sync_shims(&self) -> Result<SyncOutcome> {
        let cfg = self.load_user_config().await?;

        // 1. Compute the expected (shim_name → qualified) map.
        let mut expected: std::collections::BTreeMap<String, QualifiedRef> =
            std::collections::BTreeMap::new();

        // 1a. Per-tool deps.
        for (key, dep) in &cfg.dependencies {
            let (org, name) = key
                .split_once('/')
                .ok_or_else(|| anyhow!("malformed dep key {key}"))?;
            let shim_name = match &dep.shim_name {
                Some(s) => s.clone(),
                None => {
                    // Need to look up the manifest to find the tool name.
                    // Fallback: use the component name. Shim creation will
                    // still write a deterministic body.
                    match self.fetch_manifest(org, name, &dep.version).await {
                        Ok(m) => m
                            .tool
                            .as_ref()
                            .map(|t| t.name.clone())
                            .unwrap_or_else(|| name.to_string()),
                        Err(_) => name.to_string(),
                    }
                }
            };
            expected.insert(shim_name, QualifiedRef::new(org, name));
        }

        // 1b. Org catalogue subscriptions.
        for (org, cat) in &cfg.org_catalog {
            if !cat.enabled {
                continue;
            }
            let entries = match self.grpc.list_org_tools(org).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("ListOrgTools({org}) failed: {e:#}; skipping catalogue");
                    continue;
                }
            };
            for entry in entries {
                let tool = match entry.tool {
                    Some(t) => t,
                    None => continue,
                };
                if cat.banned.iter().any(|b| b == &tool.name) {
                    continue;
                }
                // Shadowed by per-tool pin?
                let per_tool_key = format!("{}/{}", entry.organisation, entry.name);
                if cfg.dependencies.contains_key(&per_tool_key) {
                    continue;
                }
                let shim_name = cat
                    .aliases
                    .get(&tool.name)
                    .cloned()
                    .unwrap_or(tool.name);
                expected.insert(
                    shim_name,
                    QualifiedRef::new(&entry.organisation, &entry.name),
                );
            }
        }

        // 2. Read existing shim dir.
        let shims_dir = self.paths.shims_dir();
        ensure_dir(&shims_dir).await?;
        let mut present: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();
        let mut rd = tokio::fs::read_dir(&shims_dir).await?;
        while let Some(entry) = rd.next_entry().await? {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(body) = read_optional(&entry.path()).await? {
                present.insert(name, body);
            }
        }

        // 3. Compute diffs and apply.
        let mut created = Vec::new();
        let mut deleted = Vec::new();

        for (shim_name, qref) in &expected {
            let want_body = crate::global::shim::shim_script_for(qref);
            match present.get(shim_name) {
                Some(have) if *have == want_body => {} // up-to-date
                _ => {
                    self.write_shim(shim_name, qref).await?;
                    created.push(shim_name.clone());
                }
            }
        }

        for (shim_name, body) in &present {
            if expected.contains_key(shim_name) {
                continue;
            }
            // Orphan: delete only if Forest-authored (marker on line 2).
            let second = body.lines().nth(1).unwrap_or("");
            if second == crate::global::shim::SHIM_MARKER {
                self.delete_shim(shim_name).await?;
                deleted.push(shim_name.clone());
            }
        }

        Ok(SyncOutcome { created, deleted })
    }

    /// `forest global ban <org> <tool>`. Adds `tool` to the org-catalogue
    /// ban list and deletes the shim.
    pub async fn ban_tool(&self, organisation: &str, tool_name: &str) -> Result<()> {
        let mut cfg = self.load_user_config().await?;
        let cat = cfg
            .org_catalog
            .get_mut(organisation)
            .ok_or_else(|| anyhow!("not subscribed to org catalogue: {organisation}"))?;
        if !cat.banned.iter().any(|t| t == tool_name) {
            cat.banned.push(tool_name.to_string());
            cat.banned.sort();
        }
        // The shim filename equals tool_name unless an alias is set. Look up
        // alias first.
        let shim_to_delete = cat
            .aliases
            .get(tool_name)
            .cloned()
            .unwrap_or_else(|| tool_name.to_string());
        self.save_user_config(&cfg).await?;
        self.delete_shim(&shim_to_delete).await?;
        Ok(())
    }

    /// `forest global unban <org> <tool>`. Removes from ban list. Does NOT
    /// recreate the shim — call `forest global sync` for that (or it will
    /// be created on the next `forest global update`).
    pub async fn unban_tool(&self, organisation: &str, tool_name: &str) -> Result<()> {
        let mut cfg = self.load_user_config().await?;
        let cat = cfg
            .org_catalog
            .get_mut(organisation)
            .ok_or_else(|| anyhow!("not subscribed to org catalogue: {organisation}"))?;
        cat.banned.retain(|t| t != tool_name);
        self.save_user_config(&cfg).await?;
        Ok(())
    }

    /// `forest global pin <org>/<tool> <ver>` — set a per-tool pin inside a
    /// catalogue subscription.
    pub async fn pin_catalogue_tool(
        &self,
        organisation: &str,
        tool_name: &str,
        version: &str,
    ) -> Result<()> {
        let mut cfg = self.load_user_config().await?;
        let cat = cfg
            .org_catalog
            .get_mut(organisation)
            .ok_or_else(|| anyhow!("not subscribed to org catalogue: {organisation}"))?;
        cat.pins.insert(tool_name.to_string(), version.to_string());
        self.save_user_config(&cfg).await?;
        Ok(())
    }

    /// `forest global unpin <org>/<tool>` — drop a per-tool pin inside a
    /// catalogue subscription. The tool tracks `latest` again on next update.
    pub async fn unpin_catalogue_tool(
        &self,
        organisation: &str,
        tool_name: &str,
    ) -> Result<()> {
        let mut cfg = self.load_user_config().await?;
        let cat = cfg
            .org_catalog
            .get_mut(organisation)
            .ok_or_else(|| anyhow!("not subscribed to org catalogue: {organisation}"))?;
        cat.pins.remove(tool_name);
        self.save_user_config(&cfg).await?;
        Ok(())
    }

    /// `forest global remove <org>/<name>` — removes dep entry + shim.
    pub async fn remove_dependency(
        &self,
        organisation: &str,
        name: &str,
    ) -> Result<()> {
        let mut cfg = self.load_user_config().await?;
        let key = format!("{organisation}/{name}");
        let removed = cfg.dependencies.remove(&key);
        self.save_user_config(&cfg).await?;
        if let Some(dep) = removed {
            // Determine the shim name to delete: explicit alias OR the tool
            // facet's name from the registry.
            let shim_name = match dep.shim_name {
                Some(s) => Some(s),
                None => self
                    .fetch_manifest(organisation, name, &dep.version)
                    .await
                    .ok()
                    .and_then(|m| m.tool.map(|t: ToolFacet| t.name)),
            };
            if let Some(shim) = shim_name {
                self.delete_shim(&shim).await?;
            }
        }
        Ok(())
    }

    /// Walk the shims directory and resolve a bare name (Q7.a).
    /// Returns `(organisation, name)` from the shim body.
    pub async fn resolve_bare_name(
        &self,
        bare: &str,
    ) -> Result<QualifiedRef> {
        let shim = self.shim_path(bare);
        let body = read_optional(&shim)
            .await?
            .ok_or_else(|| anyhow!("tool '{bare}' is not installed"))?;
        parse_qualified_ref_from_shim(&body).ok_or_else(|| {
            anyhow!("shim {} is not a forest shim (no qualified ref in body)", shim.display())
        })
    }

    /// `forest global list` — full catalogue view.
    ///
    /// Enumerates every tool the user has subscribed to via per-tool pins
    /// AND via org-catalogue subscriptions (applying ban/alias/pin rules).
    /// Lazy installation is opaque to discovery — entries appear regardless
    /// of whether the binary has been fetched yet, with their `status`
    /// reporting `cached` or `missing`.
    pub async fn list(&self) -> Result<Vec<ListedTool>> {
        let cfg = self.load_user_config().await?;
        let lock = self.load_lockfile().await.unwrap_or_default();
        let host = platform::current();
        let mut out: std::collections::BTreeMap<String, ListedTool> =
            std::collections::BTreeMap::new();

        // 1. Per-tool pins.
        for (key, dep) in &cfg.dependencies {
            let (org, name) = key
                .split_once('/')
                .ok_or_else(|| anyhow!("malformed dep key {key}"))?;
            let shim_name = dep
                .shim_name
                .clone()
                .unwrap_or_else(|| name.to_string());
            let status =
                self.status_for(host, &lock, org, name, &dep.version).await?;
            out.insert(
                shim_name.clone(),
                ListedTool {
                    shim_name,
                    organisation: org.to_string(),
                    name: name.to_string(),
                    version: dep.version.clone(),
                    status,
                    source: ToolSource::Pin,
                },
            );
        }

        // 2. Org-catalogue subscriptions. Best-effort — if the registry is
        // unreachable, omit that org's entries with a warning rather than
        // erroring out (`list` is informational).
        for (org, cat) in &cfg.org_catalog {
            if !cat.enabled {
                continue;
            }
            let entries = match self.grpc.list_org_tools(org).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("ListOrgTools({org}) failed: {e:#}; omitting from list");
                    continue;
                }
            };
            for entry in entries {
                let tool = match &entry.tool {
                    Some(t) => t,
                    None => continue,
                };
                let banned = cat.banned.iter().any(|b| b == &tool.name);
                let per_tool_key = format!("{}/{}", entry.organisation, entry.name);
                let shadowed = cfg.dependencies.contains_key(&per_tool_key);
                let shim_name = cat
                    .aliases
                    .get(&tool.name)
                    .cloned()
                    .unwrap_or_else(|| tool.name.clone());
                let pinned_version =
                    cat.pins.get(&tool.name).cloned().unwrap_or(entry.latest_version);

                let source = if banned {
                    ToolSource::CatalogBanned { org: org.clone() }
                } else if shadowed {
                    ToolSource::CatalogShadowed { org: org.clone() }
                } else {
                    ToolSource::Catalog { org: org.clone() }
                };

                let status = if banned || shadowed {
                    // No shim emitted; never installed via this entry.
                    ToolStatus::Missing
                } else {
                    self.status_for(
                        host,
                        &lock,
                        &entry.organisation,
                        &entry.name,
                        &pinned_version,
                    )
                    .await?
                };

                // Per-tool pin wins; don't overwrite a Pin entry with a
                // Catalog entry of the same shim name.
                if matches!(out.get(&shim_name).map(|t| &t.source), Some(ToolSource::Pin)) {
                    continue;
                }
                out.insert(
                    shim_name.clone(),
                    ListedTool {
                        shim_name,
                        organisation: entry.organisation,
                        name: entry.name,
                        version: pinned_version,
                        status,
                        source,
                    },
                );
            }
        }

        let mut v: Vec<_> = out.into_values().collect();
        v.sort_by(|a, b| a.shim_name.cmp(&b.shim_name));
        Ok(v)
    }

    async fn status_for(
        &self,
        host: Option<PlatformKey>,
        lock: &GlobalLockFile,
        org: &str,
        name: &str,
        version: &str,
    ) -> Result<ToolStatus> {
        let Some(p) = host else {
            return Ok(ToolStatus::Missing);
        };
        match lock.get(
            org,
            name,
            version,
            platform::os_str(p.os),
            platform::arch_str(p.arch),
        ) {
            Some(sha) => {
                if self.cache.read_by_sha(sha).await?.is_some() {
                    Ok(ToolStatus::Cached)
                } else {
                    Ok(ToolStatus::Missing)
                }
            }
            None => Ok(ToolStatus::Missing),
        }
    }
}

#[derive(Debug)]
pub struct ListedTool {
    pub shim_name: String,
    pub organisation: String,
    pub name: String,
    pub version: String,
    pub status: ToolStatus,
    pub source: ToolSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolSource {
    /// Explicit per-tool pin in `config.dependencies`.
    Pin,
    /// Reachable via an `org_catalog` subscription, currently emitting a shim.
    Catalog { org: String },
    /// In the catalogue but banned by `config.org_catalog.<org>.banned`.
    CatalogBanned { org: String },
    /// In the catalogue but shadowed by an explicit per-tool pin.
    CatalogShadowed { org: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Cached,
    Missing,
}

/// Extract the `<org>/<name>` reference from a shim body. Returns None if
/// the file isn't a forest shim.
///
/// Looks for the `global run <ref>` token sequence anywhere in any line —
/// tolerates `exec forest`, `exec /abs/path/forest`, `exec env FOO=bar forest`,
/// or wrapper scripts that surround the canonical invocation. The forest-
/// authored shim canonically uses `exec forest global run <org>/<name> -- "$@"`,
/// but the parser stays compatible with any caller that preserves that
/// substring.
pub fn parse_qualified_ref_from_shim(body: &str) -> Option<QualifiedRef> {
    for line in body.lines() {
        // Find `global run` as a token (preceded and followed by a space).
        let Some(idx) = line.find(" global run ") else {
            continue;
        };
        let after = &line[idx + " global run ".len()..];
        let token = after.split_whitespace().next()?;
        // Strip any optional `@version` tail.
        let token = token.split('@').next()?;
        let (org, name) = token.split_once('/')?;
        if org.is_empty() || name.is_empty() {
            return None;
        }
        return Some(QualifiedRef::new(org, name));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::global::user_config::OrgCatalog;

    #[test]
    fn parses_qualified_ref_from_canonical_shim_body() {
        let body = "#!/bin/sh\n# forest shim — do not edit\nexec forest global run cuteorg/ripgrep -- \"$@\"\n";
        let q = parse_qualified_ref_from_shim(body).unwrap();
        assert_eq!(q, QualifiedRef::new("cuteorg", "ripgrep"));
    }

    #[test]
    fn parses_qualified_ref_when_forest_is_an_absolute_path() {
        // Tolerates `exec /usr/local/bin/forest global run ...` and similar.
        let body = "#!/bin/sh\n# forest shim — do not edit\nexec /usr/local/bin/forest global run cuteorg/ripgrep -- \"$@\"\n";
        let q = parse_qualified_ref_from_shim(body).unwrap();
        assert_eq!(q, QualifiedRef::new("cuteorg", "ripgrep"));
    }

    #[test]
    fn parses_qualified_ref_with_version_suffix() {
        let body = "exec forest global run cuteorg/ripgrep@14.1.1 -- \"$@\"\n";
        let q = parse_qualified_ref_from_shim(body).unwrap();
        assert_eq!(q, QualifiedRef::new("cuteorg", "ripgrep"));
    }

    #[test]
    fn returns_none_for_non_shim_file() {
        let body = "#!/bin/sh\necho hello\n";
        assert!(parse_qualified_ref_from_shim(body).is_none());
    }

    #[test]
    fn render_user_config_round_trips_via_parser() {
        // The CUE we emit, when fed through `cue eval --out=json` and then
        // `parse_user_config`, must reconstruct the same UserConfig.
        // We can't run cue here, but we can at least check the produced
        // text contains the expected keys.
        let mut cfg = UserConfig::default();
        cfg.dependencies.insert(
            "cuteorg/ripgrep".into(),
            Dependency {
                version: "14.1.1".into(),
                shim_name: Some("rg".into()),
            },
        );
        cfg.org_catalog.insert(
            "cuteorg".into(),
            OrgCatalog {
                enabled: true,
                banned: vec!["forest-greet".into()],
                pins: Default::default(),
                aliases: Default::default(),
            },
        );
        let text = render_user_config(&cfg);
        assert!(text.contains("\"cuteorg/ripgrep\""));
        assert!(text.contains("version: \"14.1.1\""));
        assert!(text.contains("shim_name: \"rg\""));
        assert!(text.contains("org_catalog"));
        assert!(text.contains("banned: [\"forest-greet\"]"));
    }

    #[test]
    fn ensure_kind_field_adds_binary_for_legacy_manifest() {
        let legacy = r#"{"protocol_version": "1.0", "platforms": {}}"#;
        let patched = ensure_kind_field(legacy);
        let v: serde_json::Value = serde_json::from_str(&patched).unwrap();
        assert_eq!(v["kind"], "binary");
    }

    #[test]
    fn ensure_kind_field_preserves_existing_kind() {
        let modern = r#"{"kind": "external", "tool": {"name": "rg"}}"#;
        let patched = ensure_kind_field(modern);
        let v: serde_json::Value = serde_json::from_str(&patched).unwrap();
        assert_eq!(v["kind"], "external");
    }
}
