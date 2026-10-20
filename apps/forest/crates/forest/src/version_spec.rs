#[derive(Debug, Clone, PartialEq)]
pub enum VersionSpec {
    Exact(semver::Version),
    Minor { major: u64, minor: u64 },
    Major { major: u64 },
    Latest,
}

impl VersionSpec {
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        let s = s.trim();

        if s == "latest" || s == "*" {
            return Ok(Self::Latest);
        }

        if let Ok(version) = semver::Version::parse(s) {
            return Ok(Self::Exact(version));
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

    pub fn matches(&self, version: &semver::Version) -> bool {
        self.matches_with(version, false)
    }

    fn matches_with(&self, version: &semver::Version, allow_prerelease: bool) -> bool {
        let usable = allow_prerelease || version.pre.is_empty();
        match self {
            Self::Exact(v) => version == v,
            Self::Minor { major, minor } => {
                usable && version.major == *major && version.minor == *minor
            }
            Self::Major { major } => usable && version.major == *major,
            Self::Latest => usable,
        }
    }

    pub fn is_exact(&self) -> bool {
        matches!(self, Self::Exact(_))
    }

    pub fn pin(
        &self,
        locked: &[semver::Version],
        published: &[semver::Version],
    ) -> Option<semver::Version> {
        self.resolve(locked)
            .or_else(|| self.resolve(published))
            .cloned()
    }

    pub fn resolve<'a>(&self, versions: &'a [semver::Version]) -> Option<&'a semver::Version> {
        versions
            .iter()
            .filter(|v| self.matches(v))
            .max()
            .or_else(|| versions.iter().filter(|v| self.matches_with(v, true)).max())
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
    fn a_dotted_prerelease_is_an_exact_version() {
        assert_eq!(
            VersionSpec::parse("1.3.0-rc.1").unwrap(),
            VersionSpec::Exact(v("1.3.0-rc.1"))
        );
    }

    #[test]
    fn a_range_pins_to_the_locked_version_before_a_newer_published_one() {
        let spec = VersionSpec::Minor { major: 0, minor: 3 };
        assert_eq!(
            spec.pin(&[v("0.3.0")], &[v("0.3.0"), v("0.3.1")]),
            Some(v("0.3.0"))
        );
    }

    #[test]
    fn a_range_without_a_matching_lock_takes_the_highest_published_stable() {
        let spec = VersionSpec::Minor { major: 0, minor: 3 };
        assert_eq!(
            spec.pin(&[v("0.2.0")], &[v("0.3.0"), v("0.3.1"), v("0.3.2-rc.1")]),
            Some(v("0.3.1"))
        );
        assert_eq!(spec.pin(&[], &[v("0.4.0")]), None);
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

#[cfg(test)]
mod prerelease_tests {
    use super::VersionSpec;

    fn v(s: &str) -> semver::Version {
        semver::Version::parse(s).unwrap()
    }

    fn versions(list: &[&str]) -> Vec<semver::Version> {
        list.iter().map(|s| v(s)).collect()
    }

    #[test]
    fn latest_skips_a_prerelease_that_outranks_the_release() {
        let all = versions(&["0.1.7", "0.1.8", "0.2.0-rc.1"]);
        assert_eq!(VersionSpec::Latest.resolve(&all), Some(&v("0.1.8")));
    }

    #[test]
    fn major_and_minor_specs_skip_prereleases_too() {
        let all = versions(&["0.1.7", "0.1.8", "0.1.9-rc.1"]);
        assert_eq!(
            VersionSpec::Minor { major: 0, minor: 1 }.resolve(&all),
            Some(&v("0.1.8"))
        );
        assert_eq!(
            VersionSpec::Major { major: 0 }.resolve(&all),
            Some(&v("0.1.8"))
        );
    }

    #[test]
    fn an_exact_spec_still_selects_a_prerelease() {
        let all = versions(&["0.1.8", "0.2.0-rc.1"]);
        let spec = VersionSpec::Exact(v("0.2.0-rc.1"));
        assert_eq!(spec.resolve(&all), Some(&v("0.2.0-rc.1")));
    }

    #[test]
    fn a_component_with_only_prereleases_is_still_installable() {
        let all = versions(&["0.1.0-rc.1", "0.1.0-rc.2"]);
        assert_eq!(VersionSpec::Latest.resolve(&all), Some(&v("0.1.0-rc.2")));
    }

    #[test]
    fn the_fallback_never_outranks_a_stable_version() {
        let all = versions(&["0.1.0", "9.9.9-rc.1"]);
        assert_eq!(VersionSpec::Latest.resolve(&all), Some(&v("0.1.0")));
    }

    #[test]
    fn build_metadata_is_not_a_prerelease() {
        let all = versions(&["0.1.0", "0.1.1+build.5"]);
        assert_eq!(VersionSpec::Latest.resolve(&all), Some(&v("0.1.1+build.5")));
    }
}
