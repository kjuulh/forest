# 005 - Dashboard Enhancement: Adversarial Review

**Spec**: 005 - Enhanced Dashboard & Org Management
**Date**: 2026-03-07

## Findings

### Critical (fixed)

1. **Missing server-side admin authorization on member management routes**
   - `add_member_submit`, `update_member_role_submit`, `remove_member_submit` only checked org membership, not admin/owner role
   - Template hid forms for non-admins, but POST requests could be made directly
   - **Fix**: Added `require_admin()` helper, called in all three handlers before CSRF check
   - **Tests added**: `add_member_non_admin_returns_403`, `remove_member_non_admin_returns_403`, `update_role_non_admin_returns_403`, `members_page_non_admin_can_view`

### Minor (accepted)

2. **Dashboard fetches artifacts sequentially**
   - For each org, projects are fetched sequentially, then artifacts per project
   - Could be slow with many orgs/projects
   - Mitigated by: cap of 10 artifacts, `take(5)` on projects per org, `unwrap_or_default()` on failures
   - Future improvement: use `tokio::join!` or `FuturesUnordered` for parallelism

3. **Create org error always renders onboarding template**
   - If a user with existing orgs creates a new org and it fails, they see the onboarding page instead of the dashboard
   - Acceptable for now since the form is on both pages; the user can navigate back

### Verified secure

- **XSS**: MiniJinja auto-escapes all `{{ }}` expressions. Error messages are hardcoded strings. URL paths use `validate_slug()` (only `[a-z0-9-]`).
- **CSRF**: All POST handlers validate CSRF token before performing mutations.
- **Authorization**: All org-scoped routes check membership via `require_org_membership()`. Member management routes additionally check admin/owner role via `require_admin()`.
- **Input validation**: `validate_slug()` on org/project names. Form deserialization rejects missing fields.
- **Graceful degradation**: gRPC failures return `unwrap_or_default()` (empty lists) rather than 500 errors.

## Test Coverage

- 86 total tests (22 core + 66 server), all passing
- 23 new tests for spec 005 features
- Authorization tests cover: non-member (403), non-admin member management (403), valid admin operations (303)
- Template rendering verified: dashboard content, empty states, enriched artifact fields, admin-only UI
