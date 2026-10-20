use anyhow::Context;
use forest_grpc_interface::ProjectMetadata;
use sha2::{Digest, Sha256};

use crate::{
    contexts::ContextStore,
    grpc::{GrpcClient, GrpcClientState},
    services::component_binary,
    state::State,
    user_state::UserStateLoaderState,
};

fn print_publish_context(state: &State, owner: &str, component: &str) {
    let target = ContextStore::from_env().and_then(|store| {
        crate::contexts::resolve_server_target(
            state.config.forest_server.as_deref(),
            state.config.context.as_deref(),
            &store,
        )
    });
    match target {
        Ok(target) => {
            let line = publish_line(owner, component, &target);
            tracing::debug!("{line}");
            crate::ui::status(line);
        }
        Err(_) => crate::ui::status(format!("Publishing {owner}/{component}")),
    }
}

fn display_server(server: &str) -> std::borrow::Cow<'_, str> {
    if !server.contains(['@', '?', '#']) {
        return std::borrow::Cow::Borrowed(server);
    }
    let Ok(mut url) = reqwest::Url::parse(server) else {
        return std::borrow::Cow::Borrowed("<invalid server URL>");
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    std::borrow::Cow::Owned(url.into())
}

fn publish_line(owner: &str, component: &str, target: &crate::contexts::ServerTarget) -> String {
    let server = display_server(&target.server);
    if target.label == target.server {
        format!("Publishing {owner}/{component} to {server}")
    } else {
        format!(
            "Publishing {owner}/{component} to {} ({})",
            target.label, server
        )
    }
}

struct PublishSummary {
    owner: String,
    component: String,
    version: String,
    shape: &'static str,
    kind: &'static str,
    platform: String,
}

impl PublishSummary {
    fn print(&self) {
        tracing::debug!(
            "published {}/{}@{} as shape={} [{}] {}",
            self.owner,
            self.component,
            self.version,
            self.shape,
            self.kind,
            self.platform,
        );
        crate::ui::success(format!(
            "Published {}/{}@{}",
            self.owner, self.component, self.version
        ));
    }
}

fn derive_summary_shape(kind: &str, has_tool: bool, has_methods: bool) -> &'static str {
    match (kind, has_tool, has_methods) {
        ("binary", false, true) => "component",
        ("binary", true, false) => "tool_binary",
        ("binary", true, true) => "hybrid_component",
        ("external", true, _) => "tool_external",
        ("cue", _, _) => "library",
        ("deno", _, _) => "deno",
        _ => "component",
    }
}

fn single_platform_tool_warning(
    has_tool: bool,
    platforms: &[String],
    upload_type: Option<&str>,
) -> Option<String> {
    if !has_tool || platforms.len() != 1 {
        return None;
    }
    let only = &platforms[0];
    let remedy = if upload_type == Some("prebuilt") {
        "add the other platforms to `upload.prebuilt` in forest.cue"
    } else {
        "switch to `upload.type: \"prebuilt\"` and build each platform up front \
(a built component only ever publishes the host's platform)"
    };
    Some(format!(
        "publishing a tool for {only} only — `forest global add` will fail for every other \
platform; {remedy}"
    ))
}

fn publishable_binaries(
    current_dir: &std::path::Path,
    name: &str,
) -> anyhow::Result<Vec<(String, String, std::path::PathBuf)>> {
    let discovered = component_binary::discover_output_binaries(current_dir, name);
    if !discovered.is_empty() {
        return Ok(discovered);
    }

    anyhow::bail!(
        "no staged artifact in `.forest/component/output/` for `{name}` — run the build \
         (`forest run build`) before publishing.\n\
         \n\
         forest publishes only what the build stages there. It deliberately does not \
         pick up a binary found elsewhere in the working tree (a cargo `target/debug` \
         or `target/release` build), because doing so uploaded stale debug binaries to \
         the registry."
    )
}

#[cfg(test)]
mod publishable_binaries_tests {
    use super::*;

    fn workspace_with_decoy(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();

        let decoy_dir = tmp.path().join("target/debug");
        std::fs::create_dir_all(&decoy_dir).unwrap();
        std::fs::write(decoy_dir.join(name), b"stale debug build").unwrap();

        let component = tmp.path().join("components").join(name);
        std::fs::create_dir_all(&component).unwrap();
        (tmp, component)
    }

    fn stage(component: &std::path::Path, os: &str, arch: &str, name: &str) {
        let d = component
            .join(".forest/component/output")
            .join(os)
            .join(arch);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(name), b"release build").unwrap();
    }

    #[test]
    fn publishes_every_staged_platform_and_nothing_from_target() {
        let (_tmp, component) = workspace_with_decoy("mytool");
        stage(&component, "linux", "amd64", "mytool");
        stage(&component, "linux", "arm64", "mytool");
        stage(&component, "macos", "arm64", "mytool");

        let found = publishable_binaries(&component, "mytool").unwrap();

        let keys: Vec<String> = found
            .iter()
            .map(|(os, arch, _)| format!("{os}/{arch}"))
            .collect();
        assert_eq!(keys, vec!["linux/amd64", "linux/arm64", "macos/arm64"]);
        assert!(
            found
                .iter()
                .all(|(_, _, p)| !p.components().any(|c| c.as_os_str() == "target")),
            "a target/ path reached the publish set: {found:?}",
        );
    }

    #[test]
    fn empty_forest_dir_fails_loudly_instead_of_falling_back_to_target() {
        let (_tmp, component) = workspace_with_decoy("mytool");

        let err = publishable_binaries(&component, "mytool")
            .expect_err("an unpopulated .forest must not resolve to the target/ decoy");
        let msg = format!("{err}");

        assert!(
            msg.contains(".forest/component/output/"),
            "error should name the directory the build stages into: {msg}",
        );
        assert!(
            msg.contains("forest run build"),
            "error should tell the user how to fix it: {msg}",
        );
    }

    #[test]
    fn a_cached_blob_is_not_a_fallback_for_an_empty_forest_dir() {
        let (_tmp, component) = workspace_with_decoy("mytool");

        let meta_dir = component.join(".forest/component");
        std::fs::create_dir_all(&meta_dir).unwrap();
        let (os, arch) = component_binary::current_platform();
        std::fs::write(
            meta_dir.join("meta.json"),
            format!(
                r#"{{"platforms":{{"{os}_{arch}":{{"sha256":"{}","size":17}}}}}}"#,
                "0".repeat(64)
            ),
        )
        .unwrap();

        assert!(
            publishable_binaries(&component, "mytool").is_err(),
            "a cached blob must not stand in for a staged build artifact",
        );
    }

    #[test]
    fn a_partial_matrix_publishes_the_platforms_that_built() {
        let (_tmp, component) = workspace_with_decoy("mytool");
        stage(&component, "linux", "amd64", "mytool");

        let found = publishable_binaries(&component, "mytool").unwrap();
        assert_eq!(found.len(), 1);
        assert!(
            found[0]
                .2
                .ends_with(".forest/component/output/linux/amd64/mytool")
        );
    }
}

struct AbortOnDrop {
    client: Option<GrpcClient>,
    upload_context: String,
    reason: String,
}

impl AbortOnDrop {
    fn new(client: GrpcClient, upload_context: impl Into<String>) -> Self {
        Self {
            client: Some(client),
            upload_context: upload_context.into(),
            reason: "publish flow exited before commit".into(),
        }
    }

