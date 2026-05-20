-- Add uniqueness constraints to identities so a given external account
-- (provider, provider_user_id) can only belong to one Forest user, and so
-- a given user has at most one identity per provider. Without these,
-- LinkOAuthProvider could create duplicate rows that make
-- find_user_by_oauth non-deterministic (cross-user account takeover).
--
-- Defensive de-duplication first: keep the oldest row for each conflicting
-- group. In a healthy production database these DELETEs are no-ops.

-- 1. Collapse duplicate (provider, provider_user_id) rows. Keep the row
--    with the lowest id (UUIDv7 → earliest creation time).
DELETE FROM identities a
USING identities b
WHERE a.provider = b.provider
  AND a.provider_user_id = b.provider_user_id
  AND a.id > b.id;

-- 2. Collapse duplicate (user_id, provider) rows. Same tie-break: keep oldest.
DELETE FROM identities a
USING identities b
WHERE a.user_id = b.user_id
  AND a.provider = b.provider
  AND a.id > b.id;

-- 3. Enforce going forward.
CREATE UNIQUE INDEX identities_provider_external_id_key
    ON identities (provider, provider_user_id);

CREATE UNIQUE INDEX identities_user_provider_key
    ON identities (user_id, provider);
