CREATE TABLE IF NOT EXISTS integrations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    organisation TEXT NOT NULL,
    integration_type TEXT NOT NULL,
    name TEXT NOT NULL,
    config_encrypted BYTEA NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT true,
    created_by TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(organisation, name)
);

CREATE INDEX idx_integrations_org ON integrations(organisation);
CREATE INDEX idx_integrations_org_enabled ON integrations(organisation, enabled);

CREATE TABLE IF NOT EXISTS notification_rules (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    integration_id UUID NOT NULL REFERENCES integrations(id) ON DELETE CASCADE,
    notification_type TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT true,
    UNIQUE(integration_id, notification_type)
);

CREATE INDEX idx_notification_rules_integration ON notification_rules(integration_id);

CREATE TABLE IF NOT EXISTS notification_deliveries (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    integration_id UUID NOT NULL REFERENCES integrations(id) ON DELETE CASCADE,
    notification_id TEXT NOT NULL,
    status TEXT NOT NULL,
    error_message TEXT,
    attempted_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_deliveries_integration ON notification_deliveries(integration_id, attempted_at DESC);
CREATE INDEX idx_deliveries_status ON notification_deliveries(status, attempted_at DESC);
