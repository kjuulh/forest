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
    ///
    /// Range-style specs (`latest`, `0`, `0.1`) never select a prerelease
    /// (DATA-583). `Latest` used to match everything and take the semver
    /// maximum, so publishing `0.2.0-rc.1` made it what every `forest global
    /// add <org>/<name>` installed — a release candidate promoting itself to
    /// everyone. It also meant a CI test tag had to be numbered *below* the
    /// current release to stay inert, which is exactly backwards.
    ///
    /// This matches the rest of the ecosystem (npm, cargo, pip all refuse to
    /// resolve a range to a prerelease) and the convention the server already
    /// applies in `list_org_tools`, which picks the highest non-prerelease.
    ///
    /// An `Exact` spec still matches a prerelease, so `@0.2.0-rc.1` opts in
    /// deliberately — which is the whole point of publishing one.
    pub fn matches(&self, version: &semver::Version) -> bool {
        self.matches_with(version, false)
    }

    /// `matches`, with an escape hatch for the fallback in [`Self::resolve`].
    fn matches_with(&self, version: &semver::Version, allow_prerelease: bool) -> bool {
        let usable = allow_prerelease || version.pre.is_empty();
        match self {
            // Exact is an explicit request — a prerelease is selectable by
            // naming it, and only by naming it.
            Self::Exact(v) => version == v,
            Self::Minor { major, minor } => {
                usable && version.major == *major && version.minor == *minor
            }
            Self::Major { major } => usable && version.major == *major,
            Self::Latest => usable,
        }
    }

    /// Given a list of available versions, return the highest that matches.
    ///
    /// Falls back to allowing prereleases when nothing stable matches, so a
    /// component that has only ever published release candidates stays
    /// installable rather than resolving to nothing. The fallback can only
    /// ever *add* candidates — it never outranks a stable version, because it
    /// is reached only when there is no stable match at all.
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

/// DATA-583 — range specs must not resolve to a prerelease.
///
/// The bug these pin down was found by publishing `pgjump@0.1.7-ci.1` as a CI
/// test: `Latest` matched it, and only the fact that it sorted *below* the
/// current release kept it from becoming what everyone installed.
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
        // The exact shape of the incident: an rc numbered above the current
        // release must not become `latest`.
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
        // Naming it is how you opt in — otherwise publishing a release
        // candidate would be pointless.
        let all = versions(&["0.1.8", "0.2.0-rc.1"]);
        let spec = VersionSpec::Exact(v("0.2.0-rc.1"));
        assert_eq!(spec.resolve(&all), Some(&v("0.2.0-rc.1")));
    }

    #[test]
    fn a_component_with_only_prereleases_is_still_installable() {
        // The fallback. Refusing to resolve at all would strand a component
        // that has never cut a stable release.
        let all = versions(&["0.1.0-rc.1", "0.1.0-rc.2"]);
        assert_eq!(VersionSpec::Latest.resolve(&all), Some(&v("0.1.0-rc.2")));
    }

    #[test]
    fn the_fallback_never_outranks_a_stable_version() {
        // Guards the ordering of the two passes: as soon as anything stable
        // matches, prereleases are out of the running entirely.
        let all = versions(&["0.1.0", "9.9.9-rc.1"]);
        assert_eq!(VersionSpec::Latest.resolve(&all), Some(&v("0.1.0")));
    }

    #[test]
    fn build_metadata_is_not_a_prerelease() {
        // `+build` is not `-pre`; it must stay selectable by a range spec.
        let all = versions(&["0.1.0", "0.1.1+build.5"]);
        assert_eq!(VersionSpec::Latest.resolve(&all), Some(&v("0.1.1+build.5")));
    }
}
