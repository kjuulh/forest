-- An organisation can be public: anyone signed in may then read every
-- component it publishes. Membership is still required for any write.
ALTER TABLE organisations
    ADD COLUMN IF NOT EXISTS visibility TEXT NOT NULL DEFAULT 'private'
    CHECK (visibility IN ('public', 'private'));
