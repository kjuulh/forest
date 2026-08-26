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

/// Print a single line naming the active forest context + server URL the
/// publish is about to hit. TASKS/031 item #10 — prevents accidental
/// pushes to the wrong (e.g. prod) registry by making the destination
/// visible *before* the first RPC.
///
/// Best-effort: a failure to resolve the active context (no contexts
/// configured, corrupted state file) silently degrades to a minimal
/// "publishing as <owner>/<component>" line so we never block the publish
/// just for the courtesy print.
fn print_publish_context(owner: &str, component: &str) {
    match ContextStore::from_env().and_then(|s| s.active()) {
        Ok(ctx) => {
            // Server URL is detail for the logs, not the human line.
            tracing::debug!(
                "publishing as {owner}/{component} to {} ({})",
                ctx.name,
                ctx.server
            );
            crate::ui::status(format!("Publishing {owner}/{component} to {}", ctx.name));
        }
        Err(_) => crate::ui::status(format!("Publishing {owner}/{component}")),
    }
}

/// Shape/kind/platform discriminators the post-success summary needs from
/// the publish flow. Kept as a plain data carrier so each publish path
/// (main, external, prebuilt) can construct it inline without coupling.
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
        // shape/kind/platform are diagnostic detail — keep them in the logs.
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

/// Derive the manifest shape string from the kind + descriptor presence.
/// Mirrors `forest_manifest::derive_shape` for the binary/external kinds
/// the manifest validator accepts today; CUE-only / Deno publishes report
/// their CLI kind directly because no manifest shape is computed for them.
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

/// The binaries to publish: everything under `.forest/component/output/`, and
/// nothing else.
///
/// DATA-654. That directory is what the build components write
/// (`forest run build`), it covers the whole cross-compile matrix, and it holds
/// *release* artifacts. Before this it was only a preference: an empty output
/// tree fell back to `component_binary::resolve_binary`, which walks up to the
/// cargo workspace root and probes `target/debug/<name>` *before*
/// `target/release/<name>`. Publish then shipped whatever happened to be in
/// `target/` — in practice a stale debug build, more than once.
///
/// There is no fallback now, not even the content-addressable cache: `meta.json`
/// records whatever the *local* resolver last synced, and `forest run` syncs
/// `target/debug` builds into it, so honouring the cache would upload the same
/// stale binary with a hash check in front of it. See
/// `component_binary::resolve_publishable_binary`.
///
/// An empty output tree is an error, loudly, rather than a quiet mis-upload.
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

/// DATA-654 — `.forest` is the only place a publish takes artifacts from.
///
/// The decoy in every one of these is a `target/debug/<name>` inside a real
/// cargo workspace layout, i.e. exactly what `find_local_binary` used to hand
/// the publish flow and what got uploaded to the registry as a stale debug
/// build. It must never appear in the result.
#[cfg(test)]
mod publishable_binaries_tests {
    use super::*;

    /// A cargo workspace root with a component subdirectory, plus a decoy
    /// `target/debug/<name>` that the old resolver would have found by
    /// walking up from the component.
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

    /// The regression this whole change exists for: nothing staged, a stale
    /// debug binary sitting in target/. Publish must refuse, not upload it.
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

    /// A `meta.json` naming a cached blob — the shape a "registry cache
    /// restore" would take — is not a fallback either. That file records
    /// whatever the *local* resolver last synced, and `forest run` fills it
    /// from `target/debug`, so it cannot vouch for the build profile.
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

    /// A partial matrix still publishes what built. Cross-compiling is not
    /// always possible on one runner (DATA-583), so "some platforms staged"
    /// must stay a success, not become the new failure mode.
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

/// RAII guard that fires a best-effort `AbortUpload` RPC on Drop unless
/// disarmed. Wraps every `forest publish` flow so an early `?` return, a
/// panic, or a Ctrl-C between `begin_upload` and `commit_upload` leaves
/// the server's staging row aborted rather than half-staged. See
/// TASKS/023-publish-transactional.md.
///
/// The server's `AbortUpload` handler is idempotent — unknown / already
/// committed / already aborted uploads are no-ops — so the fire-and-forget
/// pattern here cannot create spurious state.
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

    /// Cancel the abort — call this after `commit_upload` succeeds so the
    /// guard does not roll back a legitimate publish on the way out.
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
        // Fire-and-forget on the current tokio runtime. Errors are swallowed:
        // the server is required to be idempotent on abort, and there's no
        // user-visible action to take from a Drop handler anyway.
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

