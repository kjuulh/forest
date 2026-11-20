#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriorState {
    Published,
    Unpublished,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriorVersion {
    pub manifest_hash: String,
    pub state: PriorState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishDecision {
    FirstPublish,
    IdempotentNoop,
    Restore,
    RejectImmutable,
    OverwritePrerelease,
}

/// Stable versions are immutable: different content is rejected, while prereleases are overwritten.
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
        assert_eq!(
            decide_publish(None, "h", false),
            PublishDecision::FirstPublish
        );
        assert_eq!(
            decide_publish(None, "h", true),
            PublishDecision::FirstPublish
        );
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
            assert_eq!(
                decide_publish(Some(&p), "DIFFERENT", true),
                PublishDecision::OverwritePrerelease
            );
            assert_eq!(
                decide_publish(Some(&p), "H", true),
                PublishDecision::OverwritePrerelease
            );
        }
    }
}
