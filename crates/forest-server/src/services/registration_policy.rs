//! Registration policy: gate which emails are allowed to create accounts
//! (and to be added post-signup) on this Forest instance.
//!
//! Pure core. No I/O, no DB, no clock.

use std::sync::Arc;

use crate::State;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RegistrationPolicyError {
    #[error("email does not match the allowed domain pattern")]
    DomainNotAllowed,
}

#[derive(Clone)]
pub struct RegistrationPolicy {
    domain_regex: Option<Arc<regex::Regex>>,
}

impl RegistrationPolicy {
    pub fn new(domain_regex: Option<regex::Regex>) -> Self {
        Self {
            domain_regex: domain_regex.map(Arc::new),
        }
    }

    pub fn unrestricted() -> Self {
        Self { domain_regex: None }
    }

    pub fn check_email(&self, email: &str) -> Result<(), RegistrationPolicyError> {
        let Some(regex) = self.domain_regex.as_ref() else {
            return Ok(());
        };

        let normalized = email.trim().to_lowercase();
        if regex.is_match(&normalized) {
            Ok(())
        } else {
            Err(RegistrationPolicyError::DomainNotAllowed)
        }
    }
}

pub trait RegistrationPolicyState {
    fn registration_policy(&self) -> RegistrationPolicy;
}

impl RegistrationPolicyState for State {
    fn registration_policy(&self) -> RegistrationPolicy {
        RegistrationPolicy::new(self.config.registration_email_domain_regex.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn understory_only() -> RegistrationPolicy {
        RegistrationPolicy::new(Some(regex::Regex::new(r"@understory\.io$").unwrap()))
    }

    #[test]
    fn no_regex_allows_everything() {
        let policy = RegistrationPolicy::unrestricted();
        assert!(policy.check_email("anything@anywhere.com").is_ok());
        assert!(policy.check_email("").is_ok());
    }

    #[test]
    fn matching_domain_allowed() {
        assert!(understory_only().check_email("kasper@understory.io").is_ok());
    }

    #[test]
    fn non_matching_domain_rejected() {
        assert_eq!(
            understory_only().check_email("attacker@evil.com"),
            Err(RegistrationPolicyError::DomainNotAllowed)
        );
    }

    #[test]
    fn case_insensitive_normalization() {
        assert!(understory_only().check_email("Kasper@Understory.IO").is_ok());
    }

    #[test]
    fn anchored_regex_rejects_suffix_attack() {
        // operator-supplied $ anchor protects against this
        assert_eq!(
            understory_only().check_email("kasper@understory.io.evil.com"),
            Err(RegistrationPolicyError::DomainNotAllowed)
        );
    }

    #[test]
    fn whitespace_is_trimmed_before_match() {
        assert!(
            understory_only()
                .check_email("   kasper@understory.io   ")
                .is_ok()
        );
    }

    #[test]
    fn empty_email_rejected_when_regex_set() {
        assert_eq!(
            understory_only().check_email(""),
            Err(RegistrationPolicyError::DomainNotAllowed)
        );
    }

    #[test]
    fn unanchored_regex_is_a_footgun_but_honored() {
        // Operator wrote `@understory\.io` without a `$`. We do not
        // auto-wrap; the spec calls this out and we log the compiled
        // pattern at startup so they can see what they configured.
        let policy =
            RegistrationPolicy::new(Some(regex::Regex::new(r"@understory\.io").unwrap()));
        assert!(policy.check_email("kasper@understory.io.evil.com").is_ok());
    }

    #[test]
    fn empty_string_regex_rejects_every_real_email() {
        let policy = RegistrationPolicy::new(Some(regex::Regex::new(r"^$").unwrap()));
        assert_eq!(
            policy.check_email("kasper@understory.io"),
            Err(RegistrationPolicyError::DomainNotAllowed)
        );
    }
}
