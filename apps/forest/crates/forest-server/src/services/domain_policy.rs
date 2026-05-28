//! Pure-core validation for org-allowed email domains (DATA-252).
//!
//! No I/O, no DB, no clock. Used both at write-time (admin adds a domain
//! to an org's allowlist) and at decision-time (joining a user — re-checked
//! so an expanded denylist retroactively blocks lingering rows).

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AllowedDomainError {
    #[error("domain cannot be empty")]
    Empty,
    #[error("domain is not a valid hostname")]
    InvalidSyntax,
    #[error("domain exceeds 253 characters")]
    TooLong,
}

/// Trim, lowercase, strip a leading '@', validate hostname syntax. Returns
/// the canonical form to store. No free-mail filtering — DNS verification
/// is the security boundary: even if an admin types `gmail.com`, the row
/// grants nothing until a TXT record they can never publish proves they
/// own the domain.
pub fn normalize_domain(raw: &str) -> Result<String, AllowedDomainError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AllowedDomainError::Empty);
    }
    let stripped = trimmed.strip_prefix('@').unwrap_or(trimmed);
    if stripped.is_empty() {
        return Err(AllowedDomainError::Empty);
    }
    let lower = stripped.to_ascii_lowercase();

    if lower.len() > 253 {
        return Err(AllowedDomainError::TooLong);
    }

    if !is_valid_hostname(&lower) {
        return Err(AllowedDomainError::InvalidSyntax);
    }

    Ok(lower)
}

/// Extract the lowercased domain part of an email. Returns None on any
/// malformed input. Intentionally permissive (no full RFC 5321 grammar —
/// the caller is operating on emails already accepted by the rest of the
/// system).
pub fn extract_domain(email: &str) -> Option<String> {
    let (_, domain) = email.trim().rsplit_once('@')?;
    if domain.is_empty() {
        return None;
    }
    Some(domain.to_ascii_lowercase())
}

/// Hostname syntax: 1+ labels separated by '.', each 1–63 chars of
/// [a-z0-9-], not starting/ending with '-'. Requires at least one dot
/// (no bare TLDs).
fn is_valid_hostname(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let labels: Vec<&str> = s.split('.').collect();
    if labels.len() < 2 {
        return false;
    }
    for label in labels {
        if label.is_empty() || label.len() > 63 {
            return false;
        }
        if label.starts_with('-') || label.ends_with('-') {
            return false;
        }
        if !label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return false;
        }
    }
    true
}

/// Policy values written to organisation_allowed_domains.policy.
///
/// Kept as a free string in SQL (consistent with how org member `role` is
/// modeled) but constrained via this enum at all service boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllowedDomainPolicy {
    AutoInviteAnyVerified,
    ManualOnly,
    /// v1.1 only — silent JIT for OAuth-verified emails at DNS-verified
    /// domains. v1 service code rejects this with `PolicyNotYetSupported`.
    AutoJoinOauth,
}

impl AllowedDomainPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AutoInviteAnyVerified => "auto_invite_any_verified",
            Self::ManualOnly => "manual_only",
            Self::AutoJoinOauth => "auto_join_oauth",
        }
    }

    pub fn parse(s: &str) -> Result<Self, PolicyParseError> {
        match s {
            "auto_invite_any_verified" => Ok(Self::AutoInviteAnyVerified),
            "manual_only" => Ok(Self::ManualOnly),
            "auto_join_oauth" => Ok(Self::AutoJoinOauth),
            _ => Err(PolicyParseError(s.to_string())),
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("unknown allowed-domain policy: {0}")]
pub struct PolicyParseError(pub String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_at_and_lowercases() {
        assert_eq!(
            normalize_domain("@Understory.IO").unwrap(),
            "understory.io"
        );
        assert_eq!(normalize_domain("  Example.COM  ").unwrap(), "example.com");
    }

    #[test]
    fn normalize_rejects_empty() {
        assert_eq!(normalize_domain(""), Err(AllowedDomainError::Empty));
        assert_eq!(normalize_domain("   "), Err(AllowedDomainError::Empty));
        assert_eq!(normalize_domain("@"), Err(AllowedDomainError::Empty));
    }

    #[test]
    fn normalize_accepts_free_mail_domains() {
        // No client-side denylist: DNS verification is the security
        // boundary, not domain identity. An admin who types `gmail.com`
        // gets an entry that grants nothing until a TXT proof they can
        // never publish appears.
        assert_eq!(normalize_domain("gmail.com").unwrap(), "gmail.com");
        assert_eq!(normalize_domain("outlook.com").unwrap(), "outlook.com");
    }

    #[test]
    fn normalize_rejects_invalid_syntax() {
        for &d in &[
            "no-dot-tld",
            "double..dot.com",
            "-leading-hyphen.com",
            "trailing-.com",
            "spaces in.com",
            ".starts-with-dot.com",
            "ends-with-dot.com.",
        ] {
            assert_eq!(
                normalize_domain(d),
                Err(AllowedDomainError::InvalidSyntax),
                "expected invalid-syntax for {d}"
            );
        }
    }

    #[test]
    fn normalize_rejects_too_long() {
        let long = format!("{}.com", "a".repeat(260));
        assert_eq!(normalize_domain(&long), Err(AllowedDomainError::TooLong));
    }

    #[test]
    fn normalize_accepts_subdomain() {
        assert_eq!(
            normalize_domain("eng.understory.io").unwrap(),
            "eng.understory.io"
        );
    }

    #[test]
    fn extract_domain_basic() {
        assert_eq!(
            extract_domain("kasper@understory.io"),
            Some("understory.io".into())
        );
        assert_eq!(
            extract_domain("KASPER@Understory.IO"),
            Some("understory.io".into())
        );
    }

    #[test]
    fn extract_domain_uses_rightmost_at() {
        // Permissive: rightmost @ wins. Inputs with multiple '@' are
        // pre-screened by add_user_email upstream; we still need a defined
        // behavior here.
        assert_eq!(
            extract_domain("weird@local@example.com"),
            Some("example.com".into())
        );
    }

    #[test]
    fn extract_domain_rejects_malformed() {
        assert_eq!(extract_domain("no-at-sign"), None);
        assert_eq!(extract_domain("trailing@"), None);
    }

    #[test]
    fn policy_round_trips() {
        for p in [
            AllowedDomainPolicy::AutoInviteAnyVerified,
            AllowedDomainPolicy::ManualOnly,
            AllowedDomainPolicy::AutoJoinOauth,
        ] {
            assert_eq!(AllowedDomainPolicy::parse(p.as_str()).unwrap(), p);
        }
    }

    #[test]
    fn policy_parse_rejects_unknown() {
        assert!(AllowedDomainPolicy::parse("something-else").is_err());
    }
}
