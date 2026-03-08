-- Forest consolidated schema
-- Merged from all incremental migrations into a single initial schema.

-- ═══════════════════════════════════════════════════════════════════
-- Extensions
-- ═══════════════════════════════════════════════════════════════════

CREATE EXTENSION IF NOT EXISTS pg_trgm;

-- ═══════════════════════════════════════════════════════════════════
-- Users & Auth
-- ═══════════════════════════════════════════════════════════════════

CREATE TABLE users (
    id UUID PRIMARY KEY NOT NULL,
    username TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX idx_username ON users(username);
CREATE INDEX idx_users_username_trgm ON users USING gin(username gin_trgm_ops);

CREATE TABLE user_emails (
    user_id UUID NOT NULL,
    email TEXT NOT NULL,
    verified BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, email)
);
CREATE UNIQUE INDEX idx_user_emails_unique ON user_emails(email);
CREATE INDEX idx_user_emails_email_trgm ON user_emails USING gin(email gin_trgm_ops);

CREATE TABLE identities (
    id UUID PRIMARY KEY NOT NULL,
    user_id UUID NOT NULL,
    provider TEXT NOT NULL,
    provider_user_id TEXT NOT NULL,
    provider_email TEXT,
    provider_data JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_identities_user_id ON identities(user_id);

CREATE TABLE provider_native_credentials (
    id UUID NOT NULL PRIMARY KEY,
    user_id UUID NOT NULL,
    password_hash BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX idx_provider_native_credentials_user_id ON provider_native_credentials(user_id);

CREATE TABLE provider_native_mfa (
    id UUID NOT NULL PRIMARY KEY,
    user_id UUID NOT NULL,
    type TEXT NOT NULL,
    secret BYTEA NOT NULL,
    verified BOOLEAN NOT NULL DEFAULT false,
    last_used_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX idx_provider_native_mfa_user_id ON provider_native_mfa(user_id);

CREATE TABLE sessions (
    id UUID PRIMARY KEY NOT NULL,
    user_id UUID NOT NULL,
    token_hash BYTEA NOT NULL,
    info JSONB,
    expires_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_sessions_user ON sessions(user_id);

CREATE TABLE provider_oauth_states (
    id UUID PRIMARY KEY NOT NULL,
    provider TEXT NOT NULL,
    state TEXT NOT NULL,
    redirect_uri TEXT,
    data JSONB NOT NULL,
    expires_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE personal_access_tokens (
    id UUID PRIMARY KEY NOT NULL,
    user_id UUID NOT NULL,
    name TEXT NOT NULL,
    token_hash BYTEA NOT NULL,
    scopes JSONB NOT NULL,
    expires_at TIMESTAMPTZ,
    last_used TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_personal_access_tokens_user ON personal_access_tokens(user_id);
CREATE INDEX idx_personal_access_tokens_user_hash ON personal_access_tokens(user_id, token_hash);

-- ═══════════════════════════════════════════════════════════════════
-- Organisations
-- ═══════════════════════════════════════════════════════════════════

CREATE TABLE organisations (
    id UUID PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX idx_organisations_name ON organisations(name);

CREATE TABLE organisation_members (
    organisation_id UUID NOT NULL,
    user_id UUID NOT NULL,
    role TEXT NOT NULL DEFAULT 'member',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (organisation_id, user_id)
);
CREATE INDEX idx_organisation_members_user ON organisation_members(user_id);

-- ═══════════════════════════════════════════════════════════════════
-- Apps (org-scoped integrations)
-- ═══════════════════════════════════════════════════════════════════

CREATE TABLE apps (
    id UUID PRIMARY KEY,
    organisation_id UUID NOT NULL REFERENCES organisations(id),
    name TEXT NOT NULL,
    description TEXT,
    permissions JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_by UUID NOT NULL REFERENCES users(id),
    suspended BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX idx_apps_org_name ON apps(organisation_id, name);

CREATE TABLE app_tokens (
    id UUID PRIMARY KEY,
    app_id UUID NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    token_hash BYTEA NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ,
    last_used TIMESTAMPTZ,
    revoked BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_app_tokens_app ON app_tokens(app_id);

-- ═══════════════════════════════════════════════════════════════════
-- Components
-- ═══════════════════════════════════════════════════════════════════

CREATE TABLE components (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL,
    organisation TEXT NOT NULL,
    version TEXT NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX idx_component_unique_version ON components (name, organisation, version);

CREATE TABLE component_staging (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL,
    organisation TEXT NOT NULL,
    version TEXT NOT NULL,
    status TEXT NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX idx_component_staging_unique_version ON component_staging (name, organisation, version);

CREATE TABLE component_files (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    component_id UUID NOT NULL,
    file_path TEXT NOT NULL,
    file_content BYTEA NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ═══════════════════════════════════════════════════════════════════
-- Projects & Environments
-- ═══════════════════════════════════════════════════════════════════

CREATE TABLE projects (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    organisation TEXT NOT NULL,
    project TEXT NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT fk_projects_organisation FOREIGN KEY (organisation) REFERENCES organisations(name)
);
CREATE INDEX idx_project_organisation ON projects (organisation);
CREATE UNIQUE INDEX idx_project_unique ON projects (organisation, project);

CREATE TABLE environments (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    organisation TEXT NOT NULL REFERENCES organisations(name),
    name TEXT NOT NULL,
    description TEXT,
    sort_order INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX idx_environments_org_name ON environments(organisation, name);
CREATE INDEX idx_environments_organisation ON environments(organisation);

-- ═══════════════════════════════════════════════════════════════════
-- Destinations
-- ═══════════════════════════════════════════════════════════════════

CREATE TABLE destinations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL,
    environment TEXT NOT NULL,
    organisation TEXT NOT NULL,
    type_organisation TEXT NOT NULL,
    type_name TEXT NOT NULL,
    type_version INTEGER NOT NULL,
    metadata JSONB NOT NULL,
    environment_id UUID NOT NULL REFERENCES environments(id),
    created TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT fk_destinations_organisation FOREIGN KEY (organisation) REFERENCES organisations(name)
);
CREATE UNIQUE INDEX idx_destinations_name ON destinations (name);
CREATE INDEX idx_destinations_environment ON destinations (environment);
CREATE INDEX idx_destinations_organisation ON destinations (organisation);
CREATE INDEX idx_destinations_environment_id ON destinations(environment_id);

-- ═══════════════════════════════════════════════════════════════════
-- Blob storage
-- ═══════════════════════════════════════════════════════════════════

CREATE TABLE blob_storage (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    content TEXT,
    created TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ═══════════════════════════════════════════════════════════════════
-- Artifacts & Staging
-- ═══════════════════════════════════════════════════════════════════

CREATE TABLE artifact_staging (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    artifact_id UUID NOT NULL,
    actor_id UUID,
    actor_type TEXT,
    created TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE artifact_files (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    artifact_staging_id UUID NOT NULL,
    env TEXT NOT NULL,
    destination TEXT NOT NULL,
    file_name TEXT NOT NULL,
    file_content UUID NOT NULL,
    category TEXT NOT NULL DEFAULT 'deployment',
    created TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE artifacts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    artifact_id UUID NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ═══════════════════════════════════════════════════════════════════
-- Annotations
-- ═══════════════════════════════════════════════════════════════════

CREATE TABLE annotations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    slug TEXT NOT NULL,
    artifact_id UUID NOT NULL,
    metadata JSONB NOT NULL,
    source JSONB NOT NULL,
    context JSONB NOT NULL,
    project_id UUID NOT NULL,
    ref JSONB NOT NULL,
    actor_id UUID,
    actor_type TEXT,
    created TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX idx_annotations_slug ON annotations (slug);
CREATE INDEX idx_annotations_project_created ON annotations (project_id, created DESC);

-- ═══════════════════════════════════════════════════════════════════
-- Release Intents
-- ═══════════════════════════════════════════════════════════════════

CREATE TABLE release_intents (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    artifact UUID NOT NULL,
    annotation_id UUID NOT NULL,
    project_id UUID NOT NULL,
    actor_id UUID,
    actor_type TEXT,
    stages JSONB,        -- DAG definition from client (null = single deploy, no pipeline)
    stage_states JSONB,  -- runtime state per node: { "node-id": { "status": "...", ... } }
    status TEXT NOT NULL DEFAULT 'ACTIVE',  -- ACTIVE, SUCCEEDED, FAILED, CANCELLED
    next_evaluate_at TIMESTAMPTZ,           -- when the coordinator should next evaluate this intent
    created TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_release_intent_project_id ON release_intents (project_id);
CREATE INDEX idx_release_intent_artifact ON release_intents (artifact);
CREATE INDEX idx_release_intents_active_evaluate
    ON release_intents (next_evaluate_at)
    WHERE status = 'ACTIVE';
CREATE UNIQUE INDEX idx_release_intents_active_artifact
    ON release_intents (artifact)
    WHERE status = 'ACTIVE' AND stages IS NOT NULL;

-- ═══════════════════════════════════════════════════════════════════
-- Event-sourced Release States
-- ═══════════════════════════════════════════════════════════════════

CREATE TABLE release_states (
    release_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    release_intent_id UUID NOT NULL REFERENCES release_intents(id),
    stage_id TEXT,  -- references a node id in release_intents.stages DAG (null = legacy)
    project_id UUID NOT NULL,
    destination_id UUID NOT NULL,
    artifact_id UUID NOT NULL,
    status TEXT NOT NULL DEFAULT 'QUEUED',
    runner_id TEXT,
    error_message TEXT,
    queued_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    assigned_at TIMESTAMPTZ,
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    last_heartbeat_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX idx_release_active
    ON release_states (project_id, destination_id)
    WHERE status IN ('ASSIGNED', 'RUNNING');

CREATE INDEX idx_release_queued
    ON release_states (queued_at) WHERE status = 'QUEUED';

CREATE INDEX idx_release_states_intent
    ON release_states (release_intent_id);

CREATE INDEX idx_release_states_stage
    ON release_states (stage_id);

CREATE INDEX idx_release_queue_position
    ON release_states (project_id, destination_id, queued_at ASC)
    WHERE status = 'QUEUED';

CREATE INDEX idx_release_states_project_status
    ON release_states (project_id, status);

CREATE INDEX idx_release_states_soak
    ON release_states (project_id, artifact_id, status)
    WHERE status = 'SUCCEEDED';

-- ═══════════════════════════════════════════════════════════════════
-- Release Events (append-only audit log)
-- ═══════════════════════════════════════════════════════════════════

CREATE TABLE release_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    release_id UUID NOT NULL REFERENCES release_states(release_id),
    sequence BIGSERIAL NOT NULL,
    event_type TEXT NOT NULL,
    payload JSONB NOT NULL DEFAULT '{}',
    actor_id UUID,
    actor_type TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_release_events_release_id ON release_events(release_id);
CREATE INDEX idx_release_events_sequence ON release_events(sequence);

-- ═══════════════════════════════════════════════════════════════════
-- Release Logs
-- ═══════════════════════════════════════════════════════════════════

CREATE TABLE release_logs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    release_attempt UUID NOT NULL,
    release_intent_id UUID NOT NULL REFERENCES release_intents(id),
    destination_id UUID NOT NULL,
    log_lines JSONB NOT NULL,
    sequence BIGSERIAL NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_release_logs_intent_destination ON release_logs (release_intent_id, destination_id);
CREATE UNIQUE INDEX idx_release_logs_intent_destination_sequence ON release_logs (release_attempt, release_intent_id, destination_id, sequence);

-- ═══════════════════════════════════════════════════════════════════
-- Release Tokens (runner authentication)
-- ═══════════════════════════════════════════════════════════════════

CREATE TABLE release_tokens (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    token_hash BYTEA NOT NULL UNIQUE,
    release_id UUID NOT NULL,
    release_intent_id UUID NOT NULL,
    artifact_id UUID NOT NULL,
    destination_id UUID NOT NULL,
    project_id UUID NOT NULL,
    runner_id TEXT NOT NULL,
    environment TEXT NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires TIMESTAMPTZ NOT NULL,
    revoked BOOLEAN NOT NULL DEFAULT false
);
CREATE INDEX idx_release_tokens_hash ON release_tokens (token_hash);
CREATE INDEX idx_release_tokens_runner ON release_tokens (runner_id) WHERE NOT revoked;

-- ═══════════════════════════════════════════════════════════════════
-- Notifications
-- ═══════════════════════════════════════════════════════════════════

CREATE TABLE notifications (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    sequence BIGSERIAL NOT NULL,
    notification_type TEXT NOT NULL,
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    organisation TEXT NOT NULL,
    project TEXT NOT NULL,
    release_context JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_notifications_sequence ON notifications (sequence);
CREATE INDEX idx_notifications_org_project ON notifications (organisation, project);

CREATE TABLE notification_preferences (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL,
    notification_type TEXT NOT NULL,
    channel TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX idx_notification_prefs_unique
    ON notification_preferences (user_id, notification_type, channel);

-- ═══════════════════════════════════════════════════════════════════
-- Triggers (auto-release on annotation match)
-- ═══════════════════════════════════════════════════════════════════

CREATE TABLE triggers (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT true,
    branch_pattern TEXT,
    title_pattern TEXT,
    author_pattern TEXT,
    commit_message_pattern TEXT,
    source_type_pattern TEXT,
    target_environments TEXT[] NOT NULL DEFAULT '{}',
    target_destinations TEXT[] NOT NULL DEFAULT '{}',
    force_release BOOLEAN NOT NULL DEFAULT false,
    use_pipeline BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(project_id, name)
);
CREATE INDEX idx_triggers_project ON triggers (project_id) WHERE enabled = true;

-- ═══════════════════════════════════════════════════════════════════
-- Policies (deployment guardrails: soak times, branch restrictions)
-- ═══════════════════════════════════════════════════════════════════

CREATE TABLE policies (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT true,
    policy_type TEXT NOT NULL,  -- 'soak_time' | 'branch_restriction'
    config JSONB NOT NULL,      -- type-specific configuration
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(project_id, name)
);
CREATE INDEX idx_policies_project ON policies (project_id) WHERE enabled = true;

-- ═══════════════════════════════════════════════════════════════════
-- Release Pipelines (reusable DAG recipes per project)
-- ═══════════════════════════════════════════════════════════════════

CREATE TABLE release_pipelines (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    stages JSONB NOT NULL,     -- DAG: { "stage-id": { "type": "deploy", "environment": "dev", "depends_on": [] }, ... }
    enabled BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(project_id, name)
);
CREATE INDEX idx_release_pipelines_project ON release_pipelines (project_id) WHERE enabled = true;

-- ═══════════════════════════════════════════════════════════════════
-- Organisation Events (outbox)
-- ═══════════════════════════════════════════════════════════════════

CREATE TABLE org_events (
    sequence BIGSERIAL PRIMARY KEY,
    event_id UUID NOT NULL DEFAULT gen_random_uuid(),
    organisation TEXT NOT NULL,
    project TEXT NOT NULL DEFAULT '',
    resource_type TEXT NOT NULL,
    action TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    metadata JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_org_events_org_seq ON org_events (organisation, sequence);
CREATE INDEX idx_org_events_project_seq ON org_events (organisation, project, sequence) WHERE project != '';

-- ═══════════════════════════════════════════════════════════════════
-- Event Subscriptions (durable cursors for third-party consumers)
-- ═══════════════════════════════════════════════════════════════════

CREATE TABLE event_subscriptions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    organisation TEXT NOT NULL REFERENCES organisations(name),
    name TEXT NOT NULL,

    -- Filters (empty array = all)
    resource_types TEXT[] NOT NULL DEFAULT '{}',
    actions TEXT[] NOT NULL DEFAULT '{}',
    projects TEXT[] NOT NULL DEFAULT '{}',

    -- Cursor: last acknowledged org_events.sequence
    cursor BIGINT NOT NULL DEFAULT 0,

    -- Status
    status TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'paused')),

    -- Ownership
    created_by_app_id UUID REFERENCES apps(id),
    created_by_user_id UUID REFERENCES users(id),

    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    UNIQUE(organisation, name)
);
CREATE INDEX idx_event_subscriptions_active
    ON event_subscriptions (organisation) WHERE status = 'active';
