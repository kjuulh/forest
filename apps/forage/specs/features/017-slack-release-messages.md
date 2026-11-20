# 017 — Slack release messages: honest status, fun emoji, details in a thread

## Problem

The bot-posted Slack release message (2026-11-19 screenshot) said
"✅ Release succeeded: understory/agent-gateway" while prod was still
deploying and the footer said "1/2 stages complete". The header came from the
latest event's title, and dev's `release_succeeded` event made the whole
release look done. The message was also wrapped in an attachment, so Slack
folded it behind "Show more", and there was nowhere to put the detail people
want: what changed, the plan, approvals and failures.

## Requirements

Applies to bot-token Slack integrations (channel posts and DMs). Incoming
webhook integrations cannot edit or thread, so they keep one message per event.

### R1 — Honest overall state

- R1.1 The header MUST say the release succeeded only when every pipeline stage
  has succeeded (skipped, cancelled or superseded stages count as finished,
  and the footer says how many were skipped). Without a pipeline, only when
  every destination has reported success: every destination in the intent's
  release steps, and at least the largest `destination_count` any event
  announced (Forest sets it on start events only, so it is kept on the ref).
- R1.4 If Forest's pipeline state can't be fetched, the delivery fails and is
  retried rather than rendered from events alone. The notifier reads
  `SUPERSEDED` and `AWAITING_SIGNAL` as they are (the web UI's converter folds
  them into `PENDING`).
- R1.6 An annotation nothing has started deploying (no pipeline stages, no
  destination events) reads "Ready to release" with a note that nothing is
  deploying, never "Releasing"; it follows along once a release starts.
  This relies on Forest sending RELEASE_STARTED (with the intent) for
  trigger-started releases too. An annotated event that arrives after a
  release has started for the message is ignored, so it cannot undo it.
- R1.7 A release Forest's reaper ends (heartbeat lost, timed out) sends
  RELEASE_FAILED like any other failure (forest-server `release_reaper`), so
  its message turns into a failure instead of staying on "Deploying".
- R1.5 A destination never moves back from a finished state to started (a
  late or redelivered start event).
- R1.2 The header shows one of: in progress, waiting for approval, succeeded,
  failed, cancelled. Any failed or rolled-back stage, or any failed destination
  event, makes the release failed.
- R1.3 A destination's terminal event that arrives before Forest marks its
  deploy stage MUST finish that stage (matched through the intent's release
  steps), so the message never sits on "running" with no further event coming.

### R2 — Readable state per stage

- R2.1 One line per stage, in dependency order, with a distinct emoji per
  state: queued, running, succeeded, failed, waiting for approval, skipped
  (cancelled or superseded), rolled back.
- R2.2 Finished stages show their duration; running stages show when they
  started as a Slack date token, which stays true while the message is not
  edited. The footer shows progress and, once finished, the total duration.

### R3 — Configurable emoji

- R3.1 Each organisation can set the emoji name for each slot (the seven
  states plus a success header "celebrate" and a failure header "mourn") on any
  of its Slack integration pages. One set per organisation.
- R3.2 An empty slot falls back to a standard emoji, so messages render before
  any custom emoji is uploaded. Names are validated (`[a-z0-9_+'-]`, ≤100);
  `:name:` is accepted and stored as `name`.
- R3.3 Only org admins with a valid CSRF token can change the set.

### R4 — Compact parent, details in a thread

- R4.1 The parent message uses top-level blocks (no attachment) and stays
  short: header with a View release button, commit title, metadata, stage
  lines, footer.
- R4.2 Thread replies, each posted at most once per parent message:
  - `changes`: annotated artifacts since the last one that reached any of this
    release's destinations, with short SHA link, author and PR link, capped at
    15. Posted only when there are at least two changes; the artifact list is
    fetched once per release either way. Known limit: it lists every artifact
    annotated in that window, including builds never meant for these
    destinations.
  - `plan:<stage>`: per destination `+add ~change -destroy` from the plan
    output; destroys flagged with :rotating_light:. Raw plan output is never
    posted.
  - `approval:<stage>:requested`: who can approve (any org member) and where.
  - `approval:<stage>:approved|rejected`: the decision, unless auto-approved.
  - `deploy:<destination>`: deployed, duration and a link to the release logs.
  - `failure:<destination>` / `failure:stage:<stage>`: the error excerpt
    (≤1500 chars) and a logs link. The stage reply is skipped when one of its
    destinations already reported the failure.
