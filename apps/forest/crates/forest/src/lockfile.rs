use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Context;

const LOCK_FILE_NAME: &str = "forest.lock";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LockEntry {
    pub organisation: String,
    pub name: String,
    pub version: String,
    pub source: LockSource,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum LockSource {
    Registry {
        os: String,
        arch: String,
        sha256: String,
    },
    Path {
        path: String,
    },
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

#[derive(Debug, Clone, Default)]
pub struct LockFile {
    entries: BTreeMap<String, LockEntry>,
}

impl LockFile {
    pub async fn load(project_dir: &Path) -> anyhow::Result<Self> {
        let path = project_dir.join(LOCK_FILE_NAME);
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = tokio::fs::read_to_string(&path)
            .await
            .context("read forest.lock")?;

        let mut entries = BTreeMap::new();
        for (index, line) in content.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let entry = parse_lock_line(line).with_context(|| {
                format!("invalid lock entry at {}:{}", path.display(), index + 1)
            })?;
            if entries.insert(entry.key(), entry).is_some() {
                anyhow::bail!("duplicate lock entry at {}:{}", path.display(), index + 1);
            }
        }

        Ok(Self { entries })
    }

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

    pub fn insert(&mut self, entry: LockEntry) {
        let key = entry.key();
        self.entries.insert(key, entry);
    }

    pub fn locked_versions(&self, org: &str, name: &str) -> Vec<semver::Version> {
        self.entries
            .values()
            .filter(|e| e.organisation == org && e.name == name)
            .filter(|e| matches!(e.source, LockSource::Registry { .. }))
            .filter_map(|e| semver::Version::parse(&e.version).ok())
            .collect()
    }

    pub fn get(&self, org: &str, name: &str, version: &str, os: &str, arch: &str) -> Option<&str> {
        let key = format!("{org}/{name}@{version} {os}/{arch}");
        self.entries.get(&key).and_then(|e| match &e.source {
            LockSource::Registry { sha256, .. } => Some(sha256.as_str()),
            LockSource::Path { .. } => None,
        })
    }

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

fn parse_lock_line(line: &str) -> Option<LockEntry> {
    let (component, source) = line.split_once(' ')?;
    let source = source.trim_start();
    let (org_name, version) = component.split_once('@')?;
    let (org, name) = org_name.split_once('/')?;
    if org.is_empty()
        || name.is_empty()
        || name.contains('/')
        || component.chars().any(char::is_whitespace)
        || semver::Version::parse(version).is_err()
    {
        return None;
    }

    if let Some(path) = source.strip_prefix("path:") {
        if path.is_empty() {
            return None;
        }
        return Some(LockEntry {
            organisation: org.to_string(),
            name: name.to_string(),
            version: version.to_string(),
            source: LockSource::Path {
                path: path.to_string(),
            },
        });
    }

    let mut fields = source.split_whitespace();
    let (os, arch) = fields.next()?.split_once('/')?;
    let sha256 = fields.next()?;
    let digest = sha256.strip_prefix("sha256:")?;
    if os.is_empty()
        || arch.is_empty()
        || arch.contains('/')
        || digest.len() != 64
        || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        || fields.next().is_some()
    {
        return None;
    }
    Some(LockEntry {
        organisation: org.to_string(),
        name: name.to_string(),
        version: version.to_string(),
        source: LockSource::Registry {
            os: os.to_string(),
            arch: arch.to_string(),
            sha256: sha256.to_string(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn corrupt_lock_entries_fail_instead_of_unlocking_dependencies() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(LOCK_FILE_NAME);
        let digest = "a".repeat(64);
        for line in [
            "org/comp@1.0.0".to_string(),
            format!("org/comp@invalid linux/amd64 sha256:{digest}"),
            "org/comp@1.0.0 linux/amd64 sha256:truncated".to_string(),
            format!("org/comp@1.0.0 linux/amd64 sha256:{digest} trailing"),
        ] {
            tokio::fs::write(&path, &line).await.unwrap();
            let error = LockFile::load(dir.path()).await.unwrap_err();
            assert!(error.to_string().contains("invalid lock entry"));
            assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), line);
        }
    }

    #[tokio::test]
    async fn duplicate_lock_entries_fail_instead_of_overriding_a_digest() {
        let dir = tempfile::tempdir().unwrap();
        let content = format!(
            "org/comp@1.0.0 linux/amd64 sha256:{}\norg/comp@1.0.0 linux/amd64 sha256:{}\n",
            "a".repeat(64),
            "b".repeat(64)
        );
        tokio::fs::write(dir.path().join(LOCK_FILE_NAME), &content)
            .await
            .unwrap();
        let error = LockFile::load(dir.path()).await.unwrap_err();
        assert!(error.to_string().contains("duplicate lock entry"));
    }

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
        assert_eq!(entry.key(), "forest-contrib/terraform-service@0.1.0");
        assert_eq!(
            entry.to_line(),
            "forest-contrib/terraform-service@0.1.0 path:../../components/forest-contrib/terraform-service"
        );
    }

    #[test]
    fn test_parse_registry_line() {
        let entry = parse_lock_line(&format!(
            "forest-contrib/k8s@0.1.0 linux/amd64 sha256:{}",
            "a".repeat(64)
        ))
        .unwrap();
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

        assert!(
            lock.verify("org", "comp", "1.0.0", "linux", "amd64", "sha256:abc")
                .is_ok()
        );
        assert!(
            lock.verify("org", "comp", "1.0.0", "linux", "amd64", "sha256:WRONG")
                .is_err()
        );
        assert!(
            lock.verify("org", "other", "1.0.0", "linux", "amd64", "sha256:anything")
                .is_ok()
        );
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
        assert!(lock.get("org", "comp", "1.0.0", "linux", "amd64").is_none());
    }
}
