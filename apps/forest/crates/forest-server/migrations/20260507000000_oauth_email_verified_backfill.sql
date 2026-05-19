-- Backfill: emails belonging to users who only have non-native (OAuth)
-- identities should be marked verified. The OAuth provider already
-- vouched for the address at signup; today's `register_oauth_user`
-- erroneously inserts the email row with verified=false, which would
-- lock these users out the moment FOREST_REQUIRE_EMAIL_VERIFICATION
-- flips on. The companion code change (services/users.rs) inserts
-- new OAuth-signup emails with verified=true going forward; this
-- migration cleans up the pre-existing rows.
UPDATE user_emails ue
SET verified = TRUE
WHERE verified = FALSE
  AND NOT EXISTS (
    SELECT 1 FROM identities i
    WHERE i.user_id = ue.user_id AND i.provider = 'native'
  )
  AND EXISTS (
    SELECT 1 FROM identities i
    WHERE i.user_id = ue.user_id
  );
