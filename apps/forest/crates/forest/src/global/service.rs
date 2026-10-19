use std::path::{Path, PathBuf};

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

pub struct GlobalService {
    pub paths: GlobalPaths,
    pub cue: CueEvaluator,
    pub cache: BinaryCache,
    pub grpc: GrpcClient,
    pub max_in_flight: usize,
}

impl GlobalService {
    pub fn from_state(state: &State) -> Result<Self> {
        let paths = GlobalPaths::from_env()?;
        Ok(Self {
            cache: BinaryCache::new(paths.clone()),
            cue: CueEvaluator::new(),
            grpc: state.grpc_client(),
            max_in_flight: state.config.max_downloads_in_flight(),
            paths,
        })
    }

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
        let cfg = parse_user_config(&json).map_err(|e| anyhow!("parsing forest.cue: {e:?}"))?;
        Ok(cfg)
    }

    pub async fn save_user_config(&self, cfg: &UserConfig) -> Result<()> {
        ensure_dir(self.paths.config_dir()).await?;

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
        let lock =
            GlobalLockFile::parse(&text).map_err(|e| anyhow!("parsing global lockfile: {e:?}"))?;
        Ok(lock)
    }

    pub async fn save_lockfile(&self, lock: &GlobalLockFile) -> Result<()> {
        ensure_dir(self.paths.state_dir()).await?;
        atomic_write(&self.paths.lockfile(), lock.serialize().as_bytes()).await?;
        Ok(())
    }

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
            .with_context(|| format!("fetching manifest for {organisation}/{name}@{version}"))?;
        let raw = ensure_kind_field(&raw);
        let manifest = manifest::parse(&raw)
            .map_err(|e| anyhow!("parsing manifest for {organisation}/{name}@{version}: {e:?}"))?;
        Ok(manifest)
    }

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

    pub async fn resolve_to_cached_path(
        &self,
        qref: &QualifiedRef,
        version: &str,
    ) -> Result<PathBuf> {
        let host = platform::current().ok_or_else(|| anyhow!("unsupported host platform"))?;

        if let Some(p) = self.cached_path_if_present(qref, version).await? {
            return Ok(p);
        }
        let lockfile = self.load_lockfile().await.unwrap_or_default();

        let manifest = self
            .fetch_manifest(&qref.organisation, &qref.name, version)
            .await?;

        if let Err(e) = self
            .write_tool_include_env(qref, version, &manifest.include.env)
            .await
        {
            tracing::debug!(
                tool = %format!("{}/{}", qref.organisation, qref.name),
                version,
                "failed to cache include env (ignored): {e:#}"
            );
        }

        if let Err(e) = self
            .write_tool_include_shell(qref, version, &manifest.include.shell.init)
            .await
        {
            tracing::debug!(
                tool = %format!("{}/{}", qref.organisation, qref.name),
                version,
                "failed to cache include shell (ignored): {e:#}"
            );
        }

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
                    .map(|p| format!("{}/{}", platform::os_str(p.os), platform::arch_str(p.arch)))
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

        let cached_path = if let Some(p) = self.cache.read_by_sha(&expected_sha, &qref.name).await?
        {
            p
        } else {
            match fetch {
                FetchPlan::Registry => {
                    let streamed = self
                        .grpc
                        .download_component_binary_to_file(
                            &crate::grpc::BinaryRef {
                                organisation: &qref.organisation,
                                name: &qref.name,
                                version,
                                os: platform::os_str(host.os),
                                arch: platform::arch_str(host.arch),
                            },
                            &self.paths.binary_cache_dir(),
                            Some(&expected_sha),
                            manifest
                                .platforms
                                .get(&forest_manifest::PlatformKey {
                                    os: host.os,
                                    arch: host.arch,
                                })
                                .and_then(|p| p.size),
                            Some(&format!("Downloading {}/{}", qref.organisation, qref.name)),
                        )
                        .await
                        .with_context(|| {
                            format!(
                                "downloading {}/{}@{}",
                                qref.organisation, qref.name, version
                            )
                        })?;
                    self.cache
                        .finalize_streamed(
                            &streamed.temp_path,
                            &streamed.sha256_hex,
                            &expected_sha,
                            &qref.name,
                        )
                        .await?
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
                    let bytes = extract_from_archive(&body, archive, binary_in_archive.as_deref())?;
                    self.cache
                        .finalize(&bytes, &expected_sha, &qref.name)
                        .await?
                }
            }
        };

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

    pub async fn cached_path_if_present(
        &self,
        qref: &QualifiedRef,
        version: &str,
    ) -> Result<Option<PathBuf>> {
        let Some(host) = platform::current() else {
            return Ok(None);
        };
        let lockfile = self.load_lockfile().await.unwrap_or_default();
        let Some(pinned_sha) = lockfile.get(
            &qref.organisation,
            &qref.name,
            version,
            platform::os_str(host.os),
            platform::arch_str(host.arch),
        ) else {
            return Ok(None);
        };
        self.cache.read_by_sha(pinned_sha, &qref.name).await
    }

    pub async fn warm_tools(
        &self,
        only: &[String],
        mut on_event: impl FnMut(WarmEvent<'_>),
    ) -> Result<WarmOutcome> {
        let listed = self.list().await?;

        let mut outcome = WarmOutcome::default();
        let selected: Vec<&ListedTool> = listed
            .iter()
            .filter(|t| tool_is_installable(t))
            .filter(|t| only.is_empty() || matches_selector(t, only))
            .collect();

        for want in only {
            if !selected.iter().any(|t| selector_matches(t, want)) {
                outcome.unknown.push(want.clone());
                on_event(WarmEvent::Unknown(want));
            }
        }

        for tool in &selected {
            let qref = QualifiedRef::new(&tool.organisation, &tool.name);

            let binary = if matches!(tool.status, ToolStatus::Cached) {
                outcome.already_warm += 1;
                on_event(WarmEvent::AlreadyWarm(tool));
                self.cached_path_if_present(&qref, &tool.version)
                    .await
                    .unwrap_or(None)
            } else {
                on_event(WarmEvent::Fetching(tool));
                match self.resolve_to_cached_path(&qref, &tool.version).await {
                    Ok(p) => {
                        outcome.fetched.push(tool.shim_name.clone());
                        on_event(WarmEvent::Fetched(tool));
                        Some(p)
                    }
                    Err(e) => {
                        outcome.failed.push(tool.shim_name.clone());
                        on_event(WarmEvent::Failed(tool, &e));
                        None
                    }
                }
            };

            if let Some(binary) = binary {
                match self
                    .capture_shell_snippets(&qref, &tool.version, &binary)
                    .await
                {
                    Ok(shells) if !shells.is_empty() => {
                        outcome.shell_snippets += shells.len();
                        on_event(WarmEvent::CapturedShell(tool, &shells));
                    }
                    Ok(_) => {}
                    Err(e) => tracing::debug!(
                        tool = %tool.shim_name,
                        "shell-snippet capture skipped: {e:#}"
                    ),
                }
            }
        }

        let all: Vec<ListedTool> = listed.into_iter().filter(tool_is_installable).collect();
        if let Err(e) = self.rebuild_shell_aggregates(&all).await {
            tracing::debug!("rebuilding shell aggregates failed (ignored): {e:#}");
        }

        Ok(outcome)
    }

    pub async fn migrate_binary_store(&self) -> Result<usize> {
        let lock = self.load_lockfile().await.unwrap_or_default();
        let mut migrated = 0usize;
        for entry in lock.iter() {
            match self.cache.read_by_sha(&entry.sha256, &entry.name).await {
                Ok(Some(_)) => migrated += 1,
                Ok(None) => {}
                Err(e) => tracing::debug!(
                    tool = %format!("{}/{}", entry.organisation, entry.name),
                    "skipping store migration for this entry: {e:#}"
                ),
            }
        }
        Ok(migrated)
    }

    pub async fn write_tool_include_env(
        &self,
        qref: &QualifiedRef,
        version: &str,
        env: &std::collections::BTreeMap<String, String>,
    ) -> Result<()> {
        write_include_env(&self.paths, qref, version, env).await
    }

    pub async fn load_tool_include_env(
        &self,
        qref: &QualifiedRef,
        version: &str,
    ) -> Result<std::collections::BTreeMap<String, String>> {
        read_include_env(&self.paths, qref, version).await
    }

    pub async fn write_tool_include_shell(
        &self,
        qref: &QualifiedRef,
        version: &str,
        init: &std::collections::BTreeMap<String, Vec<String>>,
    ) -> Result<()> {
        write_include_shell(&self.paths, qref, version, init).await
    }

    pub async fn load_tool_include_shell(
        &self,
        qref: &QualifiedRef,
        version: &str,
    ) -> Result<Option<std::collections::BTreeMap<String, Vec<String>>>> {
        read_include_shell(&self.paths, qref, version).await
    }

    pub async fn capture_shell_snippets(
        &self,
        qref: &QualifiedRef,
        version: &str,
        binary: &std::path::Path,
    ) -> Result<Vec<String>> {
        let declared = match self.load_tool_include_shell(qref, version).await? {
            Some(d) => d,
            None => {
                let init = match self
                    .fetch_manifest(&qref.organisation, &qref.name, version)
                    .await
                {
                    Ok(m) => m.include.shell.init,
                    Err(e) => {
                        tracing::debug!(
                            tool = %format!("{}/{}", qref.organisation, qref.name),
                            version,
                            "shell declaration backfill skipped: {e:#}"
                        );
                        return Ok(Vec::new());
                    }
                };
                self.write_tool_include_shell(qref, version, &init).await?;
                init
            }
        };
        let mut captured = Vec::new();

        for (shell, argv) in &declared {
            let out_path =
                self.paths
                    .tool_shell_snippet(&qref.organisation, &qref.name, version, shell);
            if read_optional(&out_path).await?.is_some() {
                continue;
            }
            match self
                .run_for_shell_snippet(qref, version, binary, argv)
                .await
            {
                Ok(script) => {
                    ensure_dir(out_path.parent().expect("snippet path has a parent")).await?;
                    atomic_write(&out_path, script.as_bytes()).await?;
                    captured.push(shell.clone());
                }
                Err(e) => tracing::debug!(
                    tool = %format!("{}/{}", qref.organisation, qref.name),
                    version,
                    shell = %shell,
                    "shell-snippet capture failed (ignored): {e:#}"
                ),
            }
        }

        Ok(captured)
    }

    async fn run_for_shell_snippet(
        &self,
        qref: &QualifiedRef,
        version: &str,
        binary: &std::path::Path,
        argv: &[String],
    ) -> Result<String> {
        const MAX_SNIPPET_BYTES: usize = 512 * 1024;
        const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

        let component_env = self
            .load_tool_include_env(qref, version)
            .await
            .unwrap_or_default();

        let mut cmd = tokio::process::Command::new(binary);
        cmd.args(argv)
            .envs(&component_env)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        cmd.arg0(&qref.name);
        cmd.kill_on_drop(true);

        let output = tokio::time::timeout(TIMEOUT, cmd.output())
            .await
            .map_err(|_| anyhow!("timed out after {}s", TIMEOUT.as_secs()))?
            .with_context(|| format!("running {} {}", binary.display(), argv.join(" ")))?;

        if !output.status.success() {
            anyhow::bail!(
                "{} {} exited with {}",
                qref.name,
                argv.join(" "),
                output.status
            );
        }
        if output.stdout.len() > MAX_SNIPPET_BYTES {
            anyhow::bail!(
                "shell snippet is {} bytes, over the {MAX_SNIPPET_BYTES}-byte limit",
                output.stdout.len()
            );
        }
        String::from_utf8(output.stdout).context("shell snippet is not valid UTF-8")
    }

    pub async fn rebuild_shell_aggregates(&self, tools: &[ListedTool]) -> Result<()> {
        ensure_dir(&self.paths.shell_aggregate_dir()).await?;

        for shell in forest_manifest::SUPPORTED_SHELLS {
            let mut out = String::new();
            out.push_str(&format!(
                "# forest shell aggregate ({shell}) — generated, do not edit.\n\
                 # Rebuilt by `forest global warm` / `sync` / `update` from each\n\
                 # component's declared `include.shell.init.{shell}`.\n"
            ));

            for tool in tools {
                let Some((used_version, script)) =
                    self.newest_captured_snippet(tool, shell).await?
                else {
                    continue;
                };
                out.push_str(&format!(
                    "\n# ── {} ({}/{}@{}) ──\n",
                    tool.shim_name, tool.organisation, tool.name, used_version
                ));
                if used_version != tool.version {
                    out.push_str(&format!(
                        "# (captured at {used_version}; {} is installed — the next \
                         `forest global warm` refreshes this)\n",
                        tool.version
                    ));
                }
                out.push_str(&script);
                if !script.ends_with('\n') {
                    out.push('\n');
                }
            }

            atomic_write(&self.paths.shell_aggregate(shell), out.as_bytes()).await?;
        }
        Ok(())
    }

    async fn newest_captured_snippet(
        &self,
        tool: &ListedTool,
        shell: &str,
    ) -> Result<Option<(String, String)>> {
        newest_captured_snippet(
            &self.paths,
            &tool.organisation,
            &tool.name,
            &tool.version,
            shell,
        )
        .await
    }
}

async fn newest_captured_snippet(
    paths: &GlobalPaths,
    org: &str,
    name: &str,
    version: &str,
    shell: &str,
) -> Result<Option<(String, String)>> {
    if let Some(script) =
        read_optional(&paths.tool_shell_snippet(org, name, version, shell)).await?
    {
        return Ok(Some((version.to_string(), script)));
    }

    let Some(dir) = paths
        .tool_include_dir(org, name, version)
        .parent()
        .map(Path::to_path_buf)
    else {
        return Ok(None);
    };
    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(e) => e,
        Err(_) => return Ok(None),
    };
    let mut candidates: Vec<String> = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        if let Some(n) = entry.file_name().to_str() {
            candidates.push(n.to_string());
        }
    }
    candidates.sort_by(
        |a, b| match (semver::Version::parse(a), semver::Version::parse(b)) {
            (Ok(x), Ok(y)) => x.cmp(&y),
            _ => a.cmp(b),
        },
    );

    for candidate in candidates.into_iter().rev() {
        if let Some(script) =
            read_optional(&paths.tool_shell_snippet(org, name, &candidate, shell)).await?
        {
            tracing::debug!(
                tool = %format!("{org}/{name}"),
                installed = %version,
                using = %candidate,
                "no snippet for the installed version; serving the newest captured one"
            );
            return Ok(Some((candidate, script)));
        }
    }
    Ok(None)
}

