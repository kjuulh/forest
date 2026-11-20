# 015 - Visibility settings: adversarial review

**Reviewer:** an independent agent with a fresh context, 2026-10-29, on PR #336 head 72bba0f.
**Verdict:** no authorization blocker. One wording blocker, fixed.

| # | Finding | Severity | Resolution |
|---|---|---|---|
| 1 | The pages said "anyone signed in", but the public registry pages (`/components`, `/components/{org}/{name}`) and `SearchPublicComponents` / `GetPublicComponentDetail` / `GetPublicComponentManifest` need no sign-in, so a public component's readme, versions and manifest are visible to anyone. | blocker (honesty) | The pages, spec, skill, docs and proto comment now say public components are visible to anyone, even without signing in, and usable by anyone signed in. Tested by `the_pages_say_public_components_are_visible_without_signing_in`. |
| 2 | Spec item 7 promised a settings link on the public component page; it was not built. | should-fix | Spec corrected: that page redirects org members to the project page, whose badge links to the settings. Non-members have nothing to change. |
| 3 | A GET where Forest refused the read rendered a 500 (stale session after removal from the org). | should-fix | PermissionDenied maps to 403 and NotFound to 404 (`read_error`). Tested by `a_forest_refusal_to_read_is_a_403_or_404_not_a_500`. |
| 4 | The form is gated by Forest's `can_change` and the POST by the session role, so a stale session can disagree for a few minutes. | nit | Kept: both fail closed and Forest decides. Tested by `a_member_post_is_refused_even_when_forest_would_show_the_form`. |
| 5 | `GetVisibility` distinguishes an unknown org (NotFound) from a forbidden one (PermissionDenied). | nit | Existing `require_org_access` behaviour across Forest; not changed here. |
| 6 | Accessibility: error flash without `role="alert"`; the badge link's name was only "public"/"private". | nit | Added `role="alert"` and an `aria-label`. |
| tests | Gaps: hostile Forest message escaping; an admin of another organisation. | | `a_forest_error_message_is_escaped`; forest `an_admin_of_one_organisation_cannot_read_or_change_anothers_visibility`. |

Checked and correct (from the review): POST ordering (membership, admin, slug, CSRF,
value allow-list, then Forest); no IDOR or open redirect; `GetVisibility` writes nothing
and needs membership; `can_change` reuses the write gate (`require_visibility_admin`);
scoped tokens reach no OrganisationService method; templates autoescape; the hand-inserted
Forage gRPC messages and paths match the proto.
