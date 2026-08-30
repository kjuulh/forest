CREATE TABLE organisation_rule_sets (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    organisation TEXT NOT NULL,
    name TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT true,
    selector JSONB NOT NULL DEFAULT '{}'::jsonb,
    policies JSONB NOT NULL DEFAULT '[]'::jsonb,
    triggers JSONB NOT NULL DEFAULT '[]'::jsonb,
    release_pipelines JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT fk_org_rule_sets_organisation
        FOREIGN KEY (organisation) REFERENCES organisations(name) ON DELETE CASCADE,
    UNIQUE (organisation, name)
);

CREATE INDEX idx_org_rule_sets_organisation
    ON organisation_rule_sets (organisation);

CREATE TABLE organisation_rule_set_materializations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    rule_set_id UUID NOT NULL REFERENCES organisation_rule_sets(id) ON DELETE CASCADE,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    resource_type TEXT NOT NULL CHECK (resource_type IN ('policy', 'trigger', 'release_pipeline')),
    resource_name TEXT NOT NULL,
    resource_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (rule_set_id, project_id, resource_type, resource_name),
    UNIQUE (resource_type, resource_id)
);

CREATE INDEX idx_org_rule_set_materializations_rule_set
    ON organisation_rule_set_materializations (rule_set_id);

CREATE INDEX idx_org_rule_set_materializations_project
    ON organisation_rule_set_materializations (project_id);
