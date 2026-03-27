//! forest.lock — records the resolved dependency snapshot for a project.
//!
//! Like Cargo.lock, this records all dependencies:
//! - **Registry deps**: org/name@version os/arch sha256:hash
//! - **Path deps**: org/name@version path:/relative/path
//!
//! Path deps are "soft-locked" — the lock records what version was seen,
//! but resolution always uses whatever is on disk (just like Cargo).
//!
//! Registry deps are "hard-locked" — the SHA-256 is verified on download.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Context;

const LOCK_FILE_NAME: &str = "forest.lock";

/// A lock file entry.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LockEntry {
    pub organisation: String,
    pub name: String,
    pub version: String,
    pub source: LockSource,
}

/// Where a locked dependency comes from.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum LockSource {
    /// Registry dependency — locked to a specific binary hash per platform.
    Registry {
        os: String,
        arch: String,
        sha256: String,
    },
    /// Local path dependency — records the path, but always resolves from disk.
    Path { path: String },
}

impl LockEntry {
    fn key(&self) -> String {
        match &self.source {
            LockSource::Registry { os, arch, .. } => {
                format!(
                    "{}/{}@{} {}/{}",
                    self.organisation, self.name, self.version, os, arch
                )
            }
            LockSource::Path { .. } => {
                format!("{}/{}@{}", self.organisation, self.name, self.version)
            }
        }
    }

    fn to_line(&self) -> String {
        match &self.source {
            LockSource::Registry { os, arch, sha256 } => {
                format!(
                    "{}/{}@{} {}/{} {}",
                    self.organisation, self.name, self.version, os, arch, sha256
                )
            }
            LockSource::Path { path } => {
                format!(
                    "{}/{}@{} path:{}",
                    self.organisation, self.name, self.version, path
                )
            }
        }
    }
}

/// The lock file contents.
#[derive(Debug, Clone, Default)]
pub struct LockFile {
    entries: BTreeMap<String, LockEntry>,
}

impl LockFile {
    /// Load from a project directory. Returns empty if no lock file exists.
    pub async fn load(project_dir: &Path) -> anyhow::Result<Self> {
        let path = project_dir.join(LOCK_FILE_NAME);
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = tokio::fs::read_to_string(&path)
            .await
            .context("read forest.lock")?;

        let mut entries = BTreeMap::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            if let Some(entry) = parse_lock_line(line) {
                entries.insert(entry.key(), entry);
            }
        }

        Ok(Self { entries })
    }

    /// Save to a project directory.
    pub async fn save(&self, project_dir: &Path) -> anyhow::Result<()> {
        let path = project_dir.join(LOCK_FILE_NAME);
        let mut content = String::from("# forest.lock — do not edit manually\n");

        for entry in self.entries.values() {
            content.push_str(&entry.to_line());
            content.push('\n');
        }

        tokio::fs::write(&path, &content)
            .await
            .context("write forest.lock")?;
        Ok(())
    }

    /// Add or update an entry.
    pub fn insert(&mut self, entry: LockEntry) {
        let key = entry.key();
        self.entries.insert(key, entry);
    }

    /// Look up the expected hash for a registry component+platform.
    pub fn get(
        &self,
        org: &str,
        name: &str,
        version: &str,
        os: &str,
        arch: &str,
    ) -> Option<&str> {
        let key = format!("{org}/{name}@{version} {os}/{arch}");
        self.entries.get(&key).and_then(|e| match &e.source {
            LockSource::Registry { sha256, .. } => Some(sha256.as_str()),
            LockSource::Path { .. } => None,
        })
    }

    /// Check if a hash matches the lock file expectation.
    /// Returns Ok if matches or no entry exists. Returns Err if mismatch.
    pub fn verify(
        &self,
        org: &str,
        name: &str,
        version: &str,
        os: &str,
        arch: &str,
        actual_sha256: &str,
    ) -> anyhow::Result<()> {
        if let Some(expected) = self.get(org, name, version, os, arch) {
            if expected != actual_sha256 {
                anyhow::bail!(
                    "lock file mismatch for {org}/{name}@{version} ({os}/{arch}):\n  \
                     expected: {expected}\n  \
                     got:      {actual_sha256}\n\n\
                     The component binary has changed since the lock file was created.\n\
                     Run `forest update` to update the lock file, or investigate the change."
                );
            }
        }
        Ok(())
    }
}