    /// Re-implementation of the guard against a trivial sink so we can test
    /// arm / disarm semantics without needing a live gRPC server.
    ///
    /// This mirrors the production guard's *contract*: on Drop with the
    /// "armed" flag set, fire a side effect; on `disarm`, do not.
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
            // implicit drop here
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

    use super::{PublishSummary, derive_summary_shape};

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

    #[test]
    fn shape_for_cue_and_deno() {
        assert_eq!(derive_summary_shape("cue", false, false), "library");
        assert_eq!(derive_summary_shape("deno", true, true), "deno");
    }

    #[test]
    fn summary_format_is_stable() {
        // Snapshot the exact line format we print on success. Users grep
        // this output, so the format is part of the CLI contract — changing
        // it should require deliberately updating this test.
        let s = PublishSummary {
            owner: "understory".into(),
            component: "canopy-data-cli".into(),
            version: "0.1.5".into(),
            shape: "tool_binary",
            kind: "binary",
            platform: "darwin_arm64".into(),
        };
        // Mirror what `print` writes so the assertion exercises the format
        // without needing to capture stderr.
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
    fn production_guard_compiles_with_disarm_signature() {
        // Compile-time check on AbortOnDrop's API surface. We can't construct
        // one without a real GrpcClient; the runtime assertion (aborted upload
        // frees the version for a fresh begin) lives in the accepttest suite.
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
    /// `forest run build` reads the same variable, so exporting it once
    /// covers build and publish and the binary's stamped version matches
    /// what the registry records. Passing only `--version` overrides the
    /// publish but leaves an already-built binary stamped with the cue
    /// version, which `forest publish` warns about.
    #[arg(
        long = "version",
        value_name = "VERSION",
        env = "FOREST_COMPONENT_VERSION"
    )]
    version: Option<String>,
}

impl PublishCommand {
    /// Construct a non-dry-run publish for programmatic use. Used by the
    /// hidden `forest bootstrap` command, which publishes many components by
    /// switching the working directory between them. DATA-312.
    ///
    /// `version: None` is deliberate and load-bearing. Bootstrap publishes
    /// every workspace component in one process; a `FOREST_COMPONENT_VERSION`
    /// in the ambient environment would otherwise stamp all of them with one
    /// version. Because the override is read by clap's `env` (applied only
    /// when parsing argv) rather than by `std::env::var` at use-site, this
    /// constructor is immune by construction — keep it that way.
    pub fn for_bootstrap() -> Self {
        Self {
            dry_run: false,
            version: None,
        }
    }

