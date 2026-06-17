//! Pure publish-decision logic for version immutability (TASKS/024).
//!
//! Given what the registry already knows about a version (its prior content
//! hash + published/unpublished state) and whether the version is a prerelease,
//! [`decide_publish`] returns how a (re)publish must proceed. This is the
//! correctness heart of the immutability feature — pure, exhaustively testable,
//! and shared so the CLI could pre-check the same way the server enforces.
//!
//! Nothing calls this yet; it is the groundwork the enforcement wiring
//! (TASKS/024 §B3/B4 — move the decision into `commit_upload`, relax
//! `begin_upload`) will build on.

/// Prior published/unpublished state of a version, as the aggregate knows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriorState {
    Published,
    Unpublished,
}

/// What the registry recorded for a version that was published before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriorVersion {
    /// Canonical manifest hash of the originally-published content
    /// (see [`crate::hash::manifest_hash`]).
    pub manifest_hash: String,
    pub state: PriorState,
}

/// How a publish of new content should proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishDecision {
    /// Version never published before — write and record it.
    FirstPublish,
    /// Stable version re-published with byte-identical content while already
    /// published — succeed as a no-op.
    IdempotentNoop,
    /// Stable version re-published with byte-identical content after unpublish —
    /// restore it to published.
    Restore,
    /// Stable version already published with *different* content — reject;
    /// the author must bump the version (burned, even after unpublish).
    RejectImmutable,
    /// Prerelease re-publish — overwrite freely (prereleases are mutable).
    OverwritePrerelease,
}

/// Decide how publishing `new_hash` must proceed (TASKS/024).
///
/// - First publish of any version ⇒ [`PublishDecision::FirstPublish`].
/// - Prerelease re-publish ⇒ [`PublishDecision::OverwritePrerelease`] (mutable).
/// - Stable re-publish, identical content ⇒ no-op (or restore if unpublished).
/// - Stable re-publish, different content ⇒ [`PublishDecision::RejectImmutable`].
pub fn decide_publish(
    prior: Option<&PriorVersion>,
    new_hash: &str,
    prerelease: bool,
) -> PublishDecision {
    match prior {
        None => PublishDecision::FirstPublish,
        Some(_) if prerelease => PublishDecision::OverwritePrerelease,
        Some(p) if p.manifest_hash == new_hash => match p.state {
            PriorState::Published => PublishDecision::IdempotentNoop,
            PriorState::Unpublished => PublishDecision::Restore,
        },
        Some(_) => PublishDecision::RejectImmutable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prior(hash: &str, state: PriorState) -> PriorVersion {
        PriorVersion {
            manifest_hash: hash.to_string(),
            state,
        }
    }

    #[test]
    fn first_publish_when_no_prior() {
        assert_eq!(decide_publish(None, "h", false), PublishDecision::FirstPublish);
        // Prerelease first publish is still a first publish.
        assert_eq!(decide_publish(None, "h", true), PublishDecision::FirstPublish);
    }

    #[test]
    fn p1_stable_different_content_is_rejected() {
        let p = prior("OLD", PriorState::Published);
        assert_eq!(
            decide_publish(Some(&p), "NEW", false),
            PublishDecision::RejectImmutable
        );
    }

    #[test]
    fn p2_stable_identical_published_is_noop() {
        let p = prior("H", PriorState::Published);
        assert_eq!(
            decide_publish(Some(&p), "H", false),
            PublishDecision::IdempotentNoop
        );
    }

    #[test]
    fn p2_stable_identical_unpublished_restores() {
        let p = prior("H", PriorState::Unpublished);
        assert_eq!(
            decide_publish(Some(&p), "H", false),
            PublishDecision::Restore
        );
    }

    #[test]
    fn p4_burned_after_unpublish_rejects_different() {
        let p = prior("H", PriorState::Unpublished);
        assert_eq!(
            decide_publish(Some(&p), "DIFFERENT", false),
            PublishDecision::RejectImmutable
        );
    }

    #[test]
    fn p3_prerelease_always_overwrites_never_rejects() {
        for state in [PriorState::Published, PriorState::Unpublished] {
            let p = prior("H", state);
            // Different content — a stable version would be rejected here.
            assert_eq!(
                decide_publish(Some(&p), "DIFFERENT", true),
                PublishDecision::OverwritePrerelease
            );
            // Identical content too.
            assert_eq!(
                decide_publish(Some(&p), "H", true),
                PublishDecision::OverwritePrerelease
            );
        }
    }
}