    fn disarm(mut self) {
        self.client = None;
    }
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        let Some(client) = self.client.take() else {
            return;
        };
        let upload_context = std::mem::take(&mut self.upload_context);
        let reason = std::mem::take(&mut self.reason);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                if let Err(e) = client
                    .abort_component_upload(&upload_context, &reason)
                    .await
                {
                    tracing::debug!(
                        upload_context = %upload_context,
                        "abort_component_upload failed (ignored): {e:#}"
                    );
                }
            });
        }
    }
}

#[cfg(test)]
mod abort_on_drop_tests {
    use super::AbortOnDrop;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    struct TestGuard {
        fired: Arc<AtomicBool>,
        armed: bool,
    }
    impl TestGuard {
        fn new(fired: Arc<AtomicBool>) -> Self {
            Self { fired, armed: true }
        }
        fn disarm(mut self) {
            self.armed = false;
        }
    }
    impl Drop for TestGuard {
        fn drop(&mut self) {
            if self.armed {
                self.fired.store(true, Ordering::SeqCst);
            }
        }
    }

    #[test]
    fn guard_fires_on_drop_when_armed() {
        let fired = Arc::new(AtomicBool::new(false));
        {
            let _g = TestGuard::new(fired.clone());
        }
        assert!(fired.load(Ordering::SeqCst), "guard should fire on drop");
    }

    #[test]
    fn guard_does_not_fire_after_disarm() {
        let fired = Arc::new(AtomicBool::new(false));
        {
            let g = TestGuard::new(fired.clone());
            g.disarm();
        }
        assert!(
            !fired.load(Ordering::SeqCst),
            "disarmed guard must not fire"
        );
    }

    #[test]
    fn guard_fires_on_panic() {
        let fired = Arc::new(AtomicBool::new(false));
        let fired_inner = fired.clone();
        let result = std::panic::catch_unwind(move || {
            let _g = TestGuard::new(fired_inner);
            panic!("simulated early-exit failure");
        });
        assert!(result.is_err());
        assert!(
            fired.load(Ordering::SeqCst),
            "panic during scope should still fire the guard"
        );
    }

    use super::{PublishSummary, collect_component_files, derive_summary_shape};

    #[test]
    fn shape_for_binary_tool() {
        assert_eq!(derive_summary_shape("binary", true, false), "tool_binary");
        assert_eq!(
            derive_summary_shape("binary", true, true),
            "hybrid_component"
        );
        assert_eq!(derive_summary_shape("binary", false, true), "component");
    }

    #[test]
    fn shape_for_external() {
        assert_eq!(
            derive_summary_shape("external", true, false),
            "tool_external"
        );
        assert_eq!(
            derive_summary_shape("external", true, true),
            "tool_external"
        );
    }

    async fn published_names(dir: &std::path::Path) -> Vec<String> {
        collect_component_files(dir, &[], &super::DeclaredPaths::default())
            .await
            .expect("collect")
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    }

    #[tokio::test]
    async fn publish_includes_the_cue_module_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        tokio::fs::write(dir.path().join("forest.cue"), "package x\n")
            .await
            .expect("write forest.cue");
        tokio::fs::create_dir_all(dir.path().join("cue.mod"))
            .await
            .expect("create cue.mod");
        tokio::fs::write(
            dir.path().join("cue.mod").join("module.cue"),
            "module: \"forest.sh/forest/deployment\"\n",
        )
        .await
        .expect("write module.cue");

        let names = published_names(dir.path()).await;

        assert!(
            names.iter().any(|n| n == "cue.mod/module.cue"),
            "module file must be published, got: {names:?}",
        );
        assert!(names.iter().any(|n| n == "forest.cue"), "got: {names:?}");
    }

    #[tokio::test]
    async fn publish_without_a_module_file_is_fine() {
        let dir = tempfile::tempdir().expect("tempdir");
        tokio::fs::write(dir.path().join("forest.cue"), "package x\n")
            .await
            .expect("write forest.cue");

        let names = published_names(dir.path()).await;
        assert_eq!(names, vec!["forest.cue".to_string()]);
    }

    #[tokio::test]
    async fn publish_includes_templates_and_scaffolding() {
        let dir = tempfile::tempdir().expect("tempdir");
        tokio::fs::write(dir.path().join("forest.component.cue"), "package x\n")
            .await
            .expect("write component cue");

        let tf = dir.path().join("templates/deployment/forest/terraform@1");
        tokio::fs::create_dir_all(&tf).await.expect("create tf dir");
        tokio::fs::write(tf.join("main.tf"), "resource \"null_resource\" \"x\" {}\n")
            .await
            .expect("write main.tf");

        let init = dir.path().join("init/service/files");
        tokio::fs::create_dir_all(&init).await.expect("create init");
        tokio::fs::write(init.join("README.md"), "scaffold\n")
            .await
            .expect("write scaffold");

        let names = published_names(dir.path()).await;

        assert!(
            names
                .iter()
                .any(|n| n == "templates/deployment/forest/terraform@1/main.tf"),
            "templates must publish, got: {names:?}",
        );
        assert!(
            names.iter().any(|n| n == "init/service/files/README.md"),
            "scaffolding must publish, got: {names:?}",
        );
    }

    #[tokio::test]
    async fn publish_stages_the_payload_under_dot_forest() {
        let dir = tempfile::tempdir().expect("tempdir");
        tokio::fs::write(dir.path().join("forest.cue"), "package x\n")
            .await
            .expect("write forest.cue");
        let tf = dir.path().join("templates/deployment/forest/terraform@1");
        tokio::fs::create_dir_all(&tf).await.expect("create tf dir");
        tokio::fs::write(tf.join("main.tf"), "resource {}\n")
            .await
            .expect("write main.tf");

        let files = super::stage_component_package(
            dir.path(),
            &[],
            &super::DeclaredPaths::default(),
            Vec::new(),
        )
        .await
        .expect("stage");
        assert_eq!(files.len(), 2, "got: {files:?}");

        let staged = dir.path().join(super::PACKAGE_DIR);
        assert!(staged.join("forest.cue").is_file(), "forest.cue not staged");
        assert!(
            staged
                .join("templates/deployment/forest/terraform@1/main.tf")
                .is_file(),
            "templates not staged",
        );
    }

    #[tokio::test]
    async fn the_staged_package_does_not_show_up_as_untracked_in_git() {
        let dir = tempfile::tempdir().expect("tempdir");
        tokio::fs::write(dir.path().join("forest.cue"), "package x\n")
            .await
            .expect("write forest.cue");
        tokio::fs::write(dir.path().join(".gitignore"), "dist/\n")
            .await
            .expect("write the component's own .gitignore");
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .output()
                .expect("run git")
        };
        assert!(git(&["init", "-q"]).status.success());

        let files = super::stage_component_package(
            dir.path(),
            &[],
            &super::DeclaredPaths::default(),
            Vec::new(),
        )
        .await
        .expect("stage");

        let own_ignore = files.iter().find(|(name, _)| name == ".gitignore");
        assert_eq!(
            own_ignore.map(|(_, c)| c.as_slice()),
            Some(b"dist/\n".as_slice()),
            "the component's own .gitignore must publish unchanged, got: {files:?}",
        );
        let staged_copy = std::fs::read(dir.path().join(super::PACKAGE_DIR).join(".gitignore"))
            .expect("staged .gitignore");
        assert_eq!(staged_copy, b"dist/\n");
        let status = git(&["status", "--porcelain", "--untracked-files=all"]);
        let untracked = String::from_utf8_lossy(&status.stdout);
        let mut lines: Vec<&str> = untracked.lines().collect();
        lines.sort();
        assert_eq!(
            lines,
            ["?? .gitignore", "?? forest.cue"],
            "got: {untracked}"
        );
    }

    #[tokio::test]
    async fn staging_clears_what_the_component_no_longer_has() {
        let dir = tempfile::tempdir().expect("tempdir");
        tokio::fs::write(dir.path().join("forest.cue"), "package x\n")
            .await
            .expect("write forest.cue");

        let stale = dir.path().join(super::PACKAGE_DIR).join("templates");
        tokio::fs::create_dir_all(&stale)
            .await
            .expect("create stale");
        tokio::fs::write(stale.join("gone.tf"), "removed\n")
            .await
            .expect("write stale");

        super::stage_component_package(
            dir.path(),
            &[],
            &super::DeclaredPaths::default(),
            Vec::new(),
        )
        .await
        .expect("stage");

        assert!(
            !stale.join("gone.tf").exists(),
            "a file the component no longer has was left in the package",
        );
    }

    #[tokio::test]
    async fn publish_excludes_staged_build_output() {
        let dir = tempfile::tempdir().expect("tempdir");
        tokio::fs::write(dir.path().join("forest.cue"), "package x\n")
            .await
            .expect("write forest.cue");

        let staged = dir.path().join(".forest/component/output/linux/amd64");
        tokio::fs::create_dir_all(&staged)
            .await
            .expect("create staged");
        tokio::fs::write(staged.join("service"), b"ELF".to_vec())
            .await
            .expect("write staged binary");

        let meta = dir.path().join(".forest/component");
        tokio::fs::write(meta.join("meta.json"), "{}\n")
            .await
            .expect("write meta.json");

        let names = published_names(dir.path()).await;

        assert!(
            !names
                .iter()
                .any(|n| n.starts_with(".forest/component/output/")),
            "staged binaries must not ship as files, got: {names:?}",
        );
        assert!(
            names.iter().any(|n| n == ".forest/component/meta.json"),
            "meta.json is consumer-read and must publish, got: {names:?}",
        );
    }

    #[test]
    fn shape_for_cue_and_deno() {
        assert_eq!(derive_summary_shape("cue", false, false), "library");
        assert_eq!(derive_summary_shape("deno", true, true), "deno");
    }

    #[test]
    fn summary_format_is_stable() {
        let s = PublishSummary {
            owner: "understory".into(),
            component: "canopy-data-cli".into(),
            version: "0.1.5".into(),
            shape: "tool_binary",
            kind: "binary",
            platform: "darwin_arm64".into(),
        };
        let line = format!(
            "published {}/{}@{} as shape={} [{}] {}",
            s.owner, s.component, s.version, s.shape, s.kind, s.platform,
        );
        assert_eq!(
            line,
            "published understory/canopy-data-cli@0.1.5 as shape=tool_binary [binary] darwin_arm64"
        );
    }

    #[test]
    fn publish_line_names_the_selected_context_not_the_active_one() {
        use super::{ContextStore, publish_line};
        use crate::contexts::resolve_server_target;

        let td = tempfile::TempDir::new().unwrap();
        let store = ContextStore::with_root(td.path().to_path_buf());
        store
            .create("prod", "https://forest.example.invalid", false)
            .unwrap();

        let prod = resolve_server_target(None, Some("prod"), &store).unwrap();
        assert_eq!(
            publish_line("understory", "fungus", &prod),
            "Publishing understory/fungus to prod (https://forest.example.invalid)"
        );
        let active = resolve_server_target(None, None, &store).unwrap();
        assert_eq!(active.label, "default");
        assert!(resolve_server_target(None, Some("missing"), &store).is_err());
    }

    #[test]
    fn production_guard_compiles_with_disarm_signature() {
        let _ = std::mem::size_of::<AbortOnDrop>();
    }
}

