//! Publish-readiness preflight (TASKS/028).
//!
//! `forest publish` runs every check here as a Phase 1 gate before any
//! RPC happens. `forest validate` runs the same set so the standalone
//! command stops returning a misleading "Validated 0 component(s)" on
//! projects whose names disagree across files.
//!
//! Each check is a small `PreflightCheck` impl that takes a fully
//! evaluated cue doc plus the resolved component identity and returns
//! either `Ok(())` or a `CheckFailure` with a stable ID + actionable
//! hint. The runner collects every failure (no short-circuit) so the
//! user sees the full list on a single invocation instead of fixing
//! issues one-at-a-time.
//!
//! Forest is intentionally agnostic to the build toolchain — checks
//! inspect what the component DECLARED in CUE and what artifacts are
//! on disk, never the build system that produced them. A Rust crate, a
//! Go module, a `.exe` from MSBuild, and a hand-written `chmod +x`
//! shell script all flow through the same gates.
//!
//! Phase scope today:
//! - **Pre-build (this module):** C3 names-agree, C6 binary artifact
//!   exists when the component declared a binary upload, C8 version is
//!   valid semver. Run on every publish + validate.
//! - **Post-build (deferred):** C7 `_meta/describe` produces a valid
//!   descriptor — already enforced in-line by the publish flow; will
//!   move here once a follow-up slice splits Phase 1 / Phase 2.
//! - **Network (deferred):** C4 registry owner check, C9 version
//!   already-published, C12 auth context reachable. Need either an
//!   ambient gRPC client in the context or a separate async-aware
//!   runner. Tracked for the slice after this one.

use std::path::{Path, PathBuf};

/// Inputs available to every check. Fully populated by the caller before
/// the runner spins; checks are pure functions of this struct.
pub struct PreflightContext {
    pub current_dir: PathBuf,
    pub doc: serde_json::Value,
    pub organisation: String,
    pub component_name: String,
    pub version: String,
}

/// A single check verdict. `id` is stable across releases — users may
/// `grep` it from CI output and the future `--explain <id>` command
/// looks it up here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckFailure {
    pub id: &'static str,
    pub message: String,
    pub hint: String,
}

pub trait PreflightCheck: Sync + Send {
    fn id(&self) -> &'static str;
    fn run(&self, ctx: &PreflightContext) -> Result<(), CheckFailure>;
}

/// The default check set. Adding a new check is one new impl block plus
/// an entry here; the framework is intentionally not configurable from
/// outside the source so reviewers can spot new rules in diffs.
pub fn standard_checks() -> Vec<Box<dyn PreflightCheck>> {
    vec![
        Box::new(C3NamesAgree),
        Box::new(C6BinaryArtifactExists),
        Box::new(C8SemverValid),
    ]
}

/// Run every check, collect every failure, and return them all together.
/// Critical property: a check that errors does NOT abort the runner —
/// the user gets the full punch list.
pub fn run_checks(
    ctx: &PreflightContext,
    checks: &[Box<dyn PreflightCheck>],
) -> Result<(), Vec<CheckFailure>> {
    let mut failures = Vec::new();
    for check in checks {
        if let Err(f) = check.run(ctx) {
            failures.push(f);
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        // Stable sort by ID so output is deterministic across runs.
        failures.sort_by(|a, b| a.id.cmp(b.id));
        Err(failures)
    }
}

/// User-facing rendering. Each failure becomes a multi-line block with
/// the ID, the violation, and a fix hint.
pub fn render_failures(failures: &[CheckFailure]) -> String {
    let mut out = format!(
        "forest publish: preflight failed ({} issue{})\n\n",
        failures.len(),
        if failures.len() == 1 { "" } else { "s" }
    );
    for f in failures {
        out.push_str(&format!("  {}: {}\n", f.id, f.message));
        for line in f.hint.lines() {
            out.push_str(&format!("       {line}\n"));
        }
        out.push('\n');
    }
    out
}

// ============================================================
// Individual checks
// ============================================================

/// C3 — `project.name` agrees with `forest.component.name`.
///
/// Cue itself rejects conflicting concrete values for the same path,
/// so a hard mismatch fails earlier. This check catches the subtler
/// case where the two paths are independent (project.name in
/// forest.cue, component.name in forest.component.cue) and the user
/// has set them to different values by accident.
pub struct C3NamesAgree;
impl PreflightCheck for C3NamesAgree {
    fn id(&self) -> &'static str {
        "C3"
    }
    fn run(&self, ctx: &PreflightContext) -> Result<(), CheckFailure> {
        let project_name = ctx.doc.pointer("/project/name").and_then(|v| v.as_str());
        let component_name = ctx
            .doc
            .pointer("/forest/component/name")
            .and_then(|v| v.as_str());

        match (project_name, component_name) {
            (Some(p), Some(c)) if p != c => Err(CheckFailure {
                id: "C3",
                message: format!("project.name (`{p}`) and forest.component.name (`{c}`) disagree"),
                hint: "Set them to the same value. The convention is one project = one \
                       component; if you genuinely need them different, this check is the \
                       wrong abstraction — file a follow-up."
                    .into(),
            }),
            _ => Ok(()),
        }
    }
}