async fn write_include_env(
    paths: &GlobalPaths,
    qref: &QualifiedRef,
    version: &str,
    env: &std::collections::BTreeMap<String, String>,
) -> Result<()> {
    let file = paths.tool_include_env_file(&qref.organisation, &qref.name, version);
    if env.is_empty() {
        remove_if_present(&file).await?;
        return Ok(());
    }
    ensure_dir(&paths.tool_include_dir(&qref.organisation, &qref.name, version)).await?;
    let json = serde_json::to_vec(env).context("serialise include env")?;
    atomic_write(&file, &json).await?;
    Ok(())
}

async fn write_include_shell(
    paths: &GlobalPaths,
    qref: &QualifiedRef,
    version: &str,
    init: &std::collections::BTreeMap<String, Vec<String>>,
) -> Result<()> {
    let file = paths.tool_include_shell_file(&qref.organisation, &qref.name, version);
    ensure_dir(&paths.tool_include_dir(&qref.organisation, &qref.name, version)).await?;
    let json = serde_json::to_vec(init).context("serialise include shell")?;
    atomic_write(&file, &json).await?;
    Ok(())
}

async fn read_include_shell(
    paths: &GlobalPaths,
    qref: &QualifiedRef,
    version: &str,
) -> Result<Option<std::collections::BTreeMap<String, Vec<String>>>> {
    let file = paths.tool_include_shell_file(&qref.organisation, &qref.name, version);
    match read_optional(&file).await? {
        None => Ok(None),
        Some(s) => serde_json::from_str(&s)
            .map(Some)
            .context("parse cached include shell"),
    }
}