/// Publish the component to the Forest registry.
///
/// Uploads the compiled binary, CUE spec files (forest.cue,
/// forest.component.cue, spec.cue), and the component manifest
/// to the registry. Requires `forest build` to be run first.
///
/// The component is published under {organisation}/{name}@{version}
/// as declared in forest.cue. Requires org membership.
#[derive(clap::Parser)]
pub struct PublishCommand {
    /// Run the local preflight (cue eval, cargo bin check, describe
    /// probe, manifest build) and print what would be published, but
    /// do not contact the registry. TASKS/031 item #5b. Use this to
    /// confirm the publish will land as `[binary]` (not `[files]`) and
    /// against the right context, before flipping the destructive bit.
    #[arg(long = "dry-run")]
    dry_run: bool,

    /// Publish under this version instead of `forest.component.version`
    /// from forest.cue (DATA-583).
    ///
    /// Exists so a tag-triggered CI release can pick the version without
    /// editing a tracked file mid-run: the workflow derives `0.1.8` from
    /// the tag `v0.1.8` and passes it here. Precedence is
    /// **`--version` > `FOREST_COMPONENT_VERSION` > forest.cue** — with
    /// neither the flag nor the env set, the cue value is used exactly as
    /// before, so manual publishing is unchanged.
    ///
    /// Set the *env* form rather than the flag when the build has to agree:
    /// forest's rust/go/docker build components stamp that variable into the
    /// binary. A `prebuilt` build script must stamp it itself; `forest
    /// components version` prints the version this publish would use.
    #[arg(
        long = "version",
        value_name = "VERSION",
        env = "FOREST_COMPONENT_VERSION"
    )]
    version: Option<String>,
}

impl PublishCommand {
    pub fn for_bootstrap() -> Self {
        Self {
            dry_run: false,
            version: None,
        }
    }

    fn resolve_version<'a>(&'a self, cue_version: &'a str) -> &'a str {
        effective_version(self.version.as_deref(), cue_version)
    }

    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let current_dir = std::env::current_dir()?;
        let doc = export_forest_cue(&current_dir).await?;

        let component = doc.get("forest").and_then(|f| f.get("component"));

        let project = doc.get("project");

        let name = component
            .and_then(|c| c.get("name"))
            .and_then(|v| v.as_str())
            .or_else(|| project.and_then(|p| p.get("name")).and_then(|v| v.as_str()))
            .context("component or project name is required")?;

        let cue_version = cue_component_version(&doc);
        let version = self.resolve_version(cue_version);
        if version != cue_version {
            crate::ui::status(format!(
                "Version overridden: {cue_version} (forest.cue) → {version}"
            ));
            warn_if_build_saw_a_different_version(component, version);
        }

        let organisation = project
            .and_then(|p| p.get("organisation"))
            .and_then(|v| v.as_str())
            .context("project.organisation is required")?;

        tracing::info!("publishing component {organisation}/{name}@{version}");

        ensure_authenticated(state).await?;

        if !self.dry_run {
            sync_project_fields(state, &current_dir, organisation, name, &doc).await?;
        }

        let external = component.and_then(|c| c.get("external"));
        if let Some(external_block) = external {
            return publish_external(
                state,
                &current_dir,
                organisation,
                name,
                version,
                &doc,
                external_block,
                self.dry_run,
            )
            .await;
        }

        let upload_type = component
            .and_then(|c| c.get("upload"))
            .and_then(|u| u.get("type"))
            .and_then(|v| v.as_str());
        if upload_type == Some("prebuilt") {
            return publish_prebuilt(
                state,
                &current_dir,
                organisation,
                name,
                version,
                &doc,
                self.dry_run,
            )
            .await;
        }