/// Parse a single lock file line.
fn parse_lock_line(line: &str) -> Option<LockEntry> {
    let parts: Vec<&str> = line.splitn(3, ' ').collect();
    if parts.len() < 2 {
        return None;
    }

    let component = parts[0];
    let (org_name, version) = component.split_once('@')?;
    let (org, name) = org_name.split_once('/')?;

    // Path dep: "org/name@version path:/some/path"
    if parts[1].starts_with("path:") {
        let path = parts[1].strip_prefix("path:")?;
        return Some(LockEntry {
            organisation: org.to_string(),
            name: name.to_string(),
            version: version.to_string(),
            source: LockSource::Path {
                path: path.to_string(),
            },
        });
    }

    // Registry dep: "org/name@version os/arch sha256:hash"
    if parts.len() != 3 {
        return None;
    }

    let (os, arch) = parts[1].split_once('/')?;
    Some(LockEntry {
        organisation: org.to_string(),
        name: name.to_string(),
        version: version.to_string(),
        source: LockSource::Registry {
            os: os.to_string(),
            arch: arch.to_string(),
            sha256: parts[2].to_string(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_entry() {
        let entry = LockEntry {
            organisation: "forest-contrib".into(),
            name: "kubernetes-service".into(),
            version: "0.1.0".into(),
            source: LockSource::Registry {
                os: "linux".into(),
                arch: "amd64".into(),
                sha256: "sha256:abc123".into(),
            },
        };
        assert_eq!(
            entry.key(),
            "forest-contrib/kubernetes-service@0.1.0 linux/amd64"
        );
        assert_eq!(
            entry.to_line(),
            "forest-contrib/kubernetes-service@0.1.0 linux/amd64 sha256:abc123"
        );
    }

    #[test]
    fn test_path_entry() {
        let entry = LockEntry {
            organisation: "forest-contrib".into(),
            name: "terraform-service".into(),
            version: "0.1.0".into(),
            source: LockSource::Path {
                path: "../../components/forest-contrib/terraform-service".into(),
            },
        };
        assert_eq!(
            entry.key(),
            "forest-contrib/terraform-service@0.1.0"
        );
        assert_eq!(
            entry.to_line(),
            "forest-contrib/terraform-service@0.1.0 path:../../components/forest-contrib/terraform-service"
        );
    }

    #[test]
    fn test_parse_registry_line() {
        let entry =
            parse_lock_line("forest-contrib/k8s@0.1.0 linux/amd64 sha256:abc").unwrap();
        assert_eq!(entry.organisation, "forest-contrib");
        assert_eq!(entry.name, "k8s");
        assert_eq!(entry.version, "0.1.0");
        assert!(matches!(entry.source, LockSource::Registry { .. }));
    }

    #[test]
    fn test_parse_path_line() {
        let entry =
            parse_lock_line("forest-contrib/terraform-service@0.1.0 path:../../components/tf")
                .unwrap();
        assert_eq!(entry.organisation, "forest-contrib");
        assert_eq!(entry.name, "terraform-service");
        assert_eq!(entry.version, "0.1.0");
        assert!(matches!(
            entry.source,
            LockSource::Path { path } if path == "../../components/tf"
        ));
    }

    #[test]
    fn test_verify_match() {
        let mut lock = LockFile::default();
        lock.insert(LockEntry {
            organisation: "org".into(),
            name: "comp".into(),
            version: "1.0.0".into(),
            source: LockSource::Registry {
                os: "linux".into(),
                arch: "amd64".into(),
                sha256: "sha256:abc".into(),
            },
        });

        assert!(lock
            .verify("org", "comp", "1.0.0", "linux", "amd64", "sha256:abc")
            .is_ok());
        assert!(lock
            .verify("org", "comp", "1.0.0", "linux", "amd64", "sha256:WRONG")
            .is_err());
        // No entry → ok (first time)
        assert!(lock
            .verify("org", "other", "1.0.0", "linux", "amd64", "sha256:anything")
            .is_ok());
    }

    #[test]
    fn test_path_entry_not_in_get() {
        let mut lock = LockFile::default();
        lock.insert(LockEntry {
            organisation: "org".into(),
            name: "comp".into(),
            version: "1.0.0".into(),
            source: LockSource::Path {
                path: "./local".into(),
            },
        });
        // get() only returns registry hashes, not path entries
        assert!(lock.get("org", "comp", "1.0.0", "linux", "amd64").is_none());
    }
}