/// C6 — when the component DECLARED a binary upload, a built artifact
/// for the current platform must exist on disk.
///
/// Language-agnostic. The check looks at the project's declared shape
/// (`forest.component.upload.architectures` is non-empty → the project
/// promised a binary) and verifies the artifact was actually produced
/// by whatever build tooling the user runs (cargo, go build,
/// dotnet publish, make, a shell script — forest doesn't care).
///
/// This closes the original debrief bug: a binary-shaped publish where
/// the build silently produced nothing usable would, before this check,
/// fall through to a CUE-only `[files]` shape and ship a ghost version.
/// Now it refuses with a clear "you promised a binary; none was built"
/// error.
///
/// Skipped when the component declares it's NOT a binary upload (CUE
/// libraries, Deno-source components, external URL-hosted tools, etc).
pub struct C6BinaryArtifactExists;
impl PreflightCheck for C6BinaryArtifactExists {
    fn id(&self) -> &'static str {
        "C6"
    }
    fn run(&self, ctx: &PreflightContext) -> Result<(), CheckFailure> {
        // Non-binary upload types explicitly opt out of this check.
        let upload_type = ctx
            .doc
            .pointer("/forest/component/upload/type")
            .and_then(|v| v.as_str());
        if matches!(upload_type, Some("deno") | Some("external")) {
            return Ok(());
        }

        // The "I produce a binary" signal: at least one architecture
        // declared under upload.architectures. Projects that don't
        // populate this are CUE-only / source-only and don't owe us
        // an artifact.
        let architectures = ctx
            .doc
            .pointer("/forest/component/upload/architectures")
            .and_then(|v| v.as_object());
        let declares_binary = architectures.map(|o| !o.is_empty()).unwrap_or(false);
        if !declares_binary {
            return Ok(());
        }

        // The project promised a binary. This must apply *exactly* the
        // resolution the publish flow uses, or the gate is worse than
        // useless: DATA-654 — with `resolve_binary` here, a stray
        // `target/debug/<name>` satisfied C6 while publish (which only
        // reads `.forest/component/output/`) found nothing, so the
        // publish fell through to a CUE-only shape and shipped the ghost
        // version this check exists to prevent.
        if crate::services::component_binary::resolve_publishable_binary(
            &ctx.current_dir,
            &ctx.component_name,
            Some(&ctx.organisation),
            Some(&ctx.component_name),
            Some(&ctx.version),
        )
        .is_none()
        {
            return Err(CheckFailure {
                id: "C6",
                message: format!(
                    "forest.component declared a binary upload but no built artifact \
                     for `{}` was staged in `.forest/component/output/`",
                    ctx.component_name
                ),
                hint: "Run your build first (e.g. `forest run build`, or your build tool \
                       directly). The artifact must land where forest publishes from: \
                       `.forest/component/output/<os>/<arch>/<name>`, named after the \
                       component. Forest is build-tool agnostic — whatever you use \
                       (cargo, go build, dotnet publish, make, shell script), the \
                       contract is the same: stage the artifact at that path. A binary \
                       sitting in a cargo `target/debug` or `target/release` directory \
                       is deliberately not used — publishing those shipped stale debug \
                       builds. If your build succeeded but forest still can't find it, \
                       the artifact's name doesn't match the component's name."
                    .into(),
            });
        }
        Ok(())
    }
}

