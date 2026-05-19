//! Semver version spec parsing and matching.
//!
//! Supports:
//!   "1.2.3"  — exact version
//!   "1.2"    — any patch (>=1.2.0, <1.3.0)
//!   "1"      — any minor+patch (>=1.0.0, <2.0.0)
//!   "latest" — any version

/// A parsed version specification from forest.cue dependencies.
#[derive(Debug, Clone, PartialEq)]
pub enum VersionSpec {
    /// Exact version: "1.2.3"
    Exact(semver::Version),
    /// Minor range: "1.2" matches >=1.2.0, <1.3.0
    Minor { major: u64, minor: u64 },
    /// Major range: "1" matches >=1.0.0, <2.0.0
    Major { major: u64 },
    /// Latest: any version
    Latest,
}

impl VersionSpec {
    /// Parse a version spec string.
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        let s = s.trim();

        if s == "latest" || s == "*" {
            return Ok(Self::Latest);
        }

        let parts: Vec<&str> = s.split('.').collect();
        match parts.len() {
            3 => {
                let version = semver::Version::parse(s)
                    .map_err(|e| anyhow::anyhow!("invalid version '{s}': {e}"))?;
                Ok(Self::Exact(version))
            }
            2 => {
                let major: u64 = parts[0]
                    .parse()
                    .map_err(|_| anyhow::anyhow!("invalid major version in '{s}'"))?;
                let minor: u64 = parts[1]
                    .parse()
                    .map_err(|_| anyhow::anyhow!("invalid minor version in '{s}'"))?;
                Ok(Self::Minor { major, minor })
            }
            1 => {
                let major: u64 = parts[0]
                    .parse()
                    .map_err(|_| anyhow::anyhow!("invalid major version in '{s}'"))?;
                Ok(Self::Major { major })
            }
            _ => anyhow::bail!("invalid version spec: '{s}'"),
        }
    }

    /// Check if a concrete version matches this spec.
    pub fn matches(&self, version: &semver::Version) -> bool {
        match self {
            Self::Exact(v) => version == v,
            Self::Minor { major, minor } => {
                version.major == *major && version.minor == *minor
            }
            Self::Major { major } => version.major == *major,
            Self::Latest => true,
        }
    }

    /// Given a list of available versions, return the highest that matches.
    pub fn resolve<'a>(&self, versions: &'a [semver::Version]) -> Option<&'a semver::Version> {
        versions
            .iter()
            .filter(|v| self.matches(v))
            .max()
    }
}

impl std::fmt::Display for VersionSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exact(v) => write!(f, "{v}"),
            Self::Minor { major, minor } => write!(f, "{major}.{minor}"),
            Self::Major { major } => write!(f, "{major}"),
            Self::Latest => write!(f, "latest"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> semver::Version {
        semver::Version::parse(s).unwrap()
    }

    #[test]
    fn test_parse_exact() {
        assert_eq!(
            VersionSpec::parse("1.2.3").unwrap(),
            VersionSpec::Exact(v("1.2.3"))
        );
    }

    #[test]
    fn test_parse_minor() {
        assert_eq!(
            VersionSpec::parse("1.2").unwrap(),
            VersionSpec::Minor { major: 1, minor: 2 }
        );
    }

    #[test]
    fn test_parse_major() {
        assert_eq!(
            VersionSpec::parse("1").unwrap(),
            VersionSpec::Major { major: 1 }
        );
    }

    #[test]
    fn test_parse_latest() {
        assert_eq!(VersionSpec::parse("latest").unwrap(), VersionSpec::Latest);
    }

    #[test]
    fn test_matches_minor() {
        let spec = VersionSpec::Minor { major: 1, minor: 2 };
        assert!(spec.matches(&v("1.2.0")));
        assert!(spec.matches(&v("1.2.9")));
        assert!(!spec.matches(&v("1.3.0")));
        assert!(!spec.matches(&v("2.2.0")));
    }

    #[test]
    fn test_matches_major() {
        let spec = VersionSpec::Major { major: 1 };
        assert!(spec.matches(&v("1.0.0")));
        assert!(spec.matches(&v("1.9.9")));
        assert!(!spec.matches(&v("2.0.0")));
    }

    #[test]
    fn test_resolve_picks_highest() {
        let versions = vec![v("0.1.0"), v("0.2.0"), v("0.2.5"), v("1.0.0")];

        let spec = VersionSpec::Minor { major: 0, minor: 2 };
        assert_eq!(spec.resolve(&versions), Some(&v("0.2.5")));

        let spec = VersionSpec::Major { major: 0 };
        assert_eq!(spec.resolve(&versions), Some(&v("0.2.5")));

        let spec = VersionSpec::Latest;
        assert_eq!(spec.resolve(&versions), Some(&v("1.0.0")));
    }
}
