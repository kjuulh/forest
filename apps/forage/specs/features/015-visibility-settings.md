# 015 - Visibility settings for organisations and components

**Status:** Phase 4 — implemented, adversarially reviewed (`specs/reviews/015-adversarial-review.md`), findings fed back.
**Depends on:** forest #335 (`SetOrganisationVisibility`, `UpdateProjectRequest.visibility`,
read-only public components and organisations, admin-only changes, app tokens refused).

## Problem

Forest lets an organisation make one component, or every component it publishes,
visible to anyone (the public registry pages need no sign-in) and usable by anyone signed in. Only the CLI (`forest organisation visibility`,
`forest components visibility`) can change it. Forage shows nothing: an admin can't
see or change the setting, and a member can't tell why a component is (or isn't)
reachable from other organisations.

## Separation of concerns

**Forest** owns the setting and the policy. Forage renders it and submits changes with
the signed-in user's own token. Forage never decides who may change visibility. It asks
Forest (`GetVisibility.can_change`, computed by the same gate that guards the writes),
and Forest refuses a write Forage should not have offered.

New Forest RPC, `OrganisationService.GetVisibility(organisation, optional project)`:
- needs membership of the organisation;
- returns `organisation_visibility`, `project_visibility` (when a project is named) and
  `can_change` (true only for a human admin of the organisation or a service account).

## Behaviour

1. **Org settings → Visibility** at `/orgs/{org}/settings/visibility`, linked from the
   settings sidebar. Shows whether the organisation is public or private.
2. **Project settings → Visibility** at
   `/orgs/{org}/projects/{project}/settings/visibility`, linked from the project settings
   nav. Shows the component's own setting and the effective result.
3. **Meaning, stated on both pages:** public = anyone can *see* the component on the
   public registry pages (readme, versions, manifest), even without signing in, and anyone
   signed in can resolve and download it. It is read-only: publishing, unpublishing and every
   other change still need membership. A public organisation exposes its components only,
   not its releases, destinations, members or settings.
4. **Inherited status is honest:** a component is public when its own setting **or** its
   organisation's is. When the organisation is public, the project page says the component
   is public through the organisation, and that making the component private does **not**
   override that.
5. **Who may change it:** when `can_change` is true, the page shows a public/private form
   with the current value selected. Otherwise the state is read-only, with a line saying an
   organisation admin can change it. A forged POST from a non-admin is refused by Forage
   (`require_admin`) before any call, and by Forest regardless.
6. **Forms:** CSRF-checked POSTs. Only `public` and `private` are accepted (400 otherwise).
   A successful save redirects back to the page, which reads the value back from Forest.
   A Forest refusal is shown as an error on the page, not a 500.
7. **Discoverability:** the project page's visibility badge links to the project's
   visibility settings. The public component page (`/components/{org}/{name}`) needs no
   link: it already shows the badge, and it redirects members of the organisation to the
   project page. A project with no published component is reached through Settings.

## Non-goals

- The CUE module registry (`/v2`) is unauthenticated whatever this setting says. Forage
  does not claim otherwise and does not change it.
- No bulk or per-version visibility.

## Tests (forage-server `src/tests/visibility_tests.rs`)

- org page: renders the setting; admin sees the form, member sees read-only text.
- org POST: admin saves and redirects; member refused (403) with no platform call;
  invalid value refused (400); bad CSRF refused; non-member 403/404.
- project page: shows own and effective state; inherited-public wording when the org is
  public; admin form vs member read-only.
- project POST: admin saves and redirects; member refused with no platform call; Forest
  refusal rendered as an error.
- sidebar and project nav link to the pages.

Forest-server acceptance tests cover `GetVisibility` (membership required, values,
`can_change` false for a member and for an app token, true for an admin).
