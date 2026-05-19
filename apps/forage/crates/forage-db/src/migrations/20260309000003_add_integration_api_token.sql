ALTER TABLE integrations ADD COLUMN api_token_hash TEXT;
CREATE UNIQUE INDEX idx_integrations_api_token ON integrations(api_token_hash) WHERE api_token_hash IS NOT NULL;
