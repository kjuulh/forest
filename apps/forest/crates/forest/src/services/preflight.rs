use std::path::{Path, PathBuf};

pub struct PreflightContext {
    pub current_dir: PathBuf,
    pub doc: serde_json::Value,
    pub organisation: String,
    pub component_name: String,
    pub version: String,
}

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

pub fn standard_checks() -> Vec<Box<dyn PreflightCheck>> {
    vec![
        Box::new(C3NamesAgree),
        Box::new(C6BinaryArtifactExists),
        Box::new(C8SemverValid),
    ]
}

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
        failures.sort_by(|a, b| a.id.cmp(b.id));
        Err(failures)
    }
}

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

pub struct C6BinaryArtifactExists;
impl PreflightCheck for C6BinaryArtifactExists {
    fn id(&self) -> &'static str {
        "C6"
    }
    fn run(&self, ctx: &PreflightContext) -> Result<(), CheckFailure> {
        let upload_type = ctx
            .doc
            .pointer("/forest/component/upload/type")
            .and_then(|v| v.as_str());
        if matches!(upload_type, Some("deno") | Some("external")) {
            return Ok(());
        }

        let architectures = ctx
            .doc
            .pointer("/forest/component/upload/architectures")
            .and_then(|v| v.as_object());
        let declares_binary = architectures.map(|o| !o.is_empty()).unwrap_or(false);
        if !declares_binary {
            return Ok(());
        }

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
        let doc = serde_json::json!({
            "forest": { "component": { "name": "widget" } },
        });
        assert!(C3NamesAgree.run(&ctx_with(doc, "0.1.0")).is_ok());
    }

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
        let doc = serde_json::json!({
            "forest": { "component": { "name": "widget" } },
        });
        assert!(C6BinaryArtifactExists.run(&ctx_with(doc, "0.1.0")).is_ok());
    }

    #[test]
    fn c6_skips_when_architectures_is_empty() {
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
        assert!(err.hint.contains("build-tool agnostic"));
    }

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
        assert!(!standard_checks().is_empty());
    }
}
