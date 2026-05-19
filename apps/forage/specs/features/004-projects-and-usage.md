# 004 - Projects View & Usage/Pricing

**Status**: Phase 1 - Spec
**Depends on**: 003 (BFF Sessions)

## Problem

The dashboard currently shows placeholder text ("No projects yet"). Authenticated users need to:

1. See their organisations and projects (pulled from forest-server via gRPC)
2. Understand their current usage and plan limits
3. Navigate between organisations and their projects

The pricing page exists but is disconnected from the authenticated experience - there's no "your current plan" or usage visibility.

## Scope

This spec covers:
- **Projects view**: List organisations -> projects for the authenticated user
- **Usage view**: Show current plan, resource usage, and upgrade path
- **gRPC integration**: Add OrganisationService and ReleaseService clients
- **Navigation**: Authenticated sidebar/nav with org switcher

Out of scope (future specs):
- Creating organisations or projects from the UI (CLI-first)
- Billing/Stripe integration
- Deployment management (viewing releases, logs)

## Architecture

### New gRPC Services

We need to generate stubs for and integrate:
- `OrganisationService.ListMyOrganisations` - get orgs the user belongs to
- `ReleaseService.GetProjects` - get projects within an org
- `ReleaseService.GetArtifactsByProject` - get recent releases for a project

These require copying `organisations.proto` and `releases.proto` into `interface/proto/forest/v1/` and regenerating with buf.

### New Trait: `ForestPlatform`

Separate from `ForestAuth` (which handles identity), this trait handles platform data:

```rust
#[async_trait]
pub trait ForestPlatform: Send + Sync {
    async fn list_my_organisations(
        &self,
        access_token: &str,
    ) -> Result<Vec<Organisation>, PlatformError>;

    async fn list_projects(
        &self,
        access_token: &str,
        organisation: &str,
    ) -> Result<Vec<String>, PlatformError>;

    async fn list_artifacts(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
    ) -> Result<Vec<Artifact>, PlatformError>;
}
```

### Domain Types (forage-core)

```rust
// forage-core::platform

pub struct Organisation {
    pub organisation_id: String,
    pub name: String,
    pub role: String,       // user's role in this org
}

pub struct Artifact {
    pub artifact_id: String,
    pub slug: String,
    pub context: ArtifactContext,
    pub created_at: String,
}

pub struct ArtifactContext {
    pub title: String,
    pub description: Option<String>,
}

#[derive(thiserror::Error)]
pub enum PlatformError {
    #[error("not authenticated")]
    NotAuthenticated,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("service unavailable: {0}")]
    Unavailable(String),
    #[error("{0}")]
    Other(String),
}
```

### Routes

| Route | Auth | Description |
|-------|------|-------------|
| `GET /dashboard` | Required | Redirect to first org's projects, or onboarding if no orgs |
| `GET /orgs/{org}/projects` | Required | List projects for an organisation |
| `GET /orgs/{org}/projects/{project}` | Required | Project detail: recent artifacts/releases |
| `GET /orgs/{org}/usage` | Required | Usage & plan info for the organisation |

### Templates

- `pages/projects.html.jinja` - Project list within an org
- `pages/project_detail.html.jinja` - Single project with recent artifacts
- `pages/usage.html.jinja` - Usage dashboard with plan info
- `components/app_nav.html.jinja` - Authenticated navigation with org switcher

### Authenticated Navigation

When logged in, replace the marketing nav with an app nav:
- Left: forage logo, org switcher dropdown
- Center: Projects, Usage links (scoped to current org)
- Right: user menu (settings, tokens, sign out)

The base template needs to support both modes: marketing (unauthenticated) and app (authenticated).

## Behavioral Contract

### Dashboard redirect
- Authenticated user with orgs -> redirect to `/orgs/{first_org}/projects`
- Authenticated user with no orgs -> show onboarding: "Create your first organisation with the forest CLI"
- Unauthenticated -> redirect to `/login` (existing behavior)

### Projects list
- Shows all projects in the organisation
- Each project shows: name, latest artifact slug, last deploy time
- Empty state: "No projects yet. Deploy with `forest release create`"
- User must be a member of the org (403 otherwise)

### Project detail
- Shows project name, recent artifacts (last 10)
- Each artifact: slug, title, description, created_at
- Empty state: "No releases yet"

### Usage page
- Current plan tier (hardcoded to "Early Access - Free" for now)
- Resource summary (placeholder - no real metering yet)
- "Upgrade" CTA pointing to pricing page
- Early access notice

## Test Strategy

### Unit tests (forage-core) - ~6 tests
- PlatformError display strings
- Organisation/Artifact type construction

### Integration tests (forage-server) - ~12 tests
- Dashboard redirect: authenticated with orgs -> redirect to first org
- Dashboard redirect: authenticated no orgs -> onboarding page
- Projects list: returns 200 with projects
- Projects list: empty org shows empty state
- Projects list: unauthenticated -> redirect to login
- Project detail: returns 200 with artifacts
- Project detail: unknown project -> 404
- Usage page: returns 200 with plan info
- Usage page: unauthenticated -> redirect to login
- Forest-server unavailable -> error page
- Org switcher: nav shows user's organisations
- Non-member org access -> 403

## Implementation Order

1. Copy protos, regenerate stubs (buf generate)
2. Add domain types and `ForestPlatform` trait to forage-core
3. Write failing tests (Red)
4. Implement `GrpcForestPlatform` in forage-server
5. Add `MockForestPlatform` to tests
6. Implement routes and templates (Green)
7. Update dashboard redirect logic
8. Add authenticated nav component
9. Clippy + review (Phase 3)

## Open Questions

- Should org switcher persist selection in session or always default to first org?
- Do we want a `/orgs/{org}/settings` page in this spec or defer?
