-- Device authorization grants (RFC 8628) for an OAuth app.
--
-- `forest auth login` already uses `device_login_grants`: the person approves
-- a code in Forage's /device page and the Forest CLI receives a Forest
-- session. A CLI that signs in to an *app* (datactl, for the data gateway)
-- uses the same grants, bound to that app. Such a grant yields the app's
-- scoped OAuth tokens, never a Forest session, and only the app it is bound
-- to can redeem it.
--
-- NULL is a Forest CLI login, which every existing row is. Deleting the app
-- deletes its outstanding grants.
ALTER TABLE device_login_grants
    ADD COLUMN IF NOT EXISTS app_id UUID REFERENCES oauth_apps(id) ON DELETE CASCADE;
