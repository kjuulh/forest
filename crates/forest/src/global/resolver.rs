//! Pure resolver.
//!
//! Given `(UserConfig, GlobalLockFile, Manifest, QualifiedRef, PlatformKey)`,
//! produce a [`Plan`] describing what the effectful shell must do to exec
//! the requested tool. Per TASKS/018-global-tools.md §1b.1, this function
//! is the centre of the pure core — no I/O, no time, no random, never panics.
//!
//! Property P11 (manifest `kind ↔ plan-variant` correspondence) is encoded
//! structurally: a `Binary` manifest never produces `FetchPlan::Url`, and
//! an `External` manifest never produces `FetchPlan::Registry`.

use crate::global::{
    lockfile::GlobalLockFile,
    manifest::{Archive, ComponentShape, Manifest, ManifestKind, PlatformKey},
    shim::QualifiedRef,
    user_config::UserConfig,
};

/// The resolver's output. Caller (effectful shell) reads `expected_sha`,
/// stats `cache_root/bin/<expected_sha>`; if present, `exec` it; if absent,
/// run `fetch_if_missing`, then `exec`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    Resolve {
        expected_sha: String,
        fetch_if_missing: FetchPlan,
    },
    Error(PlanError),
}

/// How to fetch a binary that is not in the local cache.
/// P11: this enum's discriminant is functionally derived from `manifest.kind`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchPlan {
    /// Stream the binary via `RegistryService.DownloadBinary` (kind=binary).
    Registry,
    /// HTTPS GET the URL, optionally extract from an archive (kind=external).
    Url {
        url: String,
        archive: Archive,
        binary_in_archive: Option<String>,
        archive_sha: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    /// The manifest declares no platform matching the caller's host.
    PlatformNotAvailable {
        requested: PlatformKey,
        available: Vec<PlatformKey>,
    },
    /// The manifest is a pure COMPONENT — no tool facet, no shim, no `forest
    /// global *` install path. `forest run <command>` is the only valid
    /// invocation for that shape.
    ShapeNotInstallable {
        shape: ComponentShape,
    },
}

/// Plan an invocation.
///
/// Inputs are taken by reference so the caller can keep ownership.
/// The function is total — every input combination returns a `Plan`.
///
/// Resolution rules (matching §1a.9 pseudocode):
///   1. If the manifest has no entry for `platform` → `PlatformNotAvailable`.
///   2. If the manifest's shape has no tool facet → `ShapeNotInstallable`.
///   3. `expected_sha` is the lockfile pin if present (the user has run this
///      before and we trust their pin), else the manifest's claim for this
///      `(org, name, version, platform)` tuple (first-run case).
///   4. `fetch_if_missing` follows P11: `Registry` for `kind=binary`,
///      `Url{...}` for `kind=external`.
pub fn plan(
    _user_config: &UserConfig,
    lockfile: &GlobalLockFile,
    manifest: &Manifest,
    qref: &QualifiedRef,
    version: &str,
    platform: PlatformKey,
) -> Plan {
    // 1. Platform must be available.
    let platform_entry = match manifest.platforms.get(&platform) {
        Some(p) => p,
        None => {
            return Plan::Error(PlanError::PlatformNotAvailable {
                requested: platform,
                available: manifest.platforms.keys().copied().collect(),
            });
        }
    };

    // 2. Shape must be installable as a shim/tool.
    let installable = matches!(
        manifest.shape,
        ComponentShape::ToolBinary
            | ComponentShape::HybridComponent
            | ComponentShape::ToolExternal
    );
    if !installable {
        return Plan::Error(PlanError::ShapeNotInstallable {
            shape: manifest.shape,
        });
    }

    // 3. expected_sha: lockfile pin wins; else manifest's claim.
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

    // 4. FetchPlan correspondence with manifest.kind (P11).
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
    use crate::global::manifest::{
        Arch, ManifestKind, Os, Platform, PlatformKey, ToolFacet,
    };
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
            shape: ComponentShape::ToolBinary,
        }
    }

    fn tool_external_manifest(sha_linux: &str) -> Manifest {
        let mut platforms = BTreeMap::new();
        platforms.insert(
            linux_amd64(),
            external_platform_targz(
                sha_linux,
                "https://github.com/example/x.tar.gz",
                "x/x",
            ),
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
            shape: ComponentShape::Component,
        }
    }

    // --- Happy paths --------------------------------------------------------

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
        // §1a.9: lockfile is the source of truth once a version has been run.
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
        // Manifest's sha differs (e.g., publisher republished the manifest
        // pointing at new bytes). Per spec re-publish is `ALREADY_EXISTS` on
        // the server side, but we defensively trust the lockfile here.
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
        // P11: kind=external => FetchPlan::Url, never Registry.
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

    // --- Plan variant correspondence (P11) ---------------------------------

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

    // --- Error cases --------------------------------------------------------

    #[test]
    fn errors_when_platform_not_available() {
        let cfg = UserConfig::default();
        let lock = GlobalLockFile::default();
        let m = tool_binary_manifest("aa"); // only linux/amd64

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
        // HYBRID has a tool facet — should be installable just like TOOL_BINARY.
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

    // --- Totality (P1) ------------------------------------------------------

    #[test]
    fn plan_is_total_on_arbitrary_empty_manifest_platforms() {
        // An empty platform map for either kind still returns a Plan, never
        // panics. (PlatformNotAvailable.available is empty in that case.)
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
        let is_error = matches!(
            p,
            Plan::Error(PlanError::PlatformNotAvailable { .. })
        );
        assert!(is_error);
    }
}
