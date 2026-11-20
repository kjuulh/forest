# 016 - Organisation baseline for individual notifications

**Status**: implemented on `feat/forage-org-baseline-notifications`; not deployed, no live provider app installed.
**Depends on**: 006 (Notification Integrations), 010 (Account Integrations)

## Problem

An organisation can connect Slack once for channel posts, but an individual DM
about your own release only happened if you had personally linked Slack
(`slack_user_links`). Most members never do, so most members get nothing.
There was also no GitHub organisation connection, and the account page's
"Slack" preference column was never read by the DM router: turning it off did
not stop DMs.

**An org admin connects the GitHub organisation and the Slack workspace once;
existing Forest members then get DMs about their own releases without linking
anything themselves, and a member's own choice still wins.** No connection
creates a Forest account, adds anyone to an organisation, grants a role or
changes a sign-in method.

## What exists today (before this change)

- Forest picks the one personal recipient of a release event
  (`release_owner::personal_recipient`): the user who ran it, or the detected
  commit author resolved by a Forest-linked GitHub numeric id or by a matching
  Forest email. Bots, apps and service accounts have no recipient.
- Forage DMs that recipient for `release_started|succeeded|failed` only through
  a personal Slack link, once per Slack integration.
- Forest stores preferences as `(user, type, channel, enabled)`; no row means on.

## Provider constraints (read 2026-11-14, primary sources)

| Claim | Source |
|---|---|
| `users.lookupByEmail` needs `users:read.email` (with `users:read`), Tier 3; a deactivated user returns `users_not_found` | https://docs.slack.dev/reference/methods/users.lookupByEmail |
| User object fields `deleted`, `is_bot`, `is_restricted`, `is_ultra_restricted`, `is_stranger`, `team_id`, `is_email_confirmed` | https://docs.slack.dev/reference/objects/user-object |
| A DM needs only `chat:write`, with the user id as `channel` | https://docs.slack.dev/reference/methods/chat.postMessage |
| `app_uninstalled`, `tokens_revoked`; request signing `v0`, 5 minute window | https://docs.slack.dev/reference/events/app_uninstalled, https://docs.slack.dev/authentication/verifying-requests-from-slack |
| Setup URL `installation_id` can be spoofed; verify with a user token and `GET /user/installations` | https://docs.github.com/en/apps/creating-github-apps/registering-a-github-app/about-the-setup-url |
| `GET /app/installations/{id}` (app JWT) returns account, `suspended_at`, permissions | https://docs.github.com/en/rest/apps/apps |
| Org membership endpoints need Members: read and return no email; user email is the public profile email only | https://docs.github.com/en/rest/orgs/members, https://docs.github.com/en/rest/users/users |
| Only org owners can install an app that requests org permissions | https://docs.github.com/en/apps/using-github-apps/installing-a-github-app-from-a-third-party |

Not established from primary sources: that Slack only matches confirmed
emails (hence the `is_email_confirmed` check when present); Slack Connect /
Enterprise Grid behaviour of `lookupByEmail` (Grid org-wide installs are
refused); that GitHub returns `state` to the Setup URL (the flow refuses to
continue without it).

**Consequence:** a GitHub App cannot supply a verified email for org members,
so GitHub is not an identity source for DMs. Silent mapping happens only on
Slack, by the member's Forest-verified email.

## Behaviour

### 1. Who gets a DM
Unchanged: Forest's personal recipient (`source_user_id`) for the three DM
event types. Forest now also sends `ReleaseContext.source_attribution`:
`actor`, `github_id`, `email` or `""`. Org baseline delivery requires `actor` or
`github_id`; commit-email-only and unknown attribution are recorded skips.
Personal links keep today's behaviour. Annotation now drops caller-supplied
`forest.author.user_id` and `forest.author.linked_by` before resolving the author
(`release_author::strip_server_owned`): before this change any caller could name
the owner of a release in its metadata. The forest CLI never sends these keys.

### 2. Eligibility, re-checked when each event is routed
The recipient must exist in Forest and be a current member of the event's
organisation (project access is organisation membership in Forest), read with
Forage's service account. Forest errors skip the DM. Events older than 24 hours
send no DM.

### 3. Where (one route, one DM per Slack workspace)
Per Slack workspace connected to the organisation (the oldest enabled
integration per `team_id` sends; others on the same workspace do not):
1. the member's personal Slack link for that workspace;
2. otherwise, when the admin enabled "Reach members without a personal Slack
   link": `users.lookupByEmail` for each verified Forest email. Exactly one
   distinct account must be acceptable: same `team_id`, not deleted, not a bot
   or Slackbot, not a guest, not `is_stranger`, email confirmed when reported,
   profile email equal to the queried one;
3. otherwise a recorded skip: `no_verified_email`, `unmapped`, `ambiguous` or
   `rejected` with the reason.

"Verified" means verified by Forest through proof of ownership. Forest's
`VerifyEmail` RPC now requires a service account like
`ConfirmEmailVerification`: before this change a signed-in user could mark any
address they added as verified, which would have let them be matched as a
colleague. No first-party client called it with a user token. Addresses that
were self-verified through it before the fix are not detected or re-checked.