- R4.3 DMs get only approval and failure replies.

### R5 — Idempotent and rate-limit safe

- R5.0 All text from outside (commit titles, names, errors) is mrkdwn-escaped
  (`&`, `<`, `>`), so it cannot ping `<!channel>` or forge a link; only
  `https://` URLs become links.

- R5.1 A thread key is claimed atomically in Postgres before posting
  (`thread_keys ? key`), across all forage replicas; a failed post releases the
  claim so a retry posts it. Re-posting the parent (message deleted) resets the
  keys.
- R5.2 The parent is not updated when the rendered blocks hash equals the last
  one sent, so a redelivered event is a no-op.
- R5.3 Events for one release are serialised across replicas with a
  transaction-scoped Postgres advisory lock held from reading the ref to
  writing it (120 s lock timeout); thread replies are posted after the lock
  is released, since each reply's claim is atomic on its own. At most 4 such dispatches run per replica,
  since each holds a pooled connection.
- R5.4 HTTP 429 / `ratelimited` waits Slack's `Retry-After` (1–30 s, ≤60 s
  total per call) and retries; then the delivery's own retry ladder applies.

### R6 — Every part links to its place in Forest

- R6.1 The release page carries stable anchors: `#stage-<id>`, `#plan-<id>`
  (on the plan output, so a closed plan expands), `#dest-<name>`, `#logs`,
  `#pipeline`, `#destinations`. Ids go through one function (`anchor_id`,
  the `anchor` template filter): characters other than `[A-Za-z0-9_-]`
  become `-`.
- R6.2 Parent: the headline links to the release; the project and slug link
  to their pages; each stage's environment (or, without a pipeline, each
  destination) links to its row. An action row replaces the header button:
  the primary button is "Review plan" (`#stage-<id>`, which always renders
  and holds the plan) while waiting for approval, "See what failed"
  (`#logs`) on failure, else "View release"; then Logs, Source (CI's
  context URL) and Project. If the linked stage list would pass Slack's
  3000-character section limit, rows fall back to plain names, then trailing
  rows collapse into "…and N more", so the message is never rejected.
- R6.3 Thread: the approval request carries a "Review plan" button to the
  stage, the plan reply a "Full plan" button to the plan output; deploy replies link the destination to its destination
  page and to the logs; failure replies carry a neutral "Full logs" button; the
  changes reply links the previous release.
- R6.4 Without a Forage URL, names render as inline code and only the Commit
  button remains. Only `https://` URLs of at most 3000 characters with no
  control characters, `|`, `<`, `>` or spaces become links or buttons.

## Not done

- Image digests: Forest records no image digest for an artifact, so the thread
  cannot show one.
- Rolled back: Forest has no rolled-back stage state yet; the slot exists and
  renders `ROLLED_BACK` if Forest starts reporting it.
- A crash between claiming a reply and posting it loses that reply; a lost
  response after Slack accepted it duplicates it. Both are rare and accepted.
- The emoji set is per organisation, not per Slack workspace.
- Running stages do not re-render on a timer; the start time token stands in
  for an elapsed counter.

## Tests

- `forage-core/src/integrations/slack_release_tests.rs`: Block Kit snapshots
  per state (queued, running, waiting for approval, succeeded, failed,
  cancelled, custom emoji, the screenshot regression), thread replies,
  plan parsing, emoji validation.
- `forage-server/src/tests/slack_release_tests.rs`: one parent per release
  through a full dev→prod run, each reply once, redelivery no-op, rate limit
  retry, failure in thread, org emoji used.
- `forage-server/src/tests/slack_tests.rs`: emoji form save, validation,
  admin/CSRF.
