#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, process::Command};

use sha2::{Digest, Sha256};

struct Project {
    root: tempfile::TempDir,
    binary: PathBuf,
}

impl Project {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        for dir in ["cache", "config", "data", "tmp", "app", "home"] {
            fs::create_dir(root.path().join(dir)).unwrap();
        }
        let validator = br##"#!/bin/sh
case "$1" in
_meta/describe)
    printf '%s\n' '{"protocol_version":"2","methods":[{"name":"commands/validate","kind":"command"}]}' ;;
commands/validate)
    case "$VALIDATOR_MODE" in
    malformed) cat >/dev/null; printf '{}\n' ;;
    rejected) cat >/dev/null; printf '{"valid":false}\n' ;;
    unavailable) exit 1 ;;
    *)
        if cue vet -c -d '#Request' "$VALIDATION_SCHEMA" json: - >/dev/null 2>&1; then
            printf '{"valid":true}\n'
        else
            printf '{"valid":false,"errors":["replicas must be a positive integer"]}\n'
        fi ;;
    esac ;;
*) exit 1 ;;
esac
"##;
        let hash = hex::encode(Sha256::digest(validator));
        let binary = root
            .path()
            .join(format!("cache/forest/components/bin/{}/{hash}", &hash[..2]));
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, validator).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            root.path().join("schema.cue"),
            "#Request: {spec: {replicas: int & >0}, input: {}}\n",
        )
        .unwrap();
        let fixture = Self { root, binary };
        fixture.component("owned", "service");
        let metadata = fixture
            .root
            .path()
            .join("cache/forest/component-meta/owned/service/1.0.0");
        fs::create_dir_all(&metadata).unwrap();
        let os = std::env::consts::OS;
        let arch = match std::env::consts::ARCH {
            "x86_64" => "amd64",
            "aarch64" => "arm64",
            other => other,
        };
        fs::write(metadata.join("meta.json"), serde_json::to_vec(&serde_json::json!({
            "platforms": {format!("{os}_{arch}"): {"sha256": hash}},
            "descriptor": {"protocol_version":"2","methods":[{"name":"commands/validate","kind":"command"}]}
        })).unwrap()).unwrap();
        fixture
    }

    fn component(&self, org: &str, name: &str) {
        let path = self
            .root
            .path()
            .join(format!("cache/forest/components/{org}/{name}/1.0.0"));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("forest.component.cue"), "package fixture\n").unwrap();
        fs::write(path.join("forest.cue"), format!(
            "package fixture\nproject: organisation: \"{org}\"\nforest: component: {{name: \"{name}\", version: \"1.0.0\"}}\n"
        )).unwrap();
    }

    fn config(&self, dependencies: &str, usage: &str) {
        fs::write(self.root.path().join("app/forest.cue"), format!(
            "package app\nproject: {{name: \"app\", organisation: \"owned\"}}\ndependencies: {{{dependencies}}}\n{usage}\n"
        )).unwrap();
    }

    fn validate(&self, mode: &str, expected_success: bool) {
        let output = Command::new(env!("CARGO_BIN_EXE_forest"))
            .args(["--forest-server", "http://127.0.0.1:1", "validate"])
            .current_dir(self.root.path().join("app"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", self.root.path().join("home"))
            .env("XDG_CACHE_HOME", self.root.path().join("cache"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("TMPDIR", self.root.path().join("tmp"))
            .env("VALIDATION_SCHEMA", self.root.path().join("schema.cue"))
            .env("VALIDATOR_MODE", mode)
            .env("CI", "true")
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            expected_success,
            "mode={mode}, status={}\nstdout={}\nstderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn pinned_registry_configs_are_checked_against_the_component_schema() {
    let project = Project::new();
    for (replicas, valid) in [("2", true), ("0", false), ("\"two\"", false)] {
        project.config(
            "\"owned/service\": version: \"1.0.0\"",
            &format!("owned: service: config: replicas: {replicas}"),
        );
        project.validate("schema", valid);
    }
}

#[test]
fn requested_validation_fails_when_the_validator_cannot_confirm_validity() {
    let project = Project::new();
    project.config(
        "\"owned/service\": version: \"1.0.0\"",
        "owned: service: config: replicas: 2",
    );
    for mode in ["malformed", "rejected", "unavailable"] {
        project.validate(mode, false);
    }
    fs::remove_file(&project.binary).unwrap();
    project.validate("schema", false);
}

#[test]
fn contract_only_and_no_component_projects_do_not_require_a_validator() {
    let project = Project::new();
    project.component("forest", "deployment");
    project.config("\"forest/deployment\": version: \"1.0.0\"", "");
    project.validate("schema", true);
    project.config("", "");
    project.validate("schema", true);
}

#[test]
fn used_components_without_config_or_a_component_contract_fail_validation() {
    let project = Project::new();
    project.config(
        "\"owned/service\": version: \"1.0.0\"",
        "owned: service: env: {}",
    );
    project.validate("schema", false);
    fs::remove_file(
        project
            .root
            .path()
            .join("cache/forest/components/owned/service/1.0.0/forest.component.cue"),
    )
    .unwrap();
    fs::write(
        project
            .root
            .path()
            .join("cache/forest/components/owned/service/1.0.0/forest.component.toml"),
        "[component]\nname = \"service\"\norganisation = \"owned\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    project.config(
        "\"owned/service\": version: \"1.0.0\"",
        "owned: service: config: replicas: 2",
    );
    project.validate("schema", false);
}

#[test]
fn local_component_configs_still_use_the_component_validator() {
    let project = Project::new();
    let component = project
        .root
        .path()
        .join("cache/forest/components/owned/service/1.0.0");
    fs::create_dir_all(component.join(".forest/component")).unwrap();
    fs::copy(
        project
            .root
            .path()
            .join("cache/forest/component-meta/owned/service/1.0.0/meta.json"),
        component.join(".forest/component/meta.json"),
    )
    .unwrap();
    for (replicas, valid) in [(2, true), (0, false)] {
        project.config(
            &format!("\"owned/service\": path: \"{}\"", component.display()),
            &format!("owned: service: config: replicas: {replicas}"),
        );
        project.validate("schema", valid);
    }
}