Baseline settings and coverage are accepted only on the integration that
sends for its workspace (409 elsewhere, with a pointer to it).

Decisions are cached per (integration, member) for 6 hours, keyed on the
verified-email set; an email change invalidates it. Provider errors are never
cached and never produce a recipient. Only the matched Slack user id is stored.

### 4. Preference precedence
Per (member, event type): an explicit Forest SLACK preference off means no DM
on any route; explicit on means a DM when a route exists; no row means the
organisation default for that type. Defaults are per Slack integration and
start as the integration's channel rules (all on), which is exactly the
pre-change behaviour. Members see "Following your organisation's default" vs
"your choice" and can return to the default (`ResetNotificationPreference`).

### 5. Duplicates, retries, replays
One DM per (workspace, Slack user, release): retries and JetStream redelivery
update the stored message (`chat.update`). Reinstall keeps the integration id
and so the message refs.

### 6. Revocation
A signed `app_uninstalled` / `tokens_revoked` (bot) event, or
`invalid_auth|token_revoked|account_inactive` when sending, disables every
integration of the organisation on that workspace and drops its directory
cache until reinstalled.
A GitHub `installation` `deleted` removes the connection; `suspend` disables it;
a failed health check on the connection page shows it as uninstalled.

### 7. Connecting
- **Slack**: the existing install, now bound to a single-use server-side nonce
  that records the admin, organisation and (for reinstall) integration. A
  reinstall authorised for a different workspace changes nothing. Grid org-wide
  installs are refused. Bot scopes are unchanged; baseline delivery uses only
  `chat:write`, `users:read`, `users:read.email`.
- **GitHub**: install URL with a nonce → Setup URL (remembers the claimed
  `installation_id` under a second nonce) → user authorisation → callback
  proves the installer can access the installation, that it is an
  `Organization` with Members: read and not suspended, and that the installer's
  membership is `admin`/`active`. The user token is dropped after these checks.
  One installation binds to exactly one Forest organisation.

The personal Slack link (`/settings/account/slack/connect`) now also uses a
single-use nonce instead of the user id as state: a personal link overrides the
directory route, so a forged callback could otherwise redirect someone's DMs.

### 8. Visibility
Admins see per-member coverage (Slack: reached by personal link / matched by
verified email / setup gap with reason; GitHub: linked GitHub account is or is
not a member, or no GitHub account linked). Only Forest members are listed;
no Slack ids, emails or other directory data are rendered. Skipped DMs appear
once per event (redeliveries do not repeat them) in the integration's delivery
history as `Skipped` with the reason and the
member's Forest username.

## Deliberately not done

- No admin-managed manual mapping of members to Slack accounts: the verified
  email match plus the personal link covers it without a new trust boundary.
- GitHub membership never routes, grants or attributes anything by itself.
- No change to Forest's commit-email attribution (it can match an unverified
  Forest email); baseline delivery refuses it instead.
- Slack bot scopes beyond the three needed predate this change and are not
  reduced here; integration configs keep the existing at-rest encoding
  (`PgIntegrationStore::encrypt_config`), which is a repeating-key XOR.
- Coverage lists the first 100 members (one page of `ListMembers`).

## Configuration

| Env var | Purpose |
|---|---|
| `SLACK_SIGNING_SECRET` | verifies `/integrations/slack/events`; unset → 404 |
| `SLACK_API_BASE_URL`, `SLACK_AUTHORIZE_URL` | default `https://slack.com/api`, `https://slack.com/oauth/v2/authorize` |
| `GITHUB_APP_ID`, `GITHUB_APP_SLUG`, `GITHUB_APP_PRIVATE_KEY` (PEM) | with `GITHUB_APP_CLIENT_ID/SECRET`, enables GitHub organisation connections |
| `GITHUB_APP_WEBHOOK_SECRET` | verifies `/integrations/github/webhook` (`X-Hub-Signature-256`) |
| `GITHUB_API_BASE_URL`, `GITHUB_WEB_BASE_URL` | default `https://api.github.com`, `https://github.com` |
| `FOREST_SERVICE_ACCOUNT_API_KEY` | required for any DM: membership, emails and preferences are read with it |

GitHub App settings: Organization permission *Members: read*; Setup URL
`{FORAGE_HOST}/integrations/github/setup`; callback URL
`{FORAGE_HOST}/integrations/github/callback` added alongside the login callback;
webhook URL `{FORAGE_HOST}/integrations/github/webhook` subscribed to
`installation`. Slack app: Event Subscriptions request URL
`{FORAGE_HOST}/integrations/slack/events` with `app_uninstalled` and
`tokens_revoked`.

## Verification

- `forage-core`: `integrations::baseline` (18 unit tests), `integrations::direct`
  (18 planner scenarios), `integrations::signing` (3).
- `forage-server`: `tests/org_connection_tests.rs` (24 route tests against an
  in-process Slack/GitHub double, including DMs sent by the real dispatcher over
  HTTP).
- `forest-server`: `release_owner` attribution unit tests (4), `release_author`
  metadata stripping (1), `accepttest/email_verification.rs` self-verification
  refusal (1) and
  `tests/accepttest/notification_preferences.rs` (5).