        let pf_ctx = crate::services::preflight::PreflightContext {
            current_dir: current_dir.clone(),
            doc: doc.clone(),
            organisation: organisation.to_string(),
            component_name: name.to_string(),
            version: version.to_string(),
        };
        let checks = crate::services::preflight::standard_checks();
        if let Err(failures) = crate::services::preflight::run_checks(&pf_ctx, &checks) {
            let manifest = crate::diagnostics::CueManifestSource::load(&current_dir);
            return Err(crate::diagnostics::report(
                crate::diagnostics::PublishPreflightFailed::new(&failures, manifest.as_ref()),
            ));
        }

        let binary = component_binary::resolve_publishable_binary(
            &current_dir,
            name,
            Some(organisation),
            Some(name),
            Some(version),
        );

        let upload_section = component.and_then(|c| c.get("upload"));
        let upload_type = upload_section
            .and_then(|u| u.get("type"))
            .and_then(|v| v.as_str());
        let is_deno_component = upload_type == Some("deno")
            || (current_dir.join("deno.json").exists()
                && current_dir.join("src").join("main.ts").exists());

        let (descriptor, kind) = if let Some(bp) = binary.as_ref() {
            let desc = if let Some(cached) = component_binary::load_cached_descriptor(&current_dir)
            {
                cached
            } else {
                component_binary::describe_component(bp).await?
            };
            (Some(desc), "binary")
        } else if is_deno_component {
            let desc = component_binary::load_cached_descriptor_with_meta(
                &current_dir,
                Some(organisation),
                Some(name),
                Some(version),
            )
            .or_else(|| component_binary::load_cached_descriptor(&current_dir));
            (desc, "deno")
        } else {
            (None, "cue")
        };

        let mut manifest = serde_json::json!({
            "name": name,
            "organisation": organisation,
            "version": version,
            "kind": kind,
        });

        if let Some(ref desc) = descriptor {
            manifest["protocol_version"] = serde_json::json!(desc.protocol_version);
            let method_names: Vec<String> = desc.methods.iter().map(|m| m.name.clone()).collect();
            manifest["methods"] = serde_json::json!(method_names);
            manifest["capabilities"] = serde_json::json!({ "methods": desc.methods });
            if let Some(tool) = describe_response_tool_facet(desc) {
                manifest["tool"] = tool;
            }

            if binary.is_some() {
                let mut platforms = serde_json::Map::new();
                for (os, arch, path) in publishable_binaries(&current_dir, name)? {
                    let manifest_os = if os == "macos" { "darwin" } else { os.as_str() };
                    let binary_content = tokio::fs::read(&path).await?;
                    let sha256 = hex::encode(Sha256::digest(&binary_content));
                    platforms.insert(
                        format!("{manifest_os}_{arch}"),
                        serde_json::json!({
                            "sha256": sha256,
                            "size": binary_content.len(),
                        }),
                    );
                }
                manifest["platforms"] = serde_json::Value::Object(platforms);
            }
        }

        if let Some(include) = include_manifest_value(&doc)? {
            manifest["include"] = include;
        }

        tracing::info!(
            "manifest: kind={}, {}",
            kind,
            descriptor
                .as_ref()
                .map(|d| format!("{} methods", d.methods.len()))
                .unwrap_or_else(|| "CUE-only (no binary)".to_string()),
        );

        if self.dry_run {
            print_publish_context(state, organisation, name);
            let has_tool = descriptor
                .as_ref()
                .and_then(describe_response_tool_facet)
                .is_some();
            let has_methods = descriptor
                .as_ref()
                .map(|d| !d.methods.is_empty())
                .unwrap_or(false);
            let listed: Vec<String> = if binary.is_some() {
                publishable_binaries(&current_dir, name)?
                    .into_iter()
                    .map(|(os, arch, _)| {
                        let os = if os == "macos" {
                            "darwin".to_string()
                        } else {
                            os
                        };
                        format!("{os}_{arch}")
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let platform = if listed.is_empty() {
                "no-platform".to_string()
            } else {
                listed.join(", ")
            };
            let shape = derive_summary_shape(kind, has_tool, has_methods);
            if let Some(msg) = single_platform_tool_warning(has_tool, &listed, upload_type) {
                crate::ui::warn(msg);
            }
            eprintln!(
                "dry-run: would publish {}/{}@{} as shape={} [{}] {}",
                organisation, name, version, shape, kind, platform,
            );
            eprintln!("  no upload was performed.");
            return Ok(());
        }

        print_publish_context(state, organisation, name);
        let client = state.grpc_client();
        tracing::info!("beginning upload");
        let upload_context = client
            .begin_component_upload(organisation, name, version)
            .await?;

        let abort_guard = AbortOnDrop::new(client.clone(), &upload_context);

        if binary.is_some() {
            let to_upload = publishable_binaries(&current_dir, name)?;
            if let Some(msg) = single_platform_tool_warning(
                descriptor
                    .as_ref()
                    .and_then(describe_response_tool_facet)
                    .is_some(),
                &to_upload
                    .iter()
                    .map(|(os, arch, _)| {
                        let os = if os == "macos" { "darwin" } else { os.as_str() };
                        format!("{os}_{arch}")
                    })
                    .collect::<Vec<_>>(),
                upload_type,
            ) {
                crate::ui::warn(msg);
            }
            for (os, arch, path) in to_upload {
                let upload_os = if os == "macos" { "darwin" } else { os.as_str() };
                let binary_content = tokio::fs::read(&path).await?;
                let sha256 = hex::encode(Sha256::digest(&binary_content));
                tracing::info!(
                    "uploading binary for {upload_os}/{arch} ({} bytes)",
                    binary_content.len()
                );
                client
                    .upload_component_binary(
                        &upload_context,
                        upload_os,
                        &arch,
                        &sha256,
                        &binary_content,
                        Some(&format!("Uploading binary ({upload_os}/{arch})")),
                    )
                    .await?;
            }
        }

        let uploaded_binaries: Vec<std::path::PathBuf> = if binary.is_some() {
            publishable_binaries(&current_dir, name)?
                .into_iter()
                .map(|(_, _, path)| path)
                .collect()
        } else {
            Vec::new()
        };

        let extra = match kind {
            "deno" => deno_meta_extra(organisation, name, version)
                .await
                .into_iter()
                .collect(),
            _ => Vec::new(),
        };

        upload_component_files(
            &client,
            &upload_context,
            &current_dir,
            &uploaded_binaries,
            &DeclaredPaths::from_doc(&doc),
            extra,
        )
        .await?;

        if kind == "binary" || kind == "external" {
            tracing::info!("publishing manifest");
            let manifest_json = serde_json::to_string(&manifest)?;
            client
                .publish_component_manifest(&upload_context, &manifest_json)
                .await?;
        } else {
            tracing::info!("{kind}-only component — skipping manifest publish");
        }

        tracing::info!("committing upload");
        client.commit_component_upload(&upload_context).await?;
        abort_guard.disarm();

        let has_tool = descriptor
            .as_ref()
            .and_then(describe_response_tool_facet)
            .is_some();
        let has_methods = descriptor
            .as_ref()
            .map(|d| !d.methods.is_empty())
            .unwrap_or(false);
        let platform = if binary.is_some() {
            let (os, arch) = component_binary::current_platform();
            let platform_os = if os == "macos" { "darwin" } else { os };
            format!("{platform_os}_{arch}")
        } else {
            "no-platform".to_string()
        };
        PublishSummary {
            owner: organisation.to_string(),
            component: name.to_string(),
            version: version.to_string(),
            shape: derive_summary_shape(kind, has_tool, has_methods),
            kind,
            platform,
        }
        .print();

        Ok(())
    }
}

async fn ensure_authenticated(state: &State) -> anyhow::Result<()> {
    if std::env::var("FOREST_TOKEN")
        .ok()
        .is_some_and(|t| !t.trim().is_empty())
    {
        tracing::debug!("authenticating with FOREST_TOKEN from the environment");
        return Ok(());
    }

    let logged_in = matches!(state.user_state().get_state().await, Ok(Some(_)));
    if logged_in {
        return Ok(());
    }

    anyhow::bail!(
        "not authenticated — refusing to publish.\n\n\
         In CI: set FOREST_TOKEN to a forest token with write access to this \
         organisation.\n  \
         Create one with `forest auth token create --name <ci-bot>` and store it as a \
         repository or organisation secret.\n\n\
         Interactively: run `forest auth login` first.\n\n\
         forest never prompts for credentials during a publish, so an unattended run \
         with no token can only fail — this is that failure, before anything was \
         uploaded."
    )
}

pub(crate) async fn export_forest_cue(
    current_dir: &std::path::Path,
) -> anyhow::Result<serde_json::Value> {
    let mut cue_args = vec![
        "export".to_string(),
        "--out".to_string(),
        "json".to_string(),
    ];
    let mut dir_entries = tokio::fs::read_dir(current_dir).await?;
    while let Some(entry) = dir_entries.next_entry().await? {
        if entry.path().extension().and_then(|e| e.to_str()) == Some("cue") {
            cue_args.push(entry.file_name().to_string_lossy().to_string());
        }
    }

    let output = crate::tools::cue::output(|| {
        let mut cmd = tokio::process::Command::new("cue");
        cmd.args(&cue_args);
        if let Ok(registry) = std::env::var("CUE_REGISTRY") {
            cmd.env("CUE_REGISTRY", registry);
        }
        cmd
    })
    .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(crate::diagnostics::report(
            crate::diagnostics::CueEvalError::from_cue_stderr(current_dir, &stderr),
        ));
    }

    Ok(serde_json::from_slice(&output.stdout)?)
}

pub(crate) fn cue_component_version(doc: &serde_json::Value) -> &str {
    doc.pointer("/forest/component/version")
        .and_then(|v| v.as_str())
        .unwrap_or("0.1.0")
}

pub(crate) fn effective_version<'a>(
    override_version: Option<&'a str>,
    cue_version: &'a str,
) -> &'a str {
    match override_version.map(str::trim) {
        Some(v) if !v.is_empty() => v,
        _ => cue_version,
    }
}

