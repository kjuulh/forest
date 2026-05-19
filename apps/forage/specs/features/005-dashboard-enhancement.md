# 005 - Enhanced Dashboard & Org Management

**Status**: Phase 2 - Implementation
**Depends on**: 004 (Projects and Usage)

## Problem

The dashboard is a redirect stub. Projects pages show bare names with no release detail. There is no way to create organisations or manage members from the web UI. Users must use the CLI for everything.

## Scope

This spec covers:
- **Proper dashboard page**: GitHub-inspired layout with org sidebar and recent activity feed
- **Create organisation**: POST form on dashboard/onboarding
- **Richer artifact detail**: Show git ref, source, version, destinations on project detail
- **Org members page**: Read-only member list with roles
- **Org member management**: Add/remove members, update roles (admin-only)

Out of scope:
- Billing/Stripe integration
- Deployment logs/streaming
- Component registry browsing

## Architecture

### Domain Model Changes (forage-core)

Expand `Artifact` with source, ref, and destination data:

```rust
pub struct Artifact {
    pub artifact_id: String,
    pub slug: String,
    pub context: ArtifactContext,
    pub source: Option<ArtifactSource>,
    pub git_ref: Option<ArtifactRef>,
    pub destinations: Vec<ArtifactDestination>,
    pub created_at: String,
}

pub struct ArtifactSource {
    pub user: Option<String>,
    pub email: Option<String>,
    pub source_type: Option<String>,
    pub run_url: Option<String>,
}

pub struct ArtifactRef {
    pub commit_sha: String,
    pub branch: Option<String>,
    pub commit_message: Option<String>,
    pub version: Option<String>,
    pub repo_url: Option<String>,
}

pub struct ArtifactDestination {
    pub name: String,
    pub environment: String,
}

pub struct OrgMember {
    pub user_id: String,
    pub username: String,
    pub role: String,
    pub joined_at: Option<String>,
}
```

Add `organisation_id` to `CachedOrg` for member operations.

New `ForestPlatform` trait methods:
- `create_organisation(access_token, name) -> Result<String, PlatformError>`
- `list_members(access_token, organisation_id) -> Result<Vec<OrgMember>, PlatformError>`
- `add_member(access_token, organisation_id, user_id, role) -> Result<OrgMember, PlatformError>`
- `remove_member(access_token, organisation_id, user_id) -> Result<(), PlatformError>`
- `update_member_role(access_token, organisation_id, user_id, role) -> Result<OrgMember, PlatformError>`

### Routes

| Route | Method | Auth | Description |
|-------|--------|------|-------------|
| `GET /dashboard` | GET | Required | Proper page: org sidebar + recent activity feed |
| `POST /orgs` | POST | Required + CSRF | Create organisation, redirect to new org |
| `GET /orgs/{org}/settings/members` | GET | Required | Members list |
| `POST /orgs/{org}/settings/members` | POST | Required + CSRF | Add member (admin-only) |
| `POST /orgs/{org}/settings/members/{user_id}/role` | POST | Required + CSRF | Update role (admin-only) |
| `POST /orgs/{org}/settings/members/{user_id}/remove` | POST | Required + CSRF | Remove member (admin-only) |

### Templates

- `pages/dashboard.html.jinja` - Rewrite: org sidebar + activity feed + create org form
- `pages/project_detail.html.jinja` - Enhance: git ref, source, destinations
- `pages/onboarding.html.jinja` - Enhance: add create org form
- `pages/members.html.jinja` - New: members table with admin actions
- `base.html.jinja` - Add Settings/Members nav link

### Dashboard Data Flow

For each cached org, call `list_projects`, then `list_artifacts` for the first few projects. Cap at 10 total artifacts. Use `tokio::join!` for parallelism.

## Behavioral Contract

### Dashboard
- Authenticated with orgs: show dashboard page with org sidebar and recent activity
- Authenticated no orgs: show onboarding with create org form
- Recent activity: up to 10 artifacts across all orgs, newest first

### Create organisation
- Validates name with `validate_slug()`
- CSRF protection
- On success: cache new org in session, redirect to `/orgs/{name}/projects`
- On duplicate name: show error on form

### Members page
- Shows all members with username, role, join date
- Admin users see add/remove/role-change forms
- Non-members get 403

### Member management (admin-only)
- Add: username input + role select, CSRF
- Remove: confirmation form, CSRF
- Role update: role select dropdown, CSRF
- Non-admin gets 403

### Richer project detail
- Each artifact shows: title, description, slug (existing)
- Plus: version badge, branch + commit SHA, source user, destinations
- Missing fields gracefully hidden (not all artifacts have git refs)

## Test Strategy

~20 new tests:
- Dashboard renders page with org list and activity feed
- Dashboard empty activity shows empty state
- POST /orgs creates org and redirects
- POST /orgs invalid slug shows error
- POST /orgs invalid CSRF returns 403
- POST /orgs gRPC failure shows error
- Members page returns 200 with members
- Members page non-member returns 403
- Members page invalid slug returns 400
- Add/remove/update member with CSRF
- Non-admin member management returns 403
- Project detail shows enriched artifact data
- Existing dashboard tests updated (no longer redirect)