/// C8 — the declared version parses as semver.
pub struct C8SemverValid;
impl PreflightCheck for C8SemverValid {
    fn id(&self) -> &'static str {
        "C8"
    }
    fn run(&self, ctx: &PreflightContext) -> Result<(), CheckFailure> {
        if semver::Version::parse(&ctx.version).is_err() {
            return Err(CheckFailure {
                id: "C8",
                message: format!("version `{}` is not valid semver", ctx.version),
                hint: "Use MAJOR.MINOR.PATCH (with optional -prerelease and +build). \
                       Examples: 0.1.0, 1.2.3, 1.0.0-alpha.1."
                    .into(),
            });
        }
        Ok(())
    }
}

// ============================================================
// Helpers for `forest publish` / `forest validate`
// ============================================================

/// Build a [`PreflightContext`] by re-evaluating the cwd's CUE files.
///
/// Kept in this module so both publish and validate share one call path.
/// The cue invocation mirrors what publish does today verbatim, so any
/// project that publishes successfully can also be preflighted.
pub async fn build_context(current_dir: &Path) -> anyhow::Result<PreflightContext> {
    let mut cue_args = vec![
        "export".to_string(),
        "--out".to_string(),
        "json".to_string(),
    ];
    let mut entries = tokio::fs::read_dir(current_dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        if entry.path().extension().and_then(|e| e.to_str()) == Some("cue") {
            cue_args.push(entry.file_name().to_string_lossy().to_string());
        }
    }

    let output = crate::tools::cue::output(|| {
        let mut cmd = tokio::process::Command::new("cue");
        cmd.current_dir(current_dir).args(&cue_args);
        if let Ok(registry) = std::env::var("CUE_REGISTRY") {
            cmd.env("CUE_REGISTRY", registry);
        }
        cmd
    })
    .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("preflight: failed to evaluate cue: {stderr}");
    }

    let doc: serde_json::Value = serde_json::from_slice(&output.stdout)?;

    let component = doc
        .pointer("/forest/component")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let project = doc
        .pointer("/project")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    let component_name = component
        .get("name")
        .and_then(|v| v.as_str())
        .or_else(|| project.get("name").and_then(|v| v.as_str()))
        .ok_or_else(|| anyhow::anyhow!("preflight: component or project name is required"))?
        .to_string();

    let version = component
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("0.1.0")
        .to_string();

    let organisation = project
        .get("organisation")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("preflight: project.organisation is required"))?
        .to_string();

    Ok(PreflightContext {
        current_dir: current_dir.to_path_buf(),
        doc,
        organisation,
        component_name,
        version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ctx_with(doc: serde_json::Value, version: &str) -> PreflightContext {
        PreflightContext {
            current_dir: PathBuf::from("/tmp/non-existent-for-test"),
            doc,
            organisation: "acme".into(),
            component_name: "widget".into(),
            version: version.into(),
        }
    }

    // --- C3 -------------------------------------------------------

    #[test]
    fn c3_passes_when_names_agree() {
        let doc = serde_json::json!({
            "project": { "name": "widget" },
            "forest": { "component": { "name": "widget" } },
        });
        assert!(C3NamesAgree.run(&ctx_with(doc, "0.1.0")).is_ok());
    }

    #[test]
    fn c3_fails_when_names_disagree() {
        let doc = serde_json::json!({
            "project": { "name": "canopy-data-cli" },
            "forest": { "component": { "name": "data" } },
        });
        let err = C3NamesAgree.run(&ctx_with(doc, "0.1.0")).unwrap_err();
        assert_eq!(err.id, "C3");
        assert!(err.message.contains("canopy-data-cli"));
        assert!(err.message.contains("data"));
    }

    #[test]
    fn c3_passes_when_only_one_is_set() {
        // CUE-only library declares no project.name — that's not a mismatch.
        let doc = serde_json::json!({
            "forest": { "component": { "name": "widget" } },
        });
        assert!(C3NamesAgree.run(&ctx_with(doc, "0.1.0")).is_ok());
    }

    // --- C6 -------------------------------------------------------

    #[test]
    fn c6_skips_for_explicit_external() {
        let doc = serde_json::json!({
            "forest": {
                "component": {
                    "name": "widget",
                    "upload": { "type": "external" }
                }
            }
        });
        assert!(C6BinaryArtifactExists.run(&ctx_with(doc, "0.1.0")).is_ok());
    }

    #[test]
    fn c6_skips_for_deno() {
        let doc = serde_json::json!({
            "forest": {
                "component": {
                    "name": "widget",
                    "upload": { "type": "deno" }
                }
            }
        });
        assert!(C6BinaryArtifactExists.run(&ctx_with(doc, "0.1.0")).is_ok());
    }

    #[test]
    fn c6_skips_when_no_binary_declared() {
        // CUE-only library: no upload.architectures map, no upload.type
        // either. Forest doesn't owe the project a binary, so skip.
        let doc = serde_json::json!({
            "forest": { "component": { "name": "widget" } },
        });
        assert!(C6BinaryArtifactExists.run(&ctx_with(doc, "0.1.0")).is_ok());
    }

    #[test]
    fn c6_skips_when_architectures_is_empty() {
        // Defensive: an explicitly empty architectures map is the same
        // signal as "no binary upload" — don't demand an artifact.
        let doc = serde_json::json!({
            "forest": {
                "component": {
                    "name": "widget",
                    "upload": { "architectures": {} }
                }
            }
        });
        assert!(C6BinaryArtifactExists.run(&ctx_with(doc, "0.1.0")).is_ok());
    }

    #[test]
    fn c6_fails_when_binary_declared_but_missing() {
        // Project promises a binary for darwin/arm64. The test ctx
        // points at a tmpdir with no built artifact anywhere, so
        // resolve_binary returns None — failure expected.
        let doc = serde_json::json!({
            "forest": {
                "component": {
                    "name": "widget-that-cannot-exist-on-disk",
                    "upload": {
                        "architectures": {
                            "macos": { "arm64": {} }
                        }
                    }
                }
            }
        });
        let mut ctx = ctx_with(doc, "0.1.0");
        ctx.component_name = "widget-that-cannot-exist-on-disk".into();
        let err = C6BinaryArtifactExists.run(&ctx).unwrap_err();
        assert_eq!(err.id, "C6");
        assert!(
            err.message.contains("widget-that-cannot-exist-on-disk"),
            "expected component name in message: {}",
            err.message
        );
        // The hint must NOT prescribe a specific build tool — the
        // language-agnostic message lists several as examples but
        // doesn't single out one.
        assert!(err.hint.contains("build-tool agnostic"));
    }

    /// DATA-654. C6 used to call `resolve_binary`, which walks up to the
    /// cargo workspace root and probes `target/debug/<name>`. A stale debug
    /// build sitting there satisfied the gate, publish then found nothing in
    /// `.forest/component/output/` and — before this change — uploaded the
    /// decoy; after it, publish would refuse *after* the preflight had passed.
    /// Either way C6 has to be the one that says no, and say why.
    #[test]
    fn c6_is_not_satisfied_by_a_cargo_target_binary() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        let decoy_dir = tmp.path().join("target/debug");
        std::fs::create_dir_all(&decoy_dir).unwrap();
        std::fs::write(decoy_dir.join("widget"), b"stale debug build").unwrap();

        let component_dir = tmp.path().join("components/widget");
        std::fs::create_dir_all(&component_dir).unwrap();

        let doc = serde_json::json!({
            "forest": {
                "component": {
                    "name": "widget",
                    "upload": {
                        "architectures": { "macos": { "arm64": {} } }
                    }
                }
            }
        });
        let mut ctx = ctx_with(doc, "0.1.0");
        ctx.current_dir = component_dir;

        let err = C6BinaryArtifactExists
            .run(&ctx)
            .expect_err("a target/ binary must not satisfy the binary-artifact gate");
        assert_eq!(err.id, "C6");
        assert!(
            err.message.contains(".forest/component/output/"),
            "the failure should name where the artifact belongs: {}",
            err.message
        );
    }

    // --- C8 -------------------------------------------------------

    #[test]
    fn c8_accepts_basic_semver() {
        let doc = serde_json::json!({});
        for v in &["0.0.1", "0.1.0", "1.2.3", "10.20.30"] {
            assert!(C8SemverValid.run(&ctx_with(doc.clone(), v)).is_ok(), "{v}");
        }
    }

    #[test]
    fn c8_accepts_prerelease_and_build_metadata() {
        let doc = serde_json::json!({});
        for v in &[
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0+build.123",
            "1.0.0-rc.1+build.5",
        ] {
            assert!(C8SemverValid.run(&ctx_with(doc.clone(), v)).is_ok(), "{v}");
        }
    }

    #[test]
    fn c8_rejects_non_semver() {
        let doc = serde_json::json!({});
        for v in &["", "1", "1.0", "v1.0.0", "1.0.0.0", "latest", "foo"] {
            let result = C8SemverValid.run(&ctx_with(doc.clone(), v));
            assert!(result.is_err(), "should reject `{v}`");
            let err = result.unwrap_err();
            assert_eq!(err.id, "C8");
            assert!(err.message.contains(v) || v.is_empty());
        }
    }

    // --- Runner ---------------------------------------------------

    /// Test-only check that always fails with a configurable id.
    struct AlwaysFail(&'static str);
    impl PreflightCheck for AlwaysFail {
        fn id(&self) -> &'static str {
            self.0
        }
        fn run(&self, _: &PreflightContext) -> Result<(), CheckFailure> {
            Err(CheckFailure {
                id: self.0,
                message: format!("{} failed", self.0),
                hint: "test".into(),
            })
        }
    }

    /// Test-only check that always passes.
    struct AlwaysPass(&'static str);
    impl PreflightCheck for AlwaysPass {
        fn id(&self) -> &'static str {
            self.0
        }
        fn run(&self, _: &PreflightContext) -> Result<(), CheckFailure> {
            Ok(())
        }
    }

    fn empty_ctx() -> PreflightContext {
        ctx_with(serde_json::json!({}), "0.1.0")
    }

    #[test]
    fn runner_collects_every_failure_no_short_circuit() {
        let checks: Vec<Box<dyn PreflightCheck>> = vec![
            Box::new(AlwaysFail("X1")),
            Box::new(AlwaysPass("X2")),
            Box::new(AlwaysFail("X3")),
            Box::new(AlwaysFail("X4")),
        ];
        let err = run_checks(&empty_ctx(), &checks).unwrap_err();
        assert_eq!(err.len(), 3);
        let ids: Vec<&str> = err.iter().map(|f| f.id).collect();
        // Sorted by ID — stable across runs.
        assert_eq!(ids, vec!["X1", "X3", "X4"]);
    }

    #[test]
    fn runner_returns_ok_when_all_pass() {
        let checks: Vec<Box<dyn PreflightCheck>> =
            vec![Box::new(AlwaysPass("A")), Box::new(AlwaysPass("B"))];
        assert!(run_checks(&empty_ctx(), &checks).is_ok());
    }

    #[test]
    fn runner_returns_ok_for_empty_check_list() {
        let checks: Vec<Box<dyn PreflightCheck>> = vec![];
        assert!(run_checks(&empty_ctx(), &checks).is_ok());
    }

    #[test]
    fn render_includes_id_message_and_hint_per_failure() {
        let failures = vec![
            CheckFailure {
                id: "C3",
                message: "names disagree".into(),
                hint: "set them to the same value".into(),
            },
            CheckFailure {
                id: "C8",
                message: "bad semver".into(),
                hint: "use MAJOR.MINOR.PATCH".into(),
            },
        ];
        let rendered = render_failures(&failures);
        assert!(rendered.contains("2 issues"));
        assert!(rendered.contains("C3:"));
        assert!(rendered.contains("names disagree"));
        assert!(rendered.contains("set them to the same value"));
        assert!(rendered.contains("C8:"));
        assert!(rendered.contains("bad semver"));
    }

    #[test]
    fn render_singular_on_one_failure() {
        let failures = vec![CheckFailure {
            id: "C3",
            message: "x".into(),
            hint: "y".into(),
        }];
        let rendered = render_failures(&failures);
        assert!(
            rendered.contains("1 issue)"),
            "expected '1 issue)' in: {rendered}"
        );
        assert!(!rendered.contains("1 issues"));
    }

    #[test]
    fn standard_check_set_is_non_empty() {
        // Guard against accidental removal of all standard checks in
        // a future refactor.
        assert!(!standard_checks().is_empty());
    }
}
