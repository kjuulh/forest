use crate::global::{
    lockfile::GlobalLockFile,
    manifest::{Archive, ComponentShape, Manifest, ManifestKind, PlatformKey},
    shim::QualifiedRef,
    user_config::UserConfig,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    Resolve {
        expected_sha: String,
        fetch_if_missing: FetchPlan,
    },
    Error(PlanError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchPlan {
    Registry,
    Url {
        url: String,
        archive: Archive,
        binary_in_archive: Option<String>,
        archive_sha: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    PlatformNotAvailable {
        requested: PlatformKey,
        available: Vec<PlatformKey>,
    },
    ShapeNotInstallable {
        shape: ComponentShape,
    },
}

pub fn plan(
    _user_config: &UserConfig,
    lockfile: &GlobalLockFile,
    manifest: &Manifest,
    qref: &QualifiedRef,
    version: &str,
    platform: PlatformKey,
) -> Plan {
    let platform_entry = match manifest.platforms.get(&platform) {
        Some(p) => p,
        None => {
            return Plan::Error(PlanError::PlatformNotAvailable {
                requested: platform,
                available: manifest.platforms.keys().copied().collect(),
            });
        }
    };

    let installable = matches!(
        manifest.shape,
        ComponentShape::ToolBinary | ComponentShape::HybridComponent | ComponentShape::ToolExternal
    );
    if !installable {
        return Plan::Error(PlanError::ShapeNotInstallable {
            shape: manifest.shape,
        });
    }

    let expected_sha = match lockfile.get(
        &qref.organisation,
        &qref.name,
        version,
        platform_os_str(platform.os),
        platform_arch_str(platform.arch),
    ) {
        Some(pinned) => pinned.to_string(),
        None => platform_entry.sha256.clone(),
    };

    let fetch_if_missing = match manifest.kind {
        ManifestKind::Binary => FetchPlan::Registry,
        ManifestKind::External => FetchPlan::Url {
            url: platform_entry
                .url
                .clone()
                .expect("manifest::parse guarantees external platforms carry a url"),
            archive: platform_entry.archive,
            binary_in_archive: platform_entry.binary_in_archive.clone(),
            archive_sha: platform_entry.archive_sha256.clone(),
        },
    };

    Plan::Resolve {
        expected_sha,
        fetch_if_missing,
    }
}

fn platform_os_str(os: crate::global::manifest::Os) -> &'static str {
    use crate::global::manifest::Os;
    match os {
        Os::Linux => "linux",
        Os::Darwin => "darwin",
    }
}

fn platform_arch_str(arch: crate::global::manifest::Arch) -> &'static str {
    use crate::global::manifest::Arch;
    match arch {
        Arch::Amd64 => "amd64",
        Arch::Arm64 => "arm64",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::global::lockfile::GlobalLockEntry;
    use crate::global::manifest::{Arch, ManifestKind, Os, Platform, PlatformKey, ToolFacet};
    use std::collections::BTreeMap;

    fn linux_amd64() -> PlatformKey {
        PlatformKey {
            os: Os::Linux,
            arch: Arch::Amd64,
        }
    }

    fn darwin_arm64() -> PlatformKey {
        PlatformKey {
            os: Os::Darwin,
            arch: Arch::Arm64,
        }
    }

    fn binary_platform(sha: &str) -> Platform {
        Platform {
            sha256: sha.to_string(),
            size: None,
            url: None,
            archive: Archive::None,
            binary_in_archive: None,
            archive_sha256: None,
        }
    }

    fn external_platform_targz(sha: &str, url: &str, binary_in_archive: &str) -> Platform {
        Platform {
            sha256: sha.to_string(),
            size: None,
            url: Some(url.to_string()),
            archive: Archive::TarGz,
            binary_in_archive: Some(binary_in_archive.to_string()),
            archive_sha256: None,
        }
    }

    fn tool_binary_manifest(sha_linux: &str) -> Manifest {
        let mut platforms = BTreeMap::new();
        platforms.insert(linux_amd64(), binary_platform(sha_linux));
        Manifest {
            kind: ManifestKind::Binary,
            tool: Some(ToolFacet {
                name: "hello".into(),
                argv_passthrough: true,
                description: None,
            }),
            methods: vec![],
            platforms,
            include: Default::default(),
            shape: ComponentShape::ToolBinary,
        }
    }

    fn tool_external_manifest(sha_linux: &str) -> Manifest {
        let mut platforms = BTreeMap::new();
        platforms.insert(
            linux_amd64(),
            external_platform_targz(sha_linux, "https://github.com/example/x.tar.gz", "x/x"),
        );
        Manifest {
            kind: ManifestKind::External,
            tool: Some(ToolFacet {
                name: "rg".into(),
                argv_passthrough: true,
                description: None,
            }),
            methods: vec![],
            platforms,
            include: Default::default(),
            shape: ComponentShape::ToolExternal,
        }
    }

    fn pure_component_manifest() -> Manifest {
        let mut platforms = BTreeMap::new();
        platforms.insert(linux_amd64(), binary_platform("abc"));
        Manifest {
            kind: ManifestKind::Binary,
            tool: None,
            methods: vec!["status".into()],
            platforms,
            include: Default::default(),
            shape: ComponentShape::Component,
        }
    }

    #[test]
    fn plan_for_tool_binary_with_no_lockfile_pin_uses_manifest_sha() {
        let cfg = UserConfig::default();
        let lock = GlobalLockFile::default();
        let m = tool_binary_manifest("4f9c");
        let qref = QualifiedRef::new("cuteorg", "forest-hello");

        let p = plan(&cfg, &lock, &m, &qref, "0.1.0", linux_amd64());

        assert_eq!(
            p,
            Plan::Resolve {
                expected_sha: "4f9c".into(),
                fetch_if_missing: FetchPlan::Registry,
            }
        );
    }

    #[test]
    fn plan_for_tool_binary_with_lockfile_pin_uses_lockfile_sha() {
        let cfg = UserConfig::default();
        let mut lock = GlobalLockFile::default();
        lock.insert(GlobalLockEntry {
            organisation: "cuteorg".into(),
            name: "forest-hello".into(),
            version: "0.1.0".into(),
            os: "linux".into(),
            arch: "amd64".into(),
            sha256: "sha256:locked".into(),
        });
        let m = tool_binary_manifest("4f9c");
        let qref = QualifiedRef::new("cuteorg", "forest-hello");

        let p = plan(&cfg, &lock, &m, &qref, "0.1.0", linux_amd64());

        assert_eq!(
            p,
            Plan::Resolve {
                expected_sha: "sha256:locked".into(),
                fetch_if_missing: FetchPlan::Registry,
            }
        );
    }

    #[test]
    fn plan_for_tool_external_uses_url_fetch_plan() {
        let cfg = UserConfig::default();
        let lock = GlobalLockFile::default();
        let m = tool_external_manifest("ad3a");
        let qref = QualifiedRef::new("cuteorg", "ripgrep");

        let p = plan(&cfg, &lock, &m, &qref, "14.1.1", linux_amd64());

        match p {
            Plan::Resolve {
                expected_sha,
                fetch_if_missing,
            } => {
                assert_eq!(expected_sha, "ad3a");
                match fetch_if_missing {
                    FetchPlan::Url {
                        url,
                        archive,
                        binary_in_archive,
                        archive_sha,
                    } => {
                        assert_eq!(url, "https://github.com/example/x.tar.gz");
                        assert_eq!(archive, Archive::TarGz);
                        assert_eq!(binary_in_archive.as_deref(), Some("x/x"));
                        assert!(archive_sha.is_none());
                    }
                    other => panic!("expected FetchPlan::Url, got {other:?}"),
                }
            }
            other => panic!("expected Resolve, got {other:?}"),
        }
    }

    #[test]
    fn p11_binary_kind_never_produces_url_fetch() {
        let cfg = UserConfig::default();
        let lock = GlobalLockFile::default();
        let m = tool_binary_manifest("aa");
        let qref = QualifiedRef::new("o", "n");
        if let Plan::Resolve {
            fetch_if_missing, ..
        } = plan(&cfg, &lock, &m, &qref, "0.1.0", linux_amd64())
        {
            let is_url = matches!(fetch_if_missing, FetchPlan::Url { .. });
            assert!(!is_url, "binary kind must not yield Url fetch");
        }
    }

    #[test]
    fn p11_external_kind_never_produces_registry_fetch() {
        let cfg = UserConfig::default();
        let lock = GlobalLockFile::default();
        let m = tool_external_manifest("aa");
        let qref = QualifiedRef::new("o", "n");
        if let Plan::Resolve {
            fetch_if_missing, ..
        } = plan(&cfg, &lock, &m, &qref, "0.1.0", linux_amd64())
        {
            assert_ne!(fetch_if_missing, FetchPlan::Registry);
        }
    }

    #[test]
    fn errors_when_platform_not_available() {
        let cfg = UserConfig::default();
        let lock = GlobalLockFile::default();
        let m = tool_binary_manifest("aa");

        let p = plan(
            &cfg,
            &lock,
            &m,
            &QualifiedRef::new("o", "n"),
            "0.1.0",
            darwin_arm64(),
        );

        match p {
            Plan::Error(PlanError::PlatformNotAvailable {
                requested,
                available,
            }) => {
                assert_eq!(requested, darwin_arm64());
                assert_eq!(available, vec![linux_amd64()]);
            }
            other => panic!("expected PlatformNotAvailable, got {other:?}"),
        }
    }

    #[test]
    fn errors_when_pure_component_has_no_tool_facet() {
        let cfg = UserConfig::default();
        let lock = GlobalLockFile::default();
        let m = pure_component_manifest();

        let p = plan(
            &cfg,
            &lock,
            &m,
            &QualifiedRef::new("o", "n"),
            "0.1.0",
            linux_amd64(),
        );

        match p {
            Plan::Error(PlanError::ShapeNotInstallable { shape }) => {
                assert_eq!(shape, ComponentShape::Component);
            }
            other => panic!("expected ShapeNotInstallable, got {other:?}"),
        }
    }

    #[test]
    fn hybrid_component_is_installable() {
        let mut m = tool_binary_manifest("hyb");
        m.methods = vec!["greet".into()];
        m.shape = ComponentShape::HybridComponent;
        let cfg = UserConfig::default();
        let lock = GlobalLockFile::default();
        let qref = QualifiedRef::new("cuteorg", "forest-greet");

        let p = plan(&cfg, &lock, &m, &qref, "0.1.0", linux_amd64());
        let is_resolve = matches!(p, Plan::Resolve { .. });
        assert!(is_resolve, "hybrid must be installable");
    }

    #[test]
    fn plan_is_total_on_arbitrary_empty_manifest_platforms() {
        let mut m = tool_binary_manifest("x");
        m.platforms.clear();
        let p = plan(
            &UserConfig::default(),
            &GlobalLockFile::default(),
            &m,
            &QualifiedRef::new("o", "n"),
            "0.0.0",
            linux_amd64(),
        );
        let is_error = matches!(p, Plan::Error(PlanError::PlatformNotAvailable { .. }));
        assert!(is_error);
    }
}
