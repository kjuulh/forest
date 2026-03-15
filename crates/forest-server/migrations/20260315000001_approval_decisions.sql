CREATE TABLE approval_decisions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    release_intent_id UUID NOT NULL REFERENCES release_intents(id) ON DELETE CASCADE,
    policy_id UUID NOT NULL REFERENCES policies(id) ON DELETE CASCADE,
    target_environment TEXT NOT NULL,
    user_id UUID NOT NULL,
    username TEXT NOT NULL,
    decision TEXT NOT NULL CHECK (decision IN ('approved', 'rejected')),
    comment TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX idx_approval_decisions_unique
    ON approval_decisions (release_intent_id, target_environment, user_id);

CREATE INDEX idx_approval_decisions_lookup
    ON approval_decisions (release_intent_id, target_environment, decision);
