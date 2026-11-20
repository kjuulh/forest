-- Public OAuth clients (RFC 8252 native apps).
--
-- A CLI such as `datactl` signs a person in with Forest through the
-- authorization-code flow, but it ships to every laptop and cannot keep a
-- client_secret. A public app has no usable secret: its
-- `client_secret_hash` is the hash of a random value that was never
-- returned to anyone, so the secret check cannot be satisfied and is
-- skipped instead. What binds the code to the client that asked for it is
-- a mandatory PKCE S256 challenge, checked at issue and at exchange.
--
-- A public app may only use `authorization_code`, and may not hold
-- `openid` (its id_token would be signed with the secret it does not
-- have). Both are enforced in the service, not here.
--
-- Existing apps are confidential and stay so.
ALTER TABLE oauth_apps
    ADD COLUMN IF NOT EXISTS public BOOLEAN NOT NULL DEFAULT false;