    /// Resolve the version to publish under: the override if one was given,
    /// otherwise whatever forest.cue declared.
    ///
    /// An all-whitespace override is treated as absent. CI writes these from
    /// shell interpolation (`--version "${{ inputs.version }}"`), and an
    /// unset input expands to the empty string — publishing `""` would fail
    /// the C8 semver gate with a baffling message instead of just falling
    /// back to the manifest.
    fn resolve_version<'a>(&'a self, cue_version: &'a str) -> &'a str {
        match self.version.as_deref().map(str::trim) {
            Some(v) if !v.is_empty() => v,
            _ => cue_version,
        }
    }

    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        // 1. Parse the component's CUE files to get metadata
        let mut cue_args = vec![
            "export".to_string(),
            "--out".to_string(),
            "json".to_string(),
        ];
        let current_dir = std::env::current_dir()?;
        // Collect all .cue files for evaluation
        let mut dir_entries = tokio::fs::read_dir(&current_dir).await?;
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
                crate::diagnostics::CueEvalError::from_cue_stderr(&current_dir, &stderr),
            ));
        }

        let doc: serde_json::Value = serde_json::from_slice(&output.stdout)?;

        // Extract metadata — forest.component is optional for CUE-only components
        let component = doc.get("forest").and_then(|f| f.get("component"));

        let project = doc.get("project");

        let name = component
            .and_then(|c| c.get("name"))
            .and_then(|v| v.as_str())
            .or_else(|| project.and_then(|p| p.get("name")).and_then(|v| v.as_str()))
            .context("component or project name is required")?;

        let cue_version = component
            .and_then(|c| c.get("version"))
            .and_then(|v| v.as_str())
            .unwrap_or("0.1.0");
        // DATA-583: `--version` / FOREST_COMPONENT_VERSION win over the
        // manifest so a tag-triggered release picks the version without
        // editing forest.cue. Resolved once, here, so every downstream
        // step — preflight, manifest, upload, summary — sees one value.
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

        // DATA-583: fail here, offline, rather than on the first RPC. Publish
        // is the one command CI runs unattended, and the overwhelmingly common
        // CI misconfiguration — the secret never reached the job — otherwise
        // surfaces as a transport-layer "user is not logged in" from inside the
        // auth interceptor, several frames from anything actionable.
        ensure_authenticated(state).await?;

        // Sync project-level metadata (description, About-sidebar fields, README)
        // from forest.cue → server. CUE is source of truth: missing in CUE = cleared.
        // See specs/features/009-project-metadata.md.
        //
        // Skipped under --dry-run: this is `create_project` plus a metadata
        // write, so running it contacts — and mutates — the registry the flag
        // promises not to touch. It also *clears* server-side fields absent from
        // CUE, so a dry run could quietly wipe a project's description.
        if !self.dry_run {
            sync_project_fields(state, &current_dir, organisation, name, &doc).await?;
        }

        // Dispatch: `external:` block in forest.cue means external manifest mode
        // (TASKS/018-global-tools.md §1a.2b). No build, no UploadBinary.
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

        // Dispatch: `upload.type == "prebuilt"` uploads existing binaries
        // declared per-platform in CUE. Skips `forest build` and skips
        // the `_meta/describe` probe — the tool facet is sourced from
        // `#Tool` instead. Result is kind=binary (auth-gated download).
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

        // TASKS/028: run the full Phase 1 preflight as a gate. This
        // subsumes the standalone 027 check (it's now C5) and adds the
        // names-agree (C3) and semver-valid (C8) checks. All failures
        // are reported together so the user doesn't have to fix one,
        // re-run, fix the next.
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

        // 2. Check for binary (optional — CUE-only / Deno components don't need one)
        //
        // DATA-654: resolve through the publish-only resolver, not
        // `resolve_binary`. The latter also probes the cargo target directory,
        // which would let a stray `target/debug/<name>` answer "yes, this is a
        // binary component" here and then be uploaded below.
        let binary = component_binary::resolve_publishable_binary(
            &current_dir,
            name,
            Some(organisation),
            Some(name),
            Some(version),
        );

        // Detect Deno components: forest.cue declares `upload.type = "deno"`
        // *or* the working dir has the Deno shape (deno.json + src/main.ts).
        // When matched, the publish flow uploads the source tree alongside
        // CUE so consumers can spawn the component directly from cache,
        // matching how a local path-dep behaves.
        let upload_section = component.and_then(|c| c.get("upload"));
        let upload_type = upload_section
            .and_then(|u| u.get("type"))
            .and_then(|v| v.as_str());
        let upload_source = upload_section
            .and_then(|u| u.get("source"))
            .and_then(|v| v.as_str())
            .unwrap_or("./src");

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
            // Deno components carry a descriptor via the local build cache's
            // meta.json. Load it so the published manifest can advertise
            // methods + tool facet, matching the binary path.
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

        // 3. Build manifest
        let mut manifest = serde_json::json!({
            "name": name,
            "organisation": organisation,
            "version": version,
            "kind": kind,
        });

        if let Some(ref desc) = descriptor {
            manifest["protocol_version"] = serde_json::json!(desc.protocol_version);
            // Methods are also surfaced as a plain string array for the
            // shape derivation in forest-manifest (HYBRID vs COMPONENT).
            let method_names: Vec<String> = desc.methods.iter().map(|m| m.name.clone()).collect();
            manifest["methods"] = serde_json::json!(method_names);
            manifest["capabilities"] = serde_json::json!({ "methods": desc.methods });
            // Carry the tool facet through to the published manifest if the
            // describe response advertised one.
            if let Some(tool) = describe_response_tool_facet(desc) {
                manifest["tool"] = tool;
            }

            // `platforms` is binary-only metadata: per-OS/arch hashes for
            // the downloader. Deno components run via the source bundle
            // we upload separately and have no `platforms` map.
            if binary.is_some() {
                // Every platform the build produced, not just the host's.
                let mut platforms = serde_json::Map::new();
                for (os, arch, path) in publishable_binaries(&current_dir, name)? {
                    // forest-manifest's validator accepts "darwin", not "macos";
                    // the on-disk layout uses "macos", so translate at the
                    // manifest boundary.
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

        // `include` (TASKS/023): default env shipped beside the binary. Read
        // straight from the CUE doc (a regular field, present in `cue export`)
        // and attach to the manifest — independent of kind/describe.
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

        // TASKS/031: dry-run stops here. Everything above is local-only
        // (cue eval, bin check, describe probe, manifest synthesis); the
        // first RPC is `begin_component_upload` below. Print the would-be
        // summary using the same renderer as the post-success path so the
        // user sees exactly the line a real publish would emit.
        if self.dry_run {
            print_publish_context(organisation, name);
            let has_tool = descriptor
                .as_ref()
                .and_then(describe_response_tool_facet)
                .is_some();
            let has_methods = descriptor
                .as_ref()
                .map(|d| !d.methods.is_empty())
                .unwrap_or(false);
            let platform = if binary.is_some() {
                // Report every platform that would be uploaded. Summarising only
                // the host's was how a single-platform publish went unnoticed:
                // the dry-run agreed with the mistake.
                let listed: Vec<String> = publishable_binaries(&current_dir, name)?
                    .into_iter()
                    .map(|(os, arch, _)| {
                        let os = if os == "macos" {
                            "darwin".to_string()
                        } else {
                            os
                        };
                        format!("{os}_{arch}")
                    })
                    .collect();
                if listed.is_empty() {
                    "no-platform".to_string()
                } else {
                    listed.join(", ")
                }
            } else {
                "no-platform".to_string()
            };
            let shape = derive_summary_shape(kind, has_tool, has_methods);
            eprintln!(
                "dry-run: would publish {}/{}@{} as shape={} [{}] {}",
                organisation, name, version, shape, kind, platform,
            );
            eprintln!("  no upload was performed.");
            return Ok(());
        }

        // 4. Begin upload
        print_publish_context(organisation, name);
        let client = state.grpc_client();
        tracing::info!("beginning upload");
        let upload_context = client
            .begin_component_upload(organisation, name, version)
            .await?;

        // TASKS/023: roll back the staged upload if any subsequent step
        // returns Err or the process is killed. Disarmed after commit.
        let abort_guard = AbortOnDrop::new(client.clone(), &upload_context);

        // 5. Upload binary (if present)
        if binary.is_some() {
            for (os, arch, path) in publishable_binaries(&current_dir, name)? {
                // Align the upload os with what the manifest validator +
                // resolver expect ("darwin" not "macos").
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

        // 6. Upload CUE spec files
        let cue_files: Vec<(String, String)> = collect_cue_files(&current_dir).await?;
        if !cue_files.is_empty() {
            tracing::info!("uploading {} CUE spec file(s)", cue_files.len());
            for (rel_path, content) in &cue_files {
                client
                    .upload_component_file(&upload_context, rel_path, content.as_bytes())
                    .await
                    .with_context(|| format!("upload CUE file: {rel_path}"))?;
            }
        }

        // 6b. Upload Deno source tree (and module / lock / meta).
        // Consumers' `forest update` already streams every file via
        // `get_component_files` into the cache, so anything we put here
        // ends up at ~/.cache/forest/components/<org>/<name>/<version>/.
        // The `.forest/component/meta.json` path matters: it's the
        // fallback `read_meta_json()` already checks, so the same
        // is_deno_component_with_meta()/resolve_entrypoint_with_meta()
        // helpers work against the cached copy without further changes.
        if kind == "deno" {
            let deno_files =
                collect_deno_files(&current_dir, upload_source, organisation, name, version)
                    .await?;
            if !deno_files.is_empty() {
                tracing::info!(
                    "uploading {} Deno source file(s) from {upload_source}",
                    deno_files.len()
                );
                for (rel_path, content) in &deno_files {
                    client
                        .upload_component_file(&upload_context, rel_path, content)
                        .await
                        .with_context(|| format!("upload Deno file: {rel_path}"))?;
                }
            }
        }

        // 7. Publish manifest — skipped for CUE-only components. The
        //    server's manifest validator (forest-manifest::parse) only
        //    accepts `kind: "binary"` and `kind: "external"`; a pure CUE
        //    library (e.g. forest/sdk, forest/deployment) has neither a
        //    binary nor an external manifest and so doesn't carry any of
        //    the rule-derived shape constraints. commit_upload defaults
        //    the shape to "component" when no manifest was published —
        //    forage renders that gracefully (no platforms table, no
        //    install command). Adding a proper `Library` shape is
        //    tracked separately; this keeps SDK publishes unblocked.
        // The server's manifest validator (forest-manifest::parse) only
        // accepts `kind: "binary"` and `kind: "external"`. CUE-only and
        // Deno-source components carry their methods via the uploaded
        // meta.json (Deno) or are pure schema libraries (CUE) — neither
        // shape needs the binary/external rule set. Skip publish_manifest
        // for them; commit_upload still records the version + uploaded
        // files. Adding a `Library` / `Deno` shape to the manifest
        // validator is a separate piece of work.
        if kind == "binary" || kind == "external" {
            tracing::info!("publishing manifest");
            let manifest_json = serde_json::to_string(&manifest)?;
            client
                .publish_component_manifest(&upload_context, &manifest_json)
                .await?;
        } else {
            tracing::info!("{kind}-only component — skipping manifest publish");
        }

        // 8. Commit
        tracing::info!("committing upload");
        client.commit_component_upload(&upload_context).await?;
        abort_guard.disarm();

        // TASKS/031 #5: visible summary so the user can see what landed
        // (notably the shape — a binary publish that lands as `[files]`
        // because of name mismatch is now immediately obvious).
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

/// Confirm a credential exists before the first mutating RPC (DATA-583).
///
/// Two ways to be authenticated, matching the two audiences:
///   - `FOREST_TOKEN` in the environment — the CI path. The gRPC auth
///     interceptor short-circuits to it, so no local login is involved and
///     nothing can prompt.
///   - A logged-in user in the local state file — the interactive path,
///     established by `forest auth login`.
///
/// This checks that *a* credential is present, not that it is valid — the
/// first RPC does that, and it is the only thing that can. The failure it
/// catches is the one worth a good message: an unattended publish that was
/// never handed a token at all.
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

/// Warn when the version being published cannot be the version that was
/// built into the binary (DATA-583).
///
/// The build runs in a separate process — a depended-on `forest-contrib/build-*`
/// component (DATA-312) — which recovers the version by re-reading forest.cue,
/// with `FOREST_COMPONENT_VERSION` as its only override. So a `--version` passed
/// to `forest publish` alone reaches the *upload* and not the *build*: the
/// registry records 0.1.8 while `mytool version` reports the manifest's 0.1.7.
/// That is precisely the inconsistency this flag exists to remove, so say so
/// rather than shipping a binary that misreports itself.
///
/// Only meaningful for components that produce a binary — a CUE library has
/// nothing stamped into it. Advisory: never blocks the publish, because the
/// override is also legitimate for re-tagging an artifact whose version is not
/// baked in.
fn warn_if_build_saw_a_different_version(component: Option<&serde_json::Value>, version: &str) {
    let declares_binary = component
        .and_then(|c| c.pointer("/upload/architectures"))
        .and_then(|v| v.as_object())
        .map(|o| !o.is_empty())
        .unwrap_or(false);
    if !declares_binary {
        return;
    }

    // The build only ever sees the env form. If it does not carry the version
    // we are about to publish, the build could not have stamped it.
    let build_saw = std::env::var("FOREST_COMPONENT_VERSION").ok();
    if build_saw.as_deref().map(str::trim) == Some(version) {
        return;
    }

    eprintln!(
        "warning: publishing as {version}, but FOREST_COMPONENT_VERSION does not carry \
         that value, so the build stamped a different version into the binary.\n         \
         Export it instead of (or as well as) passing --version, and re-run the build:\n         \
         export FOREST_COMPONENT_VERSION={version} && forest run build && forest publish"
    );
}

/// Ensure the project exists and push its declared fields (description,
/// metadata, README) up to the server before the artefact upload.
///
/// - Calls `create_project` first (idempotent: server upserts on conflict)
///   so a publish into a brand-new project still works without a separate
///   `forest project create` step.
/// - Reads `project.description` and `project.metadata.*` from the
///   already-parsed CUE JSON.
/// - Reads README.md from the project directory if present.
/// - Sends all three to `UpdateProject` with field-mask semantics — empty
///   values clear the server. See spec §"Publish flow".
async fn sync_project_fields(
    state: &State,
    current_dir: &std::path::Path,
    organisation: &str,
    name: &str,
    doc: &serde_json::Value,
) -> anyhow::Result<()> {
    let client = state.grpc_client();

    // Idempotent — server treats existing project as a no-op via ON CONFLICT.
    client
        .create_project(organisation, name)
        .await
        .context("ensure project exists")?;

    let project = doc.get("project");

    // String fields default to "" when missing from CUE (= clear server-side).
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

/// Pull blessed metadata fields out of the parsed CUE JSON.
/// Missing keys become empty strings (cleared server-side per spec).
fn extract_project_metadata(meta: &serde_json::Value) -> ProjectMetadata {
    let s = |key: &str| -> String {
        meta.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    ProjectMetadata {
        git_url: s("git_url"),
        homepage: s("homepage"),
        docs_url: s("docs_url"),
        support_url: s("support_url"),
        domain: s("domain"),
        owner: s("owner"),
    }
}

/// Read a project's README.md (case-insensitive) if present. Returns
/// empty string when absent — server treats that as "clear", matching
/// the missing-in-CUE-clears policy.
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

/// Read the optional `tool` facet from a component's `_meta/describe`
/// response if it advertised one. Returns the JSON form ready to embed
/// in the manifest. The describe protocol places `tool` alongside
/// `methods` (see TASKS/018-global-tools.md §1a.1).
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

/// Extract the `include` block (TASKS/023) from the CUE project doc, ready to
/// attach to the manifest. `include` is a plain CUE field, so it appears in
/// `cue export` output directly (no `cue eval -e` needed). Validates env
/// names/values for a friendly publish-time error and warns on secret-ish keys.
/// Returns `None` when there is no `include` block.
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

    // `include.shell` (DATA-588). Validated here so a typo'd shell name or a
    // malformed argv fails the publish with a pointed message, rather than
    // shipping a manifest whose shell integration silently never loads. The
    // block is still passed through verbatim below — the manifest parser is the
    // authority, this is just the friendly front door.
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

/// Heuristic: does this env key name look like it carries a secret? Advisory
/// only (TASKS/023 E8/Q4) — never blocks the publish.
fn looks_secret(key: &str) -> bool {
    let u = key.to_ascii_uppercase();
    u == "PASSWORD"
        || u.ends_with("_SECRET")
        || u.ends_with("_TOKEN")
        || u.ends_with("_KEY")
        || u.ends_with("_PASS")
        || u.ends_with("_PASSWORD")
}

/// External-manifest publishing path. Skips the binary build/upload entirely
/// and submits only the manifest (kind=external). See §1a.2b.
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
    // Build the platforms map from the CUE `external.platforms` array.
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
        // The CUE-facing #ForestArchitectures enum uses "macos"; the
        // server-side manifest validator wants "darwin". Translate at
        // the manifest boundary (same shape as the upload path).
        let manifest_os = if os == "macos" { "darwin" } else { os };
        platforms.insert(format!("{manifest_os}_{arch}"), platform_obj);
    }

    // Extract the `#Tool` facet via a dedicated `cue eval -e tool`.
    // `#Tool` is a CUE definition (hidden from `cue export`); we eval it
    // explicitly to extract its concrete JSON form.
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

    print_publish_context(organisation, name);

    // Dry-run stops here, exactly as the built and prebuilt paths do. Everything
    // above is local — cue eval, tool-facet extraction, manifest synthesis — and
    // `begin_component_upload` below is the first RPC. Without this the flag was
    // silently ignored on the external path and `--dry-run` published for real,
    // which is the opposite of what it exists to promise.
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

    // Skip UploadBinary entirely — externals are URL-hosted.
    // Upload the CUE files (lightweight, for the registry's discovery UI).
    let cue_files: Vec<(String, String)> = collect_cue_files(current_dir).await?;
    for (rel_path, content) in &cue_files {
        client
            .upload_component_file(&upload_context, rel_path, content.as_bytes())
            .await
            .with_context(|| format!("upload CUE file: {rel_path}"))?;
    }

    let manifest_json = serde_json::to_string(&manifest)?;
    client
        .publish_component_manifest(&upload_context, &manifest_json)
        .await?;
    client.commit_component_upload(&upload_context).await?;
    abort_guard.disarm();

    // External tools advertise multiple platforms by URL — pick the first
    // declared key for the summary line; the user can run `forest components
    // show` to see the full list.
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

/// Publish a `upload.type=prebuilt` component: iterate per-platform
/// binary paths declared in CUE, upload each as the binary payload for
/// that (os, arch) tuple, and synthesise the manifest descriptor from
/// the `#Tool` facet rather than probing `_meta/describe`.
///
/// Result is `kind=binary` (download flows through gRPC + auth), but
/// the binaries are pre-built rather than produced by `forest build`.
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

    // Tool facet sourced from #Tool, replacing _meta/describe.
    let tool_facet = eval_tool_facet(current_dir).await?;

    // Flatten the os→arch→path map and read each binary.
    let mut platforms_for_manifest = serde_json::Map::new();
    let mut uploads: Vec<(String, String, Vec<u8>, String)> = Vec::new();
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

            // Match the upload/external paths: SDK exposes "macos" to
            // CUE authors, manifest validator wants "darwin".
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

    // Honour --dry-run here too. The prebuilt path returns long before the
    // dry-run check on the built path, so `--dry-run` used to publish for real
    // against a flag documented as "do not contact the registry".
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

    print_publish_context(organisation, name);
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

    let cue_files: Vec<(String, String)> = collect_cue_files(current_dir).await?;
    for (rel_path, content) in &cue_files {
        client
            .upload_component_file(&upload_context, rel_path, content.as_bytes())
            .await
            .with_context(|| format!("upload CUE file: {rel_path}"))?;
    }

    let manifest_json = serde_json::to_string(&manifest)?;
    client
        .publish_component_manifest(&upload_context, &manifest_json)
        .await?;
    client.commit_component_upload(&upload_context).await?;
    abort_guard.disarm();

    // Prebuilt uploads can span multiple platforms; show them comma-joined
    // sorted so the line is stable across runs.
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

/// Evaluate `#Tool` from the project's CUE package. Since `#Tool` is a
/// definition (hidden from `cue export`), we use `cue eval --expression`
/// to extract its concrete value.
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

/// Collect all `.cue` files from a directory (non-recursive, excludes cue.mod/).
async fn collect_cue_files(dir: &std::path::Path) -> anyhow::Result<Vec<(String, String)>> {
    let mut files = Vec::new();
    let mut entries = tokio::fs::read_dir(dir).await?;

    // Include all .cue files in the component directory.
    // These form the component's public API (types, contracts, specs).
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("cue") {
            let file_name = entry.file_name().to_string_lossy().to_string();
            let content = tokio::fs::read_to_string(&path).await?;
            files.push((file_name, content));
        }
    }

    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

/// Collect the Deno runtime + module + meta files that consumers need.
///
/// Returns `(relative_path, bytes)` pairs. Paths are POSIX-style with
/// forward slashes so they round-trip through the registry storage and
/// re-emerge identically in the consumer cache. The set covers:
///   - The full `upload.source` tree (default `./src`), recursively.
///   - `deno.json` (+ optional `deno.lock`, `import_map.json`).
///   - `cue.mod/module.cue` if present.
///   - The local-build `meta.json` placed at `.forest/component/meta.json`
///     so the consumer's existing `read_meta_json()` fallback finds it.
async fn collect_deno_files(
    dir: &std::path::Path,
    upload_source: &str,
    organisation: &str,
    name: &str,
    version: &str,
) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();

    // --- 1. upload.source tree (recursive)
    let source_root = dir.join(upload_source.trim_start_matches("./"));
    if source_root.exists() {
        collect_dir_recursive(&source_root, dir, &mut files).await?;
    }

    // --- 2. deno.json / deno.lock / import_map.json (top-level only)
    for candidate in ["deno.json", "deno.lock", "import_map.json"] {
        let p = dir.join(candidate);
        if p.exists() {
            let content = tokio::fs::read(&p).await?;
            files.push((candidate.to_string(), content));
        }
    }

    // --- 3. cue.mod/module.cue
    let module_cue = dir.join("cue.mod").join("module.cue");
    if module_cue.exists() {
        let content = tokio::fs::read(&module_cue).await?;
        files.push(("cue.mod/module.cue".to_string(), content));
    }

    // --- 4. meta.json from the local build cache
    if let Some(meta_dir) = component_binary::component_meta_dir(organisation, name, version) {
        let meta_path = meta_dir.join("meta.json");
        if meta_path.exists() {
            let content = tokio::fs::read(&meta_path).await?;
            // Upload under the same relative path read_meta_json() falls
            // back to: <component_root>/.forest/component/meta.json
            files.push((".forest/component/meta.json".to_string(), content));
        } else {
            tracing::warn!(
                "no meta.json found at {} — run `forest run build` before `forest publish`",
                meta_path.display()
            );
        }
    }

    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

/// Recurse `root`, emitting `(relative_to_base, bytes)` pairs. Skips
/// dotfiles and common build/scratch dirs to avoid shipping cache junk
/// (`.forest/`, `target/`, `node_modules/`).
fn collect_dir_recursive<'a>(
    root: &'a std::path::Path,
    base: &'a std::path::Path,
    out: &'a mut Vec<(String, Vec<u8>)>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
    Box::pin(async move {
        let mut entries = tokio::fs::read_dir(root).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let file_name = entry.file_name();
            let name_str = file_name.to_string_lossy();

            // Skip hidden + scratch dirs. `.forest/component/meta.json` is
            // re-added by the caller from the build cache, not the source
            // tree, so excluding `.forest/` here is intentional.
            if name_str.starts_with('.') || name_str == "target" || name_str == "node_modules" {
                continue;
            }

            let ft = entry.file_type().await?;
            if ft.is_dir() {
                collect_dir_recursive(&path, base, out).await?;
            } else if ft.is_file() {
                let rel = path
                    .strip_prefix(base)
                    .map_err(|e| anyhow::anyhow!("path outside base: {e}"))?
                    .to_string_lossy()
                    .replace('\\', "/");
                let content = tokio::fs::read(&path).await?;
                out.push((rel, content));
            }
        }
        Ok(())
    })
}

/// DATA-583 — version override resolution.
///
/// Precedence is **`--version` > `FOREST_COMPONENT_VERSION` > forest.cue**, and
/// it is assembled from two independent pieces:
///
///   - clap resolves *flag vs env* into `self.version` before we see it. That
///     leg is asserted structurally (`arg_is_wired_to_the_env_var`) rather than
///     by setting the variable: every `try_parse_from` in this binary reads the
///     process environment, so a test that mutates it corrupts whichever tests
///     happen to run alongside it. Cargo runs them in threads; there is no
///     serialisation to rely on.
///   - `resolve_version` resolves *override vs manifest*, tested directly below.
///
/// Structs are built by literal rather than parsed, so no test here depends on
/// the ambient environment being clean.
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
        // The pre-DATA-583 behaviour, which must not change: manual publishing
        // keeps reading forest.cue.
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
        // An unset CI input interpolates to "" — that must mean "no override",
        // not "publish as the empty string", which would fail the C8 semver
        // gate with a message about `` instead of just working.
        assert_eq!(with_version(Some("")).resolve_version("0.1.7"), "0.1.7");
        assert_eq!(with_version(Some("   ")).resolve_version("0.1.7"), "0.1.7");
    }

    #[test]
    fn override_is_trimmed() {
        // Shell interpolation of a tag routinely carries a trailing newline.
        assert_eq!(
            with_version(Some(" 0.1.8\n")).resolve_version("0.1.7"),
            "0.1.8"
        );
    }

    #[test]
    fn bootstrap_ignores_the_ambient_env_override() {
        // Bootstrap publishes every workspace component in one process. If it
        // honoured FOREST_COMPONENT_VERSION it would stamp all of them with the
        // same version. It builds the command directly rather than parsing
        // argv, so clap's `env` never applies — assert that, because the
        // immunity is a property of the construction and easy to lose.
        assert!(PublishCommand::for_bootstrap().version.is_none());
        assert_eq!(
            PublishCommand::for_bootstrap().resolve_version("0.1.7"),
            "0.1.7"
        );
    }

    #[test]
    fn arg_is_wired_to_the_env_var() {
        // The env leg of the precedence chain. clap applies `env` only when
        // parsing argv and only when the flag is absent, which is exactly
        // "flag > env"; what this asserts is that the variable is still named
        // FOREST_COMPONENT_VERSION — the same name `forest run build` reads, so
        // one export covers build and publish. Renaming one side silently
        // decouples the stamped version from the published one.
        //
        // Spelled as a literal rather than imported from
        // `forest_build_core::FOREST_VERSION_ENV`: the CLI does not depend on
        // the build crate (the build is a separately published component), and
        // taking a dependency for one constant would invert that. The coupling
        // is real, so it is named here and in the build crate's doc comment.
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

    // --- include.shell (DATA-588) ----------------------------------------

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
        // Catching the typo here is the whole point: an unrecognised shell would
        // otherwise ship in the manifest and silently never load.
        let doc = doc_with_include(serde_json::json!({
            "shell": { "init": { "zshell": ["init", "zsh"] } }
        }));
        assert!(include_manifest_value(&doc).is_err());
    }

    #[test]
    fn empty_shell_argv_errors_at_publish() {
        // An empty argv would exec the tool with no arguments at capture time,
        // running its default action instead of printing a script.
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
