-- Auto-invite to organisations based on verified email domain (DATA-252).
--
-- An org admin lists email domains whose verified-email holders are eligible
-- for self-service join. v1 only emits "invite offers" the user must accept.
--
-- dns_verification_token / dns_verified_at are reserved for v1.1 (DNS TXT
-- ownership proof). They're populated/ignored as appropriate by v1; the
-- schema is forward-compatible so v1.1 lands without another migration.
--
-- policy values:
--   'auto_invite_any_verified' — v1 default; surface a join offer the user
--                                 must explicitly accept.
--   'manual_only'              — entry is informational only; no offers
--                                 surfaced. Lets admins stage future domains
--                                 without granting access.
--   'auto_join_oauth'          — v1.1; silent JIT for users whose email was
--                                 verified via OAuth at a DNS-verified
--                                 domain. Rejected by v1 service code.

CREATE TABLE organisation_allowed_domains (
    organisation_id        UUID        NOT NULL REFERENCES organisations(id) ON DELETE CASCADE,
    domain                 TEXT        NOT NULL,
    policy                 TEXT        NOT NULL DEFAULT 'auto_invite_any_verified',
    dns_verification_token TEXT        NOT NULL,
    dns_verified_at        TIMESTAMPTZ,
    created_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by             UUID        NOT NULL,
    PRIMARY KEY (organisation_id, domain)
);

-- Hot path: given a verified email's domain, find orgs that allow it.
CREATE INDEX organisation_allowed_domains_by_domain
    ON organisation_allowed_domains (domain);

-- Track how each user-email's verified bit was set. v1 only records the
-- value (defaulting historical rows to 'magic_link'); v1.1 uses it to gate
-- silent JIT to OAuth-sourced emails only.
ALTER TABLE user_emails
    ADD COLUMN verification_source TEXT NOT NULL DEFAULT 'magic_link';