async fn read_include_env(
    paths: &GlobalPaths,
    qref: &QualifiedRef,
    version: &str,
) -> Result<std::collections::BTreeMap<String, String>> {
    let file = paths.tool_include_env_file(&qref.organisation, &qref.name, version);
    match read_optional(&file).await? {
        None => Ok(std::collections::BTreeMap::new()),
        Some(s) => serde_json::from_str(&s).context("parse cached include env"),
    }
}

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
            out.push_str(&format!("\t\t\tversion: {}\n", cue_string(&dep.version)));
            if dep.pinned {
                out.push_str("\t\t\tpinned: true\n");
            }
            if let Some(shim) = &dep.shim_name {
                out.push_str(&format!("\t\t\tshim_name: {}\n", cue_string(shim)));
            }
            if !dep.env.is_empty() {
                out.push_str("\t\t\tenv: {\n");
                for (ek, ev) in &dep.env {
                    out.push_str(&format!("\t\t\t\t{}: {}\n", cue_string(ek), cue_string(ev)));
                }
                out.push_str("\t\t\t}\n");
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
                    out.push_str(&format!("\t\t\t\t{}: {}\n", cue_string(k), cue_string(v)));
                }
                out.push_str("\t\t\t}\n");
            }
            if !cat.aliases.is_empty() {
                out.push_str("\t\t\taliases: {\n");
                for (k, v) in &cat.aliases {
                    out.push_str(&format!("\t\t\t\t{}: {}\n", cue_string(k), cue_string(v)));
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
            let idx =
                extract::select(&names, target).map_err(|e| anyhow!("select {target}: {e:?}"))?;
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
            let idx =
                extract::select(&names, target).map_err(|e| anyhow!("select {target}: {e:?}"))?;
            Ok(entries.swap_remove(idx).1)
        }
        other => anyhow::bail!("archive format {:?} not yet wired", other),
    }
}

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
    pub held: usize,
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
    pub async fn add_dependency(
        &self,
        organisation: &str,
        name: &str,
        version: Option<&str>,
        as_shim_name: Option<&str>,
    ) -> Result<AddOutcome> {
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
                pinned: version.is_some(),
                shim_name: as_shim_name.map(str::to_string),
                env: Default::default(),
            },
        );
        self.save_user_config(&cfg).await?;

        let shim_name_emitted = match (&manifest.tool, as_shim_name) {
            (Some(facet), Some(alias)) => {
                let qref = QualifiedRef::new(organisation, name);
                self.write_shim(alias, &qref).await?;
                let _ = facet;
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

    pub async fn subscribe_to_org(
        &self,
        organisation: &str,
        banned: &[String],
        pins: &[(String, String)],
        aliases: &[(String, String)],
    ) -> Result<OrgSubscribeOutcome> {
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

        let mut emitted = Vec::new();
        let mut banned_seen = Vec::new();
        let mut shadowed = Vec::new();
        for entry in &entries {
            let tool = match &entry.tool {
                Some(t) => t,
                None => continue,
            };
            let upstream_name = &tool.name;
            if banned_set.contains(upstream_name.as_str()) {
                banned_seen.push(upstream_name.clone());
                continue;
            }
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

    pub async fn update_all(&self) -> Result<UpdateOutcome> {
        let mut cfg = self.load_user_config().await?;
        let mut bumps = Vec::new();
        let mut held = 0usize;

        let mut floating = Vec::new();
        for (key, dep) in &cfg.dependencies {
            let (org, name) = key
                .split_once('/')
                .ok_or_else(|| anyhow!("malformed dep key {key}"))?;
            if dep.pinned {
                held += 1;
                continue;
            }
            floating.push((
                key.clone(),
                org.to_string(),
                name.to_string(),
                dep.version.clone(),
            ));
        }

        let limiter = crate::download::Limiter::new(self.max_in_flight);
        let latests = crate::download::map_bounded(
            floating,
            std::sync::Arc::clone(&limiter),
            |(key, org, name, current), _lim| async move {
                let latest = match self.grpc.get_component(&name, &org).await {
                    Ok(Some(c)) => Some(c.version.to_string()),
                    _ => None,
                };
                Ok((key, current, latest))
            },
        )
        .await;

        for resolved in latests {
            let (key, current, latest) = resolved?;
            let Some(latest) = latest else { continue };
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
        if let Err(e) = self.migrate_binary_store().await {
            tracing::debug!("binary-store migration skipped: {e:#}");
        }
        Ok(UpdateOutcome { bumps, held, sync })
    }

    pub async fn sync_shims(&self) -> Result<SyncOutcome> {
        let cfg = self.load_user_config().await?;

        let mut expected: std::collections::BTreeMap<String, QualifiedRef> =
            std::collections::BTreeMap::new();

        let mut dep_keys = Vec::new();
        for (key, dep) in &cfg.dependencies {
            let (org, name) = key
                .split_once('/')
                .ok_or_else(|| anyhow!("malformed dep key {key}"))?;
            dep_keys.push((org.to_string(), name.to_string(), dep.clone()));
        }

        let limiter = crate::download::Limiter::new(self.max_in_flight);
        let resolved_shims = crate::download::map_bounded(
            dep_keys,
            std::sync::Arc::clone(&limiter),
            |(org, name, dep), _lim| async move {
                let shim_name = match &dep.shim_name {
                    Some(s) => s.clone(),
                    None => match self.fetch_manifest(&org, &name, &dep.version).await {
                        Ok(m) => m
                            .tool
                            .as_ref()
                            .map(|t| t.name.clone())
                            .unwrap_or_else(|| name.clone()),
                        Err(_) => name.clone(),
                    },
                };
                Ok((shim_name, QualifiedRef::new(&org, &name)))
            },
        )
        .await;
        for resolved in resolved_shims {
            let (shim_name, qref) = resolved?;
            expected.insert(shim_name, qref);
        }

        let orgs: Vec<String> = cfg
            .org_catalog
            .iter()
            .filter(|(_, cat)| cat.enabled)
            .map(|(org, _)| org.clone())
            .collect();
        let catalogues = crate::download::map_bounded(
            orgs,
            std::sync::Arc::clone(&limiter),
            |org, _lim| async move {
                let entries = self.grpc.list_org_tools(&org).await;
                Ok((org, entries))
            },
        )
        .await;

        for catalogue in catalogues {
            let (org, entries) = catalogue?;
            let cat = match cfg.org_catalog.get(&org) {
                Some(c) => c,
                None => continue,
            };
            let entries = match entries {
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
                let per_tool_key = format!("{}/{}", entry.organisation, entry.name);
                if cfg.dependencies.contains_key(&per_tool_key) {
                    continue;
                }
                let shim_name = cat.aliases.get(&tool.name).cloned().unwrap_or(tool.name);
                expected.insert(
                    shim_name,
                    QualifiedRef::new(&entry.organisation, &entry.name),
                );
            }
        }

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

        let mut created = Vec::new();
        let mut deleted = Vec::new();

        for (shim_name, qref) in &expected {
            let want_body = crate::global::shim::shim_script_for(qref);
            match present.get(shim_name) {
                Some(have) if *have == want_body => {}
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
            let second = body.lines().nth(1).unwrap_or("");
            if second == crate::global::shim::SHIM_MARKER {
                self.delete_shim(shim_name).await?;
                deleted.push(shim_name.clone());
            }
        }

        Ok(SyncOutcome { created, deleted })
    }

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
        let shim_to_delete = cat
            .aliases
            .get(tool_name)
            .cloned()
            .unwrap_or_else(|| tool_name.to_string());
        self.save_user_config(&cfg).await?;
        self.delete_shim(&shim_to_delete).await?;
        Ok(())
    }

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

    pub async fn unpin_catalogue_tool(&self, organisation: &str, tool_name: &str) -> Result<()> {
        let mut cfg = self.load_user_config().await?;
        let cat = cfg
            .org_catalog
            .get_mut(organisation)
            .ok_or_else(|| anyhow!("not subscribed to org catalogue: {organisation}"))?;
        cat.pins.remove(tool_name);
        self.save_user_config(&cfg).await?;
        Ok(())
    }

    pub async fn remove_dependency(&self, organisation: &str, name: &str) -> Result<()> {
        let mut cfg = self.load_user_config().await?;
        let key = format!("{organisation}/{name}");
        let removed = cfg.dependencies.remove(&key);
        self.save_user_config(&cfg).await?;
        if let Some(dep) = removed {
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

    pub async fn resolve_bare_name(&self, bare: &str) -> Result<QualifiedRef> {
        let shim = self.shim_path(bare);
        let body = read_optional(&shim)
            .await?
            .ok_or_else(|| anyhow!("tool '{bare}' is not installed"))?;
        parse_qualified_ref_from_shim(&body).ok_or_else(|| {
            anyhow!(
                "shim {} is not a forest shim (no qualified ref in body)",
                shim.display()
            )
        })
    }

    pub async fn list(&self) -> Result<Vec<ListedTool>> {
        let cfg = self.load_user_config().await?;
        let lock = self.load_lockfile().await.unwrap_or_default();
        let host = platform::current();
        let mut out: std::collections::BTreeMap<String, ListedTool> =
            std::collections::BTreeMap::new();

        for (key, dep) in &cfg.dependencies {
            let (org, name) = key
                .split_once('/')
                .ok_or_else(|| anyhow!("malformed dep key {key}"))?;
            let shim_name = dep.shim_name.clone().unwrap_or_else(|| name.to_string());
            let status = self
                .status_for(host, &lock, org, name, &dep.version)
                .await?;
            out.insert(
                shim_name.clone(),
                ListedTool {
                    shim_name,
                    organisation: org.to_string(),
                    name: name.to_string(),
                    version: dep.version.clone(),
                    status,
                    source: if dep.pinned {
                        ToolSource::Pin
                    } else {
                        ToolSource::Latest
                    },
                },
            );
        }

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
                let pinned_version = cat
                    .pins
                    .get(&tool.name)
                    .cloned()
                    .unwrap_or(entry.latest_version);

                let source = if banned {
                    ToolSource::CatalogBanned { org: org.clone() }
                } else if shadowed {
                    ToolSource::CatalogShadowed { org: org.clone() }
                } else {
                    ToolSource::Catalog { org: org.clone() }
                };

                let status = if banned || shadowed {
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

                if matches!(
                    out.get(&shim_name).map(|t| &t.source),
                    Some(ToolSource::Pin) | Some(ToolSource::Latest)
                ) {
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
                if self.cache.read_by_sha(sha, name).await?.is_some() {
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

pub enum WarmEvent<'a> {
    AlreadyWarm(&'a ListedTool),
    Fetching(&'a ListedTool),
    Fetched(&'a ListedTool),
    Failed(&'a ListedTool, &'a anyhow::Error),
    CapturedShell(&'a ListedTool, &'a [String]),
    Unknown(&'a str),
}

#[derive(Debug, Default)]
pub struct WarmOutcome {
    pub fetched: Vec<String>,
    pub failed: Vec<String>,
    pub already_warm: usize,
    pub shell_snippets: usize,
    pub unknown: Vec<String>,
}

fn tool_is_installable(t: &ListedTool) -> bool {
    !matches!(
        t.source,
        ToolSource::CatalogBanned { .. } | ToolSource::CatalogShadowed { .. }
    )
}

fn selector_matches(t: &ListedTool, selector: &str) -> bool {
    t.shim_name == selector
        || format!("{}/{}", t.organisation, t.name) == selector
        || t.name == selector
}

fn matches_selector(t: &ListedTool, selectors: &[String]) -> bool {
    selectors.iter().any(|s| selector_matches(t, s))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolSource {
    Pin,
    Latest,
    Catalog { org: String },
    CatalogBanned { org: String },
    CatalogShadowed { org: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Cached,
    Missing,
}

pub fn parse_qualified_ref_from_shim(body: &str) -> Option<QualifiedRef> {
    for line in body.lines() {
        let Some(idx) = line.find(" global run ") else {
            continue;
        };
        let after = &line[idx + " global run ".len()..];
        let token = after.split_whitespace().next()?;
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

    fn tmp_paths() -> (tempfile::TempDir, GlobalPaths) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let paths =
            GlobalPaths::with_roots(root.join("cfg"), root.join("state"), root.join("cache"));
        (dir, paths)
    }

    #[tokio::test]
    async fn include_env_round_trips_through_cache() {
        let (_d, paths) = tmp_paths();
        let qref = QualifiedRef::new("understory", "fungus");
        let mut env = std::collections::BTreeMap::new();
        env.insert("FUNGUS_SERVER".to_string(), "https://prod".to_string());
        write_include_env(&paths, &qref, "0.1.9", &env)
            .await
            .unwrap();
        let got = read_include_env(&paths, &qref, "0.1.9").await.unwrap();
        assert_eq!(got, env);
    }

    #[tokio::test]
    async fn missing_include_env_reads_empty() {
        let (_d, paths) = tmp_paths();
        let qref = QualifiedRef::new("understory", "fungus");
        let got = read_include_env(&paths, &qref, "9.9.9").await.unwrap();
        assert!(got.is_empty());
    }

    #[tokio::test]
    async fn empty_include_env_removes_stale_file() {
        let (_d, paths) = tmp_paths();
        let qref = QualifiedRef::new("understory", "fungus");
        let mut env = std::collections::BTreeMap::new();
        env.insert("A".to_string(), "1".to_string());
        write_include_env(&paths, &qref, "1.0.0", &env)
            .await
            .unwrap();
        write_include_env(&paths, &qref, "1.0.0", &Default::default())
            .await
            .unwrap();
        assert!(
            read_include_env(&paths, &qref, "1.0.0")
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            !paths
                .tool_include_env_file("understory", "fungus", "1.0.0")
                .exists()
        );
    }

    #[test]
    fn parses_qualified_ref_from_canonical_shim_body() {
        let body = "#!/bin/sh\n# forest shim — do not edit\nexec forest global run cuteorg/ripgrep -- \"$@\"\n";
        let q = parse_qualified_ref_from_shim(body).unwrap();
        assert_eq!(q, QualifiedRef::new("cuteorg", "ripgrep"));
    }

    #[test]
    fn parses_qualified_ref_from_a_shim_that_forwards_its_invoked_name() {
        let body = crate::global::shim::shim_script_for(&QualifiedRef::new("cuteorg", "ripgrep"));
        let q = parse_qualified_ref_from_shim(&body).unwrap();
        assert_eq!(q, QualifiedRef::new("cuteorg", "ripgrep"));
    }

    #[test]
    fn sync_rewrites_a_pre_data_510_shim_body() {
        let old = "#!/bin/sh\n# forest shim — do not edit\nexec forest global run cuteorg/ripgrep -- \"$@\"\n";
        let new = crate::global::shim::shim_script_for(&QualifiedRef::new("cuteorg", "ripgrep"));
        assert_ne!(old, new, "a stale shim must not compare equal");
        assert_eq!(
            parse_qualified_ref_from_shim(old).unwrap(),
            parse_qualified_ref_from_shim(&new).unwrap(),
            "both forms must resolve to the same tool"
        );
    }

    #[test]
    fn parses_qualified_ref_when_forest_is_an_absolute_path() {
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
        let mut cfg = UserConfig::default();
        cfg.dependencies.insert(
            "cuteorg/ripgrep".into(),
            Dependency {
                version: "14.1.1".into(),
                pinned: true,
                shim_name: Some("rg".into()),
                env: [("FUNGUS_SERVER".to_string(), "https://prod".to_string())]
                    .into_iter()
                    .collect(),
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
        assert!(text.contains("pinned: true"));
        assert!(text.contains("shim_name: \"rg\""));
        assert!(text.contains("env: {"));
        assert!(text.contains("\"FUNGUS_SERVER\": \"https://prod\""));
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

    fn listed(shim: &str, org: &str, name: &str, source: ToolSource) -> ListedTool {
        ListedTool {
            shim_name: shim.to_string(),
            organisation: org.to_string(),
            name: name.to_string(),
            version: "1.0.0".to_string(),
            status: ToolStatus::Missing,
            source,
        }
    }

    async fn put_snippet(paths: &GlobalPaths, version: &str, shell: &str, body: &str) {
        let p = paths.tool_shell_snippet("understory", "pgjump", version, shell);
        ensure_dir(p.parent().unwrap()).await.unwrap();
        atomic_write(&p, body.as_bytes()).await.unwrap();
    }

    #[tokio::test]
    async fn exact_version_snippet_wins() {
        let (_d, paths) = tmp_paths();
        put_snippet(&paths, "0.1.9", "zsh", "old\n").await;
        put_snippet(&paths, "0.1.10", "zsh", "new\n").await;
        let got = newest_captured_snippet(&paths, "understory", "pgjump", "0.1.10", "zsh")
            .await
            .unwrap();
        assert_eq!(got, Some(("0.1.10".to_string(), "new\n".to_string())));
    }

    #[tokio::test]
    async fn falls_back_to_newest_captured_when_installed_version_has_none() {
        let (_d, paths) = tmp_paths();
        put_snippet(&paths, "0.1.9", "zsh", "nine\n").await;
        let got = newest_captured_snippet(&paths, "understory", "pgjump", "0.1.10", "zsh")
            .await
            .unwrap();
        assert_eq!(got, Some(("0.1.9".to_string(), "nine\n".to_string())));
    }

    #[tokio::test]
    async fn fallback_picks_highest_by_semver_not_string_order() {
        let (_d, paths) = tmp_paths();
        put_snippet(&paths, "0.1.9", "zsh", "nine\n").await;
        put_snippet(&paths, "0.1.10", "zsh", "ten\n").await;
        let got = newest_captured_snippet(&paths, "understory", "pgjump", "0.2.0", "zsh")
            .await
            .unwrap();
        assert_eq!(got, Some(("0.1.10".to_string(), "ten\n".to_string())));
    }

    #[tokio::test]
    async fn fallback_is_per_shell() {
        let (_d, paths) = tmp_paths();
        put_snippet(&paths, "0.1.9", "zsh", "zshonly\n").await;
        assert!(
            newest_captured_snippet(&paths, "understory", "pgjump", "0.1.10", "bash")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn nothing_captured_yields_none() {
        let (_d, paths) = tmp_paths();
        assert!(
            newest_captured_snippet(&paths, "understory", "pgjump", "0.1.10", "zsh")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn absent_shell_declaration_reads_as_not_determined() {
        let (_d, paths) = tmp_paths();
        let qref = QualifiedRef::new("cuteorg", "ripgrep");
        assert_eq!(
            read_include_shell(&paths, &qref, "1.0.0").await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn empty_shell_declaration_is_recorded_not_deleted() {
        let (_d, paths) = tmp_paths();
        let qref = QualifiedRef::new("cuteorg", "ripgrep");
        let empty = std::collections::BTreeMap::new();
        write_include_shell(&paths, &qref, "1.0.0", &empty)
            .await
            .unwrap();
        assert_eq!(
            read_include_shell(&paths, &qref, "1.0.0").await.unwrap(),
            Some(empty)
        );
        assert!(
            paths
                .tool_include_shell_file("cuteorg", "ripgrep", "1.0.0")
                .exists(),
            "an empty declaration must still leave a file behind"
        );
    }

    #[tokio::test]
    async fn shell_declaration_round_trips() {
        let (_d, paths) = tmp_paths();
        let qref = QualifiedRef::new("cuteorg", "ripgrep");
        let mut init = std::collections::BTreeMap::new();
        init.insert(
            "zsh".to_string(),
            vec!["init".to_string(), "zsh".to_string()],
        );
        write_include_shell(&paths, &qref, "1.0.0", &init)
            .await
            .unwrap();
        assert_eq!(
            read_include_shell(&paths, &qref, "1.0.0").await.unwrap(),
            Some(init)
        );
    }

    #[tokio::test]
    async fn shell_declarations_are_per_version() {
        let (_d, paths) = tmp_paths();
        let qref = QualifiedRef::new("cuteorg", "ripgrep");
        let mut init = std::collections::BTreeMap::new();
        init.insert(
            "zsh".to_string(),
            vec!["init".to_string(), "zsh".to_string()],
        );
        write_include_shell(&paths, &qref, "1.0.0", &init)
            .await
            .unwrap();
        assert_eq!(
            read_include_shell(&paths, &qref, "1.0.1").await.unwrap(),
            None
        );
    }

    #[test]
    fn warm_selector_matches_the_name_you_type() {
        let t = listed("rg", "cuteorg", "ripgrep", ToolSource::Pin);
        assert!(selector_matches(&t, "rg"));
    }

    #[test]
    fn warm_selector_matches_the_qualified_ref_and_bare_component_name() {
        let t = listed("rg", "cuteorg", "ripgrep", ToolSource::Pin);
        assert!(selector_matches(&t, "cuteorg/ripgrep"));
        assert!(selector_matches(&t, "ripgrep"));
    }

    #[test]
    fn warm_selector_rejects_unrelated_names() {
        let t = listed("rg", "cuteorg", "ripgrep", ToolSource::Pin);
        assert!(!selector_matches(&t, "grep"));
        assert!(!selector_matches(&t, "otherorg/ripgrep"));
    }

    #[test]
    fn warm_skips_banned_and_shadowed_catalogue_entries() {
        let org = "cuteorg".to_string();
        assert!(!tool_is_installable(&listed(
            "a",
            "cuteorg",
            "a",
            ToolSource::CatalogBanned { org: org.clone() }
        )));
        assert!(!tool_is_installable(&listed(
            "b",
            "cuteorg",
            "b",
            ToolSource::CatalogShadowed { org: org.clone() }
        )));
        assert!(tool_is_installable(&listed(
            "c",
            "cuteorg",
            "c",
            ToolSource::Catalog { org }
        )));
        assert!(tool_is_installable(&listed(
            "d",
            "cuteorg",
            "d",
            ToolSource::Pin
        )));
        assert!(tool_is_installable(&listed(
            "e",
            "cuteorg",
            "e",
            ToolSource::Latest
        )));
    }

    #[test]
    fn empty_selector_list_is_not_used_as_a_filter() {
        let t = listed("rg", "cuteorg", "ripgrep", ToolSource::Pin);
        assert!(!matches_selector(&t, &[]));
    }

    #[test]
    fn matches_selector_accepts_any_of_the_given_names() {
        let t = listed("rg", "cuteorg", "ripgrep", ToolSource::Pin);
        assert!(matches_selector(
            &t,
            &["nope".to_string(), "rg".to_string()]
        ));
        assert!(!matches_selector(
            &t,
            &["nope".to_string(), "also-nope".to_string()]
        ));
    }
}
