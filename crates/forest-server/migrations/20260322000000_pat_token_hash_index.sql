-- Add an index on token_hash alone so the auth layer can resolve PATs
-- without knowing the user_id upfront (matching how app_tokens work).
CREATE INDEX idx_personal_access_tokens_hash ON personal_access_tokens(token_hash);
