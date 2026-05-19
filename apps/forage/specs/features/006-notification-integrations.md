# 006 - Notification Integrations

**Status**: Phase 1 - Spec Crystallisation
**Depends on**: 005 (Dashboard Enhancement)

## Problem

Users can toggle notification preferences (event type × channel) on their account page, but:

1. **No delivery**: Forest fires events via `ListenNotifications` gRPC stream, but Forage doesn't consume them or route them anywhere.
2. **Fixed channels**: The current toggle matrix (CLI, Slack columns) doesn't scale beyond 2 channels. Adding Discord, webhooks, PagerDuty, email, etc. makes the table too wide.
3. **No integration config**: There's no way to connect a Slack workspace, set a webhook URL, or configure any third-party channel.
4. **Wrong ownership**: The current proto has `NotificationChannel` as a fixed enum on forest-server. But channel routing is a Forage premium feature — Forest should only fire events, Forage decides where to route them.

## Separation of Concerns

**Forest** (upstream gRPC server):
- Fires notification events when releases are annotated, started, succeed, or fail
- Exposes `ListenNotifications` (server-streaming) and `ListNotifications` (paginated) RPCs
- Knows nothing about Slack, Discord, webhooks, or any delivery channel
- Stores/returns notification preferences as opaque data (channel is just a string/enum from Forage's perspective)

**Forage** (this codebase — the BFF):
- Subscribes to Forest's `ListenNotifications` stream for each connected org
- Maintains its own integration registry: which org has which channels configured
- Routes notifications to the appropriate channels based on org integrations + user preferences
- Manages third-party OAuth flows (Slack), webhook URLs, API keys
- Gates channel availability behind org plan/premium features
- Displays notification history to users via web UI and CLI API

This means Forage needs its own persistence for integrations — not stored in Forest.

## Scope

This spec covers:
- **Integration model**: org-level integrations stored in Forage's database
- **Notification listener**: background service consuming Forest's `ListenNotifications` stream
- **Notification routing**: dispatching notifications to configured integrations
- **Slack integration**: OAuth setup, message formatting, delivery
- **Webhook integration**: generic outbound webhook with configurable URL
- **Redesigned preferences UI**: per-integration notification rules (not a fixed matrix)
- **Notification history page**: paginated list using `ListNotifications` RPC
- **CLI notification API**: JSON endpoint for CLI consumption

Out of scope:
- Discord, PagerDuty, email (future integrations — the model supports them)
- Per-project notification filtering (future enhancement)
- Billing/plan gating logic (assumes all orgs have access for now)
- Real-time browser push (SSE/WebSocket to browser — future enhancement)

## Architecture

### Integration Model

Integrations are org-scoped resources stored in Forage's PostgreSQL database.

```rust
/// An org-level notification integration (e.g., a Slack workspace, a webhook URL).
pub struct Integration {
    pub id: String,                    // UUID
    pub organisation: String,          // org name
    pub integration_type: String,      // "slack", "webhook", "cli"
    pub name: String,                  // user-given label, e.g. "#deploys"
    pub config: IntegrationConfig,     // type-specific config (encrypted at rest)
    pub enabled: bool,
    pub created_by: String,            // user_id
    pub created_at: String,
    pub updated_at: String,
}

pub enum IntegrationConfig {
    Slack {
        team_id: String,
        team_name: String,
        channel_id: String,
        channel_name: String,
        access_token: String,          // encrypted, from OAuth
        webhook_url: String,           // incoming webhook URL
    },
    Webhook {
        url: String,
        secret: Option<String>,        // HMAC signing secret
        headers: HashMap<String, String>,
    },
}
```

**CLI is special**: CLI notifications use Forest's `ListNotifications` RPC directly — there's no org-level integration for CLI. Users just call the API and get their notifications. CLI preference toggles remain per-user on Forest's side.

### Notification Rules

Each integration has notification rules that control which event types trigger it:

```rust
/// Which event types an integration should receive.
pub struct NotificationRule {
    pub integration_id: String,
    pub notification_type: String,     // e.g., "release_failed", "release_succeeded"
    pub enabled: bool,
}
```

Default: new integrations receive all event types. Users can disable specific types per integration.

### Notification Listener (Background Service)

A background tokio task in Forage that:

1. On startup, connects to Forest's `ListenNotifications` for each org with active integrations
2. When a notification arrives, looks up the org's enabled integrations
3. For each integration with a matching notification rule, dispatches via the appropriate channel
4. Handles reconnection on stream failure (exponential backoff)
5. Logs delivery success/failure for audit

```
Forest gRPC stream  ──►  Forage Listener  ──►  Integration Router  ──►  Slack API
                                                                    ──►  Webhook POST
                                                                    ──►  (future channels)
```

The listener runs as part of the Forage server process (not a separate service). It uses the org's admin access token (or a service token) to authenticate with Forest.

### Database Schema

```sql
CREATE TABLE integrations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    organisation TEXT NOT NULL,
    integration_type TEXT NOT NULL,        -- 'slack', 'webhook'
    name TEXT NOT NULL,
    config_encrypted BYTEA NOT NULL,       -- JSON config, encrypted with app key
    enabled BOOLEAN NOT NULL DEFAULT true,
    created_by TEXT NOT NULL,              -- user_id
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(organisation, name)
);

CREATE TABLE notification_rules (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    integration_id UUID NOT NULL REFERENCES integrations(id) ON DELETE CASCADE,
    notification_type TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT true,
    UNIQUE(integration_id, notification_type)
);

CREATE TABLE notification_deliveries (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    integration_id UUID NOT NULL REFERENCES integrations(id) ON DELETE CASCADE,
    notification_id TEXT NOT NULL,          -- from Forest
    status TEXT NOT NULL,                   -- 'delivered', 'failed', 'pending'
    error_message TEXT,
    attempted_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_integrations_org ON integrations(organisation);
CREATE INDEX idx_deliveries_integration ON notification_deliveries(integration_id, attempted_at DESC);
```

### Routes

| Route | Method | Auth | Description |
|-------|--------|------|-------------|
| `GET /orgs/{org}/settings/integrations` | GET | Required + admin | List integrations for org |
| `POST /orgs/{org}/settings/integrations/slack` | POST | Required + admin + CSRF | Start Slack OAuth flow |
| `GET /orgs/{org}/settings/integrations/slack/callback` | GET | Required | Slack OAuth callback |
| `POST /orgs/{org}/settings/integrations/webhook` | POST | Required + admin + CSRF | Create webhook integration |
| `GET /orgs/{org}/settings/integrations/{id}` | GET | Required + admin | Integration detail + rules |
| `POST /orgs/{org}/settings/integrations/{id}/rules` | POST | Required + admin + CSRF | Update notification rules |
| `POST /orgs/{org}/settings/integrations/{id}/test` | POST | Required + admin + CSRF | Send test notification |
| `POST /orgs/{org}/settings/integrations/{id}/toggle` | POST | Required + admin + CSRF | Enable/disable integration |
| `POST /orgs/{org}/settings/integrations/{id}/delete` | POST | Required + admin + CSRF | Delete integration |
| `GET /notifications` | GET | Required | Notification history (paginated) |
| `GET /api/notifications` | GET | Bearer token | JSON notification list for CLI |

### Templates

| Template | Status | Description |
|----------|--------|-------------|
| `pages/integrations.html.jinja` | New | Integration list: cards per integration, "Add" buttons |
| `pages/integration_detail.html.jinja` | New | Single integration: status, notification rules toggles, test/delete |
| `pages/integration_slack_setup.html.jinja` | New | Slack OAuth success/error result page |
| `pages/integration_webhook_form.html.jinja` | New | Webhook URL + secret + headers form |
| `pages/notifications.html.jinja` | Rewrite | Use `ListNotifications` RPC instead of manual assembly |
| `pages/account.html.jinja` | Update | Replace channel matrix with CLI-only toggles + link to org integrations |
| `base.html.jinja` | Update | Add "Integrations" tab under org-level nav |

### Account Settings Redesign

The current 4×2 toggle matrix becomes:

**Personal notifications (CLI)**
A single column of toggles for CLI event types (these are stored on Forest via the existing preference RPCs):

| Event | CLI |
|-------|-----|
| Release annotated | toggle |
| Release started | toggle |
| Release succeeded | toggle |
| Release failed | toggle |

Below: a link to `/orgs/{org}/settings/integrations` — "Configure Slack, webhooks, and other channels for your organisation."

### Integrations Page Layout

```
Integrations
Configure where your organisation receives deployment notifications.

[+ Add Slack] [+ Add Webhook]

┌─────────────────────────────────────────────────┐
│ 🔵 Slack · #deploys · rawpotion workspace       │
│ Receives: all events                   [Manage] │
└─────────────────────────────────────────────────┘
┌─────────────────────────────────────────────────┐
│ 🟢 Webhook · Production alerts                   │
│ Receives: release_failed only          [Manage] │
└─────────────────────────────────────────────────┘
```

### Integration Detail Page

```
Slack · #deploys
Status: Active ✓

Notification rules:
  Release annotated   [on]
  Release started     [on]
  Release succeeded   [on]
  Release failed      [on]

[Send test notification]  [Disable]  [Delete]
```

### Slack OAuth Flow

1. Admin clicks "Add Slack" → `POST /orgs/{org}/settings/integrations/slack` with CSRF
2. Server generates OAuth state (CSRF + org), stores in session, redirects to:
   `https://slack.com/oauth/v2/authorize?client_id=...&scope=assistant:write,channels:join,chat:write,chat:write.public,im:history,im:read,im:write,incoming-webhook,links:read,links:write,reactions:write,users:read,users:read.email&redirect_uri=...&state=...`
3. User authorizes in Slack
4. Slack redirects to `GET /orgs/{org}/settings/integrations/slack/callback?code=...&state=...`
5. Server validates state, exchanges code for access token via Slack API
6. Stores integration in database (token encrypted at rest)
7. Redirects to integration detail page

**Environment variables:**
- `SLACK_CLIENT_ID` — Slack app client ID
- `SLACK_CLIENT_SECRET` — Slack app client secret (encrypted/from secrets manager)
- `FORAGE_BASE_URL` — Base URL for OAuth callbacks (e.g., `https://forage.sh`)
- `INTEGRATION_ENCRYPTION_KEY` — AES-256 key for encrypting integration configs at rest

### Webhook Delivery Format

```json
{
  "event": "release_failed",
  "timestamp": "2026-03-09T14:30:00Z",
  "organisation": "rawpotion",
  "project": "service-example",
  "release": {
    "slug": "evidently-assisting-ladybeetle",
    "artifact_id": "art_123",
    "title": "fix: resolve OOM on large payload deserialization (#603)",
    "destination": "prod-eu",
    "environment": "production",
    "source_username": "hermansen",
    "commit_sha": "abc1234",
    "commit_branch": "main",
    "error_message": "container health check timeout after 120s"
  }
}
```

Webhooks include `X-Forage-Signature` header (HMAC-SHA256 of body with the webhook's secret) for verification.

### Slack Message Format

Slack messages use Block Kit for rich formatting:

- **Release succeeded**: green sidebar, title, commit, destination, link to release page
- **Release failed**: red sidebar, title, error message, commit, link to release page
- **Release started**: neutral, title, destination, link to release page
- **Release annotated**: neutral, title, description, link to release page

## Behavioral Contract

### Integrations page
- Only org admins/owners can view and manage integrations
- Non-admin members get 403
- Non-members get 403
- Lists all integrations for the org with status badges

### Slack integration setup
- CSRF protection on the initiation POST
- OAuth state validated on callback (prevents CSRF via Slack redirect)
- If Slack returns error, show error page with "Try again" button
- Duplicate channel detection: warn if same channel already configured

### Webhook integration
- URL must be HTTPS (except localhost for development)
- Secret is optional but recommended
- Test delivery on creation to validate the URL responds

### Notification routing
- Only enabled integrations with matching rules receive notifications
- Delivery failures are logged but don't block other integrations
- Retry: 3 attempts with exponential backoff (1s, 5s, 25s)
- After 3 failures, log error but don't disable integration

### Notification history
- Paginated, newest first, 20 per page
- Filterable by org and project (optional)
- Accessible to all authenticated users (scoped to their orgs)

### CLI API
- Authenticates via `Authorization: Bearer <personal_access_token>`
- Returns JSON `{ notifications: [...], next_page_token: "..." }`
- Token auth bypasses session — direct proxy to Forest's `ListNotifications` RPC

### Account settings
- CLI toggles remain per-user, stored on Forest
- Link to org integrations page for channel configuration

## Implementation Order

### Phase A: Database + Integration Model
1. Add `integrations`, `notification_rules`, `notification_deliveries` tables to `forage-db`
2. Add domain types to `forage-core` (`Integration`, `IntegrationConfig`, `NotificationRule`)
3. Add repository trait + Postgres implementation for CRUD operations
4. Unit tests for model validation

### Phase B: Integrations CRUD Routes + UI
1. Add `/orgs/{org}/settings/integrations` routes (list, detail, toggle, delete)
2. Add webhook creation form + route
3. Templates: integrations list, detail, webhook form
4. Update `base.html.jinja` nav with "Integrations" tab
5. Tests: CRUD operations, auth checks, CSRF validation

### Phase C: Slack OAuth
1. Add Slack OAuth initiation + callback routes
2. Slack API token exchange (reqwest call to `slack.com/api/oauth.v2.access`)
3. Store encrypted config in database
4. Template: success/error pages
5. Tests: mock OAuth flow, state validation

### Phase D: Notification Listener + Router
1. Background task: subscribe to Forest `ListenNotifications` for active orgs
2. Notification router: match notification to integrations + rules
3. Slack dispatcher: format Block Kit message, POST to Slack API
4. Webhook dispatcher: POST JSON payload with HMAC signature
5. Delivery logging to `notification_deliveries` table
6. Tests: routing logic, retry behavior, delivery recording

### Phase E: Notification History + CLI API
1. Rewrite `/notifications` to use `ListNotifications` RPC
2. Add `GET /api/notifications` JSON endpoint with bearer auth
3. Template: paginated notification list with filters
4. Tests: pagination, auth, JSON response shape

### Phase F: Account Settings Redesign
1. Simplify notification prefs to CLI-only toggles
2. Add link to org integrations page
3. Update tests for new layout

## Test Strategy

~35 new tests:

**Integration CRUD (10)**:
- List integrations returns 200 for admin
- List integrations returns 403 for non-admin member
- List integrations returns 403 for non-member
- Create webhook integration with valid URL
- Create webhook rejects HTTP URL (non-HTTPS)
- Create webhook validates CSRF
- Toggle integration on/off
- Delete integration with CSRF
- Update notification rules for integration
- Integration detail returns 404 for wrong org

**Slack OAuth (5)**:
- Slack initiation redirects to slack.com with correct params
- Slack callback with valid state creates integration
- Slack callback with invalid state returns 403
- Slack callback with error param shows error page
- Duplicate Slack channel shows warning

**Notification routing (8)**:
- Router dispatches to enabled integration with matching rule
- Router skips disabled integration
- Router skips integration with disabled rule for event type
- Router handles delivery failure gracefully (doesn't panic)
- Webhook dispatcher includes HMAC signature
- Slack dispatcher formats Block Kit correctly
- Retry logic attempts 3 times on failure
- Delivery logged to database

**Notification history (5)**:
- Notification page returns 200 with entries
- Notification page supports pagination
- CLI API returns JSON with bearer auth
- CLI API rejects unauthenticated request
- CLI API returns empty list gracefully

**Account settings (3)**:
- Account page shows CLI-only toggles
- Account page links to org integrations
- CLI toggle round-trip works

## Verification

- `cargo test` — all existing + new tests pass
- `cargo clippy` — clean
- `sqlx migrate` — new tables created without error
- Manual: create webhook integration, trigger release, verify delivery
- Manual: Slack OAuth flow end-to-end (requires Slack app credentials)