fn warn_if_build_saw_a_different_version(component: Option<&serde_json::Value>, version: &str) {
    let declares_binary = component
        .and_then(|c| c.pointer("/upload/architectures"))
        .and_then(|v| v.as_object())
        .map(|o| !o.is_empty())
        .unwrap_or(false);
    if !declares_binary {
        return;
    }

    let build_saw = std::env::var("FOREST_COMPONENT_VERSION").ok();
    if build_saw.as_deref().map(str::trim) == Some(version) {
        return;
    }

    eprintln!(
        "warning: publishing as {version}, but FOREST_COMPONENT_VERSION does not carry \
         that value, so the build stamped a different version into the binary.\n         \
         Export it instead of (or as well as) passing --version, and rebuild with it:\n         \
         export FOREST_COMPONENT_VERSION={version}, then build (a prebuilt build script \
         reads it from `forest components version`) and `forest publish`"
    );
}

async fn sync_project_fields(
    state: &State,
    current_dir: &std::path::Path,
    organisation: &str,
    name: &str,
    doc: &serde_json::Value,
) -> anyhow::Result<()> {
    let client = state.grpc_client();

    client
        .create_project(organisation, name)
        .await
        .context("ensure project exists")?;

    let project = doc.get("project");

    let description = project
        .and_then(|p| p.get("description"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let metadata = project
        .and_then(|p| p.get("metadata"))
        .map(extract_project_metadata)
        .unwrap_or_default();

    let readme = read_optional_readme(current_dir).await?;

    client
        .update_project(
            organisation,
            name,
            Some(readme),
            Some(description),
            Some(metadata),
        )
        .await
        .context("push project description + metadata + readme")?;

    Ok(())
}

fn extract_project_metadata(meta: &serde_json::Value) -> ProjectMetadata {
    let s = |key: &str| -> String {
        meta.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let tags = meta
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    ProjectMetadata {
        git_url: s("git_url"),
        homepage: s("homepage"),
        docs_url: s("docs_url"),
        support_url: s("support_url"),
        domain: s("domain"),
        owner: s("owner"),
        tags,
    }
}

async fn read_optional_readme(current_dir: &std::path::Path) -> anyhow::Result<String> {
    for candidate in ["README.md", "Readme.md", "readme.md"] {
        let p = current_dir.join(candidate);
        match tokio::fs::read_to_string(&p).await {
            Ok(contents) => return Ok(contents),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("read {}", p.display())),
        }
    }
    Ok(String::new())
}

fn describe_response_tool_facet(
    desc: &forest_sdk::ComponentDescriptor,
) -> Option<serde_json::Value> {
    desc.tool.as_ref().map(|t| {
        let mut obj = serde_json::json!({
            "name": t.name,
            "argv_passthrough": t.argv_passthrough,
        });
        if let Some(d) = &t.description {
            obj["description"] = serde_json::json!(d);
        }
        obj
    })
}

fn include_manifest_value(doc: &serde_json::Value) -> anyhow::Result<Option<serde_json::Value>> {
    let include = match doc.pointer("/forest/component/include") {
        Some(v) if !v.is_null() => v,
        _ => return Ok(None),
    };
    let obj = include
        .as_object()
        .context("forest.component.include must be an object")?;

    if let Some(env) = obj.get("env").filter(|v| !v.is_null()) {
        let env_obj = env
            .as_object()
            .context("forest.component.include.env must be a map of string to string")?;
        for (key, value) in env_obj {
            forest_manifest::names::validate_env_name(key).map_err(|e| {
                anyhow::anyhow!("include.env key {key:?} is not a valid env name: {e:?}")
            })?;
            let val = value
                .as_str()
                .with_context(|| format!("include.env.{key} must be a string"))?;
            forest_manifest::names::validate_env_value(val)
                .map_err(|e| anyhow::anyhow!("include.env.{key} has an invalid value: {e:?}"))?;
            if looks_secret(key) {
                eprintln!(
                    "warning: include.env.{key} looks like a secret — `include.env` is \
                     plain text in the published manifest and visible on the component \
                     page; it is not a secrets mechanism."
                );
            }
        }
    }

    if let Some(shell) = obj.get("shell").filter(|v| !v.is_null()) {
        let shell_obj = shell
            .as_object()
            .context("forest.component.include.shell must be an object")?;
        if let Some(init) = shell_obj.get("init").filter(|v| !v.is_null()) {
            let init_obj = init.as_object().context(
                "forest.component.include.shell.init must be an object keyed by shell name",
            )?;
            for (shell_name, argv) in init_obj {
                if !forest_manifest::SUPPORTED_SHELLS.contains(&shell_name.as_str()) {
                    anyhow::bail!(
                        "include.shell.init.{shell_name}: unknown shell; supported: {}",
                        forest_manifest::SUPPORTED_SHELLS.join(", "),
                    );
                }
                let args = argv.as_array().with_context(|| {
                    format!("include.shell.init.{shell_name} must be an array of strings")
                })?;
                if args.is_empty() {
                    anyhow::bail!(
                        "include.shell.init.{shell_name} must not be empty — it is the argv \
                         forest runs against your binary to get the script, e.g. \
                         [\"init\", \"{shell_name}\"]"
                    );
                }
                for a in args {
                    a.as_str().with_context(|| {
                        format!("include.shell.init.{shell_name} entries must be strings")
                    })?;
                }
            }
        }
    }

    Ok(Some(include.clone()))
}

fn looks_secret(key: &str) -> bool {
    let u = key.to_ascii_uppercase();
    u == "PASSWORD"
        || u.ends_with("_SECRET")
        || u.ends_with("_TOKEN")
        || u.ends_with("_KEY")
        || u.ends_with("_PASS")
        || u.ends_with("_PASSWORD")
}

async fn publish_external(
    state: &State,
    current_dir: &std::path::Path,
    organisation: &str,
    name: &str,
    version: &str,
    doc: &serde_json::Value,
    external_block: &serde_json::Value,
    dry_run: bool,
) -> anyhow::Result<()> {
    let raw_platforms = external_block
        .get("platforms")
        .and_then(|v| v.as_array())
        .context("forest.component.external.platforms must be an array")?;

    let mut platforms = serde_json::Map::new();
    for entry in raw_platforms {
        let os = entry
            .get("os")
            .and_then(|v| v.as_str())
            .context("platform entry missing `os`")?;
        let arch = entry
            .get("arch")
            .and_then(|v| v.as_str())
            .context("platform entry missing `arch`")?;
        let sha256 = entry
            .get("sha256")
            .and_then(|v| v.as_str())
            .context("platform entry missing `sha256`")?;
        let url = entry
            .get("url")
            .and_then(|v| v.as_str())
            .context("platform entry missing `url`")?;
        let archive = entry
            .get("archive")
            .and_then(|v| v.as_str())
            .unwrap_or("none");

        let mut platform_obj = serde_json::json!({
            "sha256": sha256,
            "url": url,
            "archive": archive,
        });
        if let Some(b) = entry.get("binary_in_archive").and_then(|v| v.as_str()) {
            platform_obj["binary_in_archive"] = serde_json::json!(b);
        }
        if let Some(a) = entry.get("archive_sha256").and_then(|v| v.as_str()) {
            platform_obj["archive_sha256"] = serde_json::json!(a);
        }
        let manifest_os = if os == "macos" { "darwin" } else { os };
        platforms.insert(format!("{manifest_os}_{arch}"), platform_obj);
    }

    let tool_facet = eval_tool_facet(current_dir).await?;

    let mut manifest = serde_json::json!({
        "name": name,
        "organisation": organisation,
        "version": version,
        "kind": "external",
        "tool": tool_facet,
        "platforms": platforms,
    });
    if let Some(include) = include_manifest_value(doc)? {
        manifest["include"] = include;
    }

    tracing::info!(
        "publishing external manifest: {organisation}/{name}@{version} ({} platforms)",
        platforms.len()
    );

    print_publish_context(state, organisation, name);

    if dry_run {
        let listed: Vec<String> = platforms.keys().cloned().collect();
        eprintln!(
            "dry-run: would publish {organisation}/{name}@{version} as shape=tool_external [external] {}",
            if listed.is_empty() {
                "no-platform".to_string()
            } else {
                listed.join(", ")
            },
        );
        eprintln!("  no upload was performed.");
        return Ok(());
    }

    let client = state.grpc_client();
    let upload_context = client
        .begin_component_upload(organisation, name, version)
        .await?;
    let abort_guard = AbortOnDrop::new(client.clone(), &upload_context);

    upload_component_files(
        &client,
        &upload_context,
        current_dir,
        &[],
        &DeclaredPaths::from_doc(doc),
        Vec::new(),
    )
    .await?;

    let manifest_json = serde_json::to_string(&manifest)?;
    client
        .publish_component_manifest(&upload_context, &manifest_json)
        .await?;
    client.commit_component_upload(&upload_context).await?;
    abort_guard.disarm();

    let platform_key = platforms
        .keys()
        .next()
        .cloned()
        .unwrap_or_else(|| "no-platform".to_string());
    PublishSummary {
        owner: organisation.to_string(),
        component: name.to_string(),
        version: version.to_string(),
        shape: "tool_external",
        kind: "external",
        platform: platform_key,
    }
    .print();
    Ok(())
}

async fn publish_prebuilt(
    state: &State,
    current_dir: &std::path::Path,
    organisation: &str,
    name: &str,
    version: &str,
    doc: &serde_json::Value,
    dry_run: bool,
) -> anyhow::Result<()> {
    let prebuilt = doc
        .pointer("/forest/component/upload/prebuilt")
        .and_then(|v| v.as_object())
        .context(
            "forest.component.upload.prebuilt must be a map of os → arch → path \
             when upload.type == \"prebuilt\"",
        )?;

    let tool_facet = eval_tool_facet(current_dir).await?;

    let mut platforms_for_manifest = serde_json::Map::new();
    let mut uploads: Vec<(String, String, Vec<u8>, String)> = Vec::new();
    let mut uploaded_binary_paths: Vec<std::path::PathBuf> = Vec::new();
    for (os, archs) in prebuilt {
        let archs = archs
            .as_object()
            .with_context(|| format!("prebuilt.{os} must be a map of arch → path"))?;
        for (arch, path_val) in archs {
            let rel_path = path_val
                .as_str()
                .with_context(|| format!("prebuilt.{os}.{arch} must be a string path"))?;
            let abs_path = current_dir.join(rel_path);
            let bytes = tokio::fs::read(&abs_path)
                .await
                .with_context(|| format!("reading prebuilt binary {}", abs_path.display()))?;
            let sha256 = hex::encode(Sha256::digest(&bytes));
            uploaded_binary_paths.push(abs_path.clone());

            let manifest_os = if os == "macos" { "darwin" } else { os.as_str() };

            platforms_for_manifest.insert(
                format!("{manifest_os}_{arch}"),
                serde_json::json!({
                    "sha256": sha256,
                    "size": bytes.len(),
                }),
            );
            uploads.push((manifest_os.to_string(), arch.to_string(), bytes, sha256));
        }
    }

    if uploads.is_empty() {
        anyhow::bail!("prebuilt block declared no platforms");
    }

    let mut manifest = serde_json::json!({
        "name": name,
        "organisation": organisation,
        "version": version,
        "kind": "binary",
        "protocol_version": "1.1",
        "methods": [],
        "tool": tool_facet,
        "capabilities": { "methods": [] },
        "platforms": platforms_for_manifest,
    });
    if let Some(include) = include_manifest_value(doc)? {
        manifest["include"] = include;
    }

    tracing::info!(
        "publishing prebuilt component {organisation}/{name}@{version} ({} platforms)",
        uploads.len(),
    );

    if let Some(msg) = single_platform_tool_warning(
        !tool_facet.is_null(),
        &uploads
            .iter()
            .map(|(os, arch, _, _)| format!("{os}_{arch}"))
            .collect::<Vec<_>>(),
        Some("prebuilt"),
    ) {
        crate::ui::warn(msg);
    }

    print_publish_context(state, organisation, name);

    if dry_run {
        let listed: Vec<String> = uploads
            .iter()
            .map(|(os, arch, _, _)| format!("{os}_{arch}"))
            .collect();
        eprintln!(
            "dry-run: would publish {organisation}/{name}@{version} as shape=tool_binary [binary] {}",
            listed.join(", ")
        );
        eprintln!("  no upload was performed.");
        return Ok(());
    }

    let client = state.grpc_client();
    let upload_context = client
        .begin_component_upload(organisation, name, version)
        .await?;
    let abort_guard = AbortOnDrop::new(client.clone(), &upload_context);

    for (os, arch, bytes, sha256) in uploads {
        tracing::info!(
            "uploading binary {os}/{arch} ({} bytes, sha {})",
            bytes.len(),
            &sha256[..12],
        );
        client
            .upload_component_binary(
                &upload_context,
                &os,
                &arch,
                &sha256,
                &bytes,
                Some("Uploading binary"),
            )
            .await?;
    }

    upload_component_files(
        &client,
        &upload_context,
        current_dir,
        &uploaded_binary_paths,
        &DeclaredPaths::from_doc(doc),
        Vec::new(),
    )
    .await?;

    let manifest_json = serde_json::to_string(&manifest)?;
    client
        .publish_component_manifest(&upload_context, &manifest_json)
        .await?;
    client.commit_component_upload(&upload_context).await?;
    abort_guard.disarm();

    let mut platform_keys: Vec<String> = platforms_for_manifest.keys().cloned().collect();
    platform_keys.sort();
    let platform = if platform_keys.is_empty() {
        "no-platform".to_string()
    } else {
        platform_keys.join(",")
    };
    PublishSummary {
        owner: organisation.to_string(),
        component: name.to_string(),
        version: version.to_string(),
        shape: "tool_binary",
        kind: "binary",
        platform,
    }
    .print();
    Ok(())
}

#[derive(Debug, Default, Clone)]
struct DeclaredPaths {
    include: Option<Vec<String>>,
    exclude: Vec<String>,
}

impl DeclaredPaths {
    fn from_doc(doc: &serde_json::Value) -> Self {
        let globs = |key: &str| -> Option<Vec<String>> {
            doc.pointer(&format!("/forest/component/paths/{key}"))?
                .as_array()
                .map(|entries| {
                    entries
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
        };

        Self {
            include: globs("include").filter(|globs| !globs.is_empty()),
            exclude: globs("exclude").unwrap_or_default(),
        }
    }
}

async fn collect_component_files(
    dir: &std::path::Path,
    binary_paths: &[std::path::PathBuf],
    paths: &DeclaredPaths,
) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
    use crate::services::component_walk::{WalkConfig, component_walk};

    let mut forestignore = paths.exclude.clone();
    if let Ok(contents) = tokio::fs::read_to_string(dir.join(".forestignore")).await {
        forestignore.extend(
            contents
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(str::to_string),
        );
    }

    let result = component_walk(
        dir,
        &WalkConfig {
            binary_paths: binary_paths.to_vec(),
            forestignore,
            allowlist: paths.include.clone(),
            ..WalkConfig::default()
        },
    )?;

    let mut files = Vec::with_capacity(result.include.len());
    for entry in &result.include {
        let content = tokio::fs::read(&entry.abs_path)
            .await
            .with_context(|| format!("read {}", entry.rel_path))?;
        files.push((entry.rel_path.clone(), content));
    }

    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

const PACKAGE_DIR: &str = ".forest/component/package";

async fn upload_component_files(
    client: &crate::grpc::GrpcClient,
    upload_context: &str,
    dir: &std::path::Path,
    binary_paths: &[std::path::PathBuf],
    paths: &DeclaredPaths,
    extra: Vec<(String, Vec<u8>)>,
) -> anyhow::Result<usize> {
    let files = stage_component_package(dir, binary_paths, paths, extra).await?;

    if files.is_empty() {
        return Ok(0);
    }

    tracing::info!(
        "uploading {} component file(s) from {}",
        files.len(),
        PACKAGE_DIR
    );
    for (rel_path, content) in &files {
        client
            .upload_component_file(upload_context, rel_path, content)
            .await
            .with_context(|| format!("upload component file: {rel_path}"))?;
    }

    Ok(files.len())
}

async fn stage_component_package(
    dir: &std::path::Path,
    binary_paths: &[std::path::PathBuf],
    paths: &DeclaredPaths,
    extra: Vec<(String, Vec<u8>)>,
) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
    let mut files = collect_component_files(dir, binary_paths, paths).await?;

    for (rel_path, content) in extra {
        match files.iter_mut().find(|(name, _)| *name == rel_path) {
            Some(existing) => existing.1 = content,
            None => files.push((rel_path, content)),
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let package_root = dir.join(PACKAGE_DIR);
    if tokio::fs::try_exists(&package_root).await.unwrap_or(false) {
        tokio::fs::remove_dir_all(&package_root)
            .await
            .with_context(|| format!("clear {}", package_root.display()))?;
    }

    for (rel_path, content) in &files {
        let staged = package_root.join(rel_path);
        if let Some(parent) = staged.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("create {}", parent.display()))?;
        }
        tokio::fs::write(&staged, content)
            .await
            .with_context(|| format!("stage {rel_path}"))?;
    }
    if !files.is_empty() {
        let ignore = dir.join(".forest/component/.gitignore");
        tokio::fs::write(&ignore, "/.gitignore\n/package/\n")
            .await
            .with_context(|| format!("write {}", ignore.display()))?;
    }

    Ok(files)
}

async fn deno_meta_extra(
    organisation: &str,
    name: &str,
    version: &str,
) -> Option<(String, Vec<u8>)> {
    let meta_dir = component_binary::component_meta_dir(organisation, name, version)?;
    let meta_path = meta_dir.join("meta.json");

    match tokio::fs::read(&meta_path).await {
        Ok(content) => Some((".forest/component/meta.json".to_string(), content)),
        Err(_) => {
            tracing::warn!(
                "no meta.json found at {} — run `forest run build` before `forest publish`",
                meta_path.display()
            );
            None
        }
    }
}

async fn eval_tool_facet(dir: &std::path::Path) -> anyhow::Result<serde_json::Value> {
    let output = crate::tools::cue::output(|| {
        let mut cmd = tokio::process::Command::new("cue");
        cmd.current_dir(dir)
            .args(["eval", "--out=json", "-e", "#Tool", "."]);
        if let Ok(registry) = std::env::var("CUE_REGISTRY") {
            cmd.env("CUE_REGISTRY", registry);
        }
        cmd
    })
    .await
    .context("running `cue eval -e #Tool`")?;
    if !output.status.success() {
        anyhow::bail!(
            "cue eval -e #Tool failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let v: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("parsing cue eval -e #Tool output")?;
    Ok(v)
}

#[cfg(test)]
mod publish_line_tests {
    use super::*;
    use crate::contexts::ServerTarget;

    #[test]
    fn publishing_never_echoes_server_url_credentials() {
        let server = "https://private-user:private-password@api.example/registry?token=private-query#private-fragment";
        for label in ["work", server] {
            let target = ServerTarget {
                label: label.into(),
                server: server.into(),
            };
            let line = publish_line("acme", "tool", &target);
            assert!(line.contains("api.example/registry"));
            for secret in [
                "private-user",
                "private-password",
                "private-query",
                "private-fragment",
            ] {
                assert!(!line.contains(secret));
            }
        }
    }

    #[test]
    fn an_invalid_server_url_does_not_echo_potential_credentials() {
        let server = "https://[invalid?token=private-query";
        let target = ServerTarget {
            label: server.into(),
            server: server.into(),
        };
        let line = publish_line("acme", "tool", &target);
        assert!(!line.contains("private-query"));
    }
}

#[cfg(test)]
mod version_override_tests {
    use super::*;

    fn with_version(version: Option<&str>) -> PublishCommand {
        PublishCommand {
            dry_run: false,
            version: version.map(str::to_string),
        }
    }

    #[test]
    fn cue_version_is_used_when_no_override_is_given() {
        assert_eq!(with_version(None).resolve_version("0.1.7"), "0.1.7");
    }

    #[test]
    fn override_wins_over_the_cue_version() {
        assert_eq!(
            with_version(Some("0.1.8")).resolve_version("0.1.7"),
            "0.1.8"
        );
    }

    #[test]
    fn blank_and_whitespace_overrides_fall_back_to_cue() {
        assert_eq!(with_version(Some("")).resolve_version("0.1.7"), "0.1.7");
        assert_eq!(with_version(Some("   ")).resolve_version("0.1.7"), "0.1.7");
    }

    #[test]
    fn override_is_trimmed() {
        assert_eq!(
            with_version(Some(" 0.1.8\n")).resolve_version("0.1.7"),
            "0.1.8"
        );
    }

    #[test]
    fn bootstrap_ignores_the_ambient_env_override() {
        assert!(PublishCommand::for_bootstrap().version.is_none());
        assert_eq!(
            PublishCommand::for_bootstrap().resolve_version("0.1.7"),
            "0.1.7"
        );
    }

    #[test]
    fn arg_is_wired_to_the_env_var() {
        use clap::CommandFactory;
        let cmd = PublishCommand::command();
        let arg = cmd
            .get_arguments()
            .find(|a| a.get_id() == "version")
            .expect("--version arg should exist");
        assert_eq!(
            arg.get_env().and_then(|e| e.to_str()),
            Some("FOREST_COMPONENT_VERSION"),
        );
    }
}

#[cfg(test)]
mod include_tests {
    use super::*;

    fn doc_with_include(include: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "forest": { "component": { "include": include } } })
    }

    #[test]
    fn no_include_returns_none() {
        let doc = serde_json::json!({ "forest": { "component": {} } });
        assert!(include_manifest_value(&doc).unwrap().is_none());
    }

    #[test]
    fn valid_env_is_passed_through() {
        let doc = doc_with_include(serde_json::json!({
            "env": { "FUNGUS_SERVER": "https://prod" }
        }));
        let out = include_manifest_value(&doc).unwrap().unwrap();
        assert_eq!(out["env"]["FUNGUS_SERVER"], "https://prod");
    }

    #[test]
    fn invalid_env_name_errors() {
        let doc = doc_with_include(serde_json::json!({ "env": { "1BAD": "x" } }));
        assert!(include_manifest_value(&doc).is_err());
    }

    #[test]
    fn non_string_env_value_errors() {
        let doc = doc_with_include(serde_json::json!({ "env": { "FOO": 5 } }));
        assert!(include_manifest_value(&doc).is_err());
    }

    #[test]
    fn valid_shell_block_is_passed_through() {
        let doc = doc_with_include(serde_json::json!({
            "shell": { "init": { "zsh": ["init", "zsh"] } }
        }));
        let out = include_manifest_value(&doc).unwrap().unwrap();
        assert_eq!(out["shell"]["init"]["zsh"][0], "init");
        assert_eq!(out["shell"]["init"]["zsh"][1], "zsh");
    }

    #[test]
    fn unknown_shell_name_errors_at_publish() {
        let doc = doc_with_include(serde_json::json!({
            "shell": { "init": { "zshell": ["init", "zsh"] } }
        }));
        assert!(include_manifest_value(&doc).is_err());
    }

    #[test]
    fn empty_shell_argv_errors_at_publish() {
        let doc = doc_with_include(serde_json::json!({
            "shell": { "init": { "zsh": [] } }
        }));
        assert!(include_manifest_value(&doc).is_err());
    }

    #[test]
    fn non_string_shell_argv_errors_at_publish() {
        let doc = doc_with_include(serde_json::json!({
            "shell": { "init": { "zsh": ["init", 3] } }
        }));
        assert!(include_manifest_value(&doc).is_err());
        let doc = doc_with_include(serde_json::json!({
            "shell": { "init": { "zsh": "init zsh" } }
        }));
        assert!(include_manifest_value(&doc).is_err());
    }

    #[test]
    fn shell_without_init_is_accepted() {
        let doc = doc_with_include(serde_json::json!({ "shell": {} }));
        assert!(include_manifest_value(&doc).unwrap().is_some());
    }

    #[test]
    fn env_and_shell_coexist() {
        let doc = doc_with_include(serde_json::json!({
            "env": { "FUNGUS_SERVER": "https://prod" },
            "shell": { "init": { "fish": ["completion", "fish"] } }
        }));
        let out = include_manifest_value(&doc).unwrap().unwrap();
        assert_eq!(out["env"]["FUNGUS_SERVER"], "https://prod");
        assert_eq!(out["shell"]["init"]["fish"][0], "completion");
    }

    #[test]
    fn secret_name_heuristic() {
        assert!(looks_secret("FUNGUS_TOKEN"));
        assert!(looks_secret("api_secret"));
        assert!(looks_secret("PASSWORD"));
        assert!(looks_secret("AWS_SECRET_ACCESS_KEY"));
        assert!(!looks_secret("FUNGUS_SERVER"));
        assert!(!looks_secret("RUST_LOG"));
    }
}

#[cfg(test)]
mod platform_guard_tests {
    use super::single_platform_tool_warning;

    fn plats(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn warns_when_a_built_tool_ships_only_the_host_platform() {
        let msg = single_platform_tool_warning(true, &plats(&["darwin_arm64"]), Some("rust"))
            .expect("a single-platform tool must warn");
        assert!(msg.contains("darwin_arm64"), "{msg}");
        assert!(
            msg.contains("prebuilt"),
            "must point at the fix, not just the problem: {msg}"
        );
        assert!(
            msg.contains("only ever publishes the host"),
            "must explain why the architectures matrix won't help: {msg}"
        );
    }

    #[test]
    fn warns_for_a_single_platform_prebuilt_tool_but_advises_differently() {
        let msg = single_platform_tool_warning(true, &plats(&["linux_amd64"]), Some("prebuilt"))
            .expect("still a single-platform tool");
        assert!(msg.contains("upload.prebuilt"), "{msg}");
        assert!(
            !msg.contains("switch to"),
            "must not tell a prebuilt author to switch to prebuilt: {msg}"
        );
    }

    #[test]
    fn stays_quiet_for_a_multi_platform_tool() {
        assert_eq!(
            single_platform_tool_warning(
                true,
                &plats(&["darwin_arm64", "darwin_amd64", "linux_amd64", "linux_arm64"]),
                Some("prebuilt"),
            ),
            None
        );
        assert_eq!(
            single_platform_tool_warning(
                true,
                &plats(&["darwin_arm64", "linux_amd64"]),
                Some("rust")
            ),
            None
        );
    }

    #[test]
    fn stays_quiet_for_a_non_tool_component() {
        assert_eq!(
            single_platform_tool_warning(false, &plats(&["darwin_arm64"]), Some("rust")),
            None
        );
    }

    #[test]
    fn stays_quiet_when_there_are_no_platforms_at_all() {
        assert_eq!(single_platform_tool_warning(true, &[], Some("rust")), None);
    }
}
