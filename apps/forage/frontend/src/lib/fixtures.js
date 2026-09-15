/**
 * Release-timeline fixtures, one per state the swim lane has to distinguish.
 *
 * Shared by the unit tests and the visual gallery so the two cannot drift: a
 * screenshot of a state that no test covers, or vice versa, is how a rendering
 * bug survives a green suite.
 *
 * Shapes mirror the `/timeline` payload — see `platform.rs`, which sends
 * destination rows plus pipeline stages, with plan stages carrying
 * approval_status in Rust's Debug spelling ("AWAITINGAPPROVAL").
 *
 * `release` + `expect` is one release resolved on its own. A fixture may also
 * carry `below` — older releases rendered underneath it, newest first — plus
 * `expectBelow`, one expectation object each. Those exist because the gutter
 * resolves lane states *across* releases: what sits below a release changes
 * what the rail draws for it, and a fixture of one release cannot show that.
 */

const plan = (env, status, approval) => ({
  id: `plan-${env}`, stage_type: "plan", environment: env, status,
  ...(approval ? { approval_status: approval } : {}),
});
const deploy = (env, status) => ({
  id: `deploy-${env}`, stage_type: "deploy", environment: env, status,
});
// Destinations are *placements within* an environment, and an environment can
// hold several. Most fixtures only need one, so the name defaults; the ones
// exercising a partial rollout name each placement themselves.
const dest = (env, status, isCurrent = false, name = `${env}-main`) => ({
  name, environment: env, status, is_current: isCurrent,
});
const wait = (seconds, status) => ({
  id: `wait-${seconds}`, stage_type: "wait", duration_seconds: seconds, status,
});

export const FIXTURES = [
  {
    key: "complete",
    title: "Pipeline complete",
    why: "Everything succeeded and this release is what is live.",
    expect: { prod: "live", dev: "live" },
    release: {
      slug: "complete", has_pipeline: true,
      destinations: [dest("dev", "SUCCEEDED", true), dest("prod", "SUCCEEDED", true)],
      pipeline_stages: [
        plan("dev", "SUCCEEDED"), deploy("dev", "SUCCEEDED"),
        plan("prod", "SUCCEEDED"), deploy("prod", "SUCCEEDED"),
      ],
    },
  },
  {
    key: "awaiting-plan",
    title: "Awaiting plan approval",
    why:
      "THE BUG. dev is done, prod's plan is parked on a human. Nothing is " +
      "executing, but the pipeline is not finished — it must not read as complete.",
    expect: { dev: "live", prod: "awaiting" },
    release: {
      slug: "awaiting-plan", has_pipeline: true, release_intent_id: "ri-1",
      destinations: [dest("dev", "SUCCEEDED", true)],
      pipeline_stages: [
        plan("dev", "SUCCEEDED"), deploy("dev", "SUCCEEDED"),
        plan("prod", "RUNNING", "AWAITINGAPPROVAL"), deploy("prod", "PENDING"),
      ],
    },
  },
  {
    key: "awaiting-underscore",
    title: "Awaiting — underscored spelling",
    why:
      "Same state, approval_status spelled AWAITING_APPROVAL. Both spellings " +
      "have been seen from the API; treating only one as real is a silent miss.",
    expect: { prod: "awaiting" },
    release: {
      slug: "awaiting-underscore", has_pipeline: true, release_intent_id: "ri-2",
      destinations: [],
      pipeline_stages: [plan("prod", "RUNNING", "AWAITING_APPROVAL"), deploy("prod", "PENDING")],
    },
  },
  {
    key: "deploying",
    title: "Deploying",
    why: "A job is actually running. Distinct from awaiting: nobody needs to act.",
    expect: { dev: "live", prod: "flight" },
    release: {
      slug: "deploying", has_pipeline: true,
      destinations: [dest("dev", "SUCCEEDED", true), dest("prod", "RUNNING")],
      pipeline_stages: [
        plan("dev", "SUCCEEDED"), deploy("dev", "SUCCEEDED"),
        plan("prod", "SUCCEEDED"), deploy("prod", "RUNNING"),
      ],
    },
  },
  {
    key: "queued",
    title: "Queued",
    why: "Headed for prod, nothing started, and no approval is blocking it.",
    expect: { prod: "pending" },
    release: {
      slug: "queued", has_pipeline: true,
      destinations: [],
      pipeline_stages: [plan("prod", "PENDING"), deploy("prod", "PENDING")],
    },
  },
  {
    key: "failed",
    title: "Pipeline failed",
    why: "Terminal. Renders as `past` — not live here — with the failure on the card.",
    expect: { prod: "past" },
    release: {
      slug: "failed", has_pipeline: true,
      destinations: [dest("prod", "FAILED")],
      pipeline_stages: [plan("prod", "SUCCEEDED"), deploy("prod", "FAILED")],
    },
  },
  {
    key: "superseded",
    title: "Superseded — prod has moved off this release",
    why:
      "It deployed to prod and prod is no longer on it: `is_current` is false " +
      "on the destination, and something else holds it now. Rendered on top of " +
      "the release prod went back to, which is what a finished rollback looks " +
      "like once nothing is in flight — a solid head lower down the lane, and a " +
      "hollow ring up here marking where this release got to.",
    expect: { prod: "past" },
    release: {
      slug: "superseded", has_pipeline: true,
      destinations: [dest("prod", "SUCCEEDED", false)],
      pipeline_stages: [plan("prod", "SUCCEEDED"), deploy("prod", "SUCCEEDED")],
    },
  },
  {
    key: "awaiting-over-pending",
    title: "Awaiting outranks a pending destination row",
    why:
      "A PENDING destination row exists AND the plan is parked. `awaiting` has " +
      "to win, or the destination row hides the thing needing attention.",
    expect: { prod: "awaiting" },
    release: {
      slug: "awaiting-over-pending", has_pipeline: true, release_intent_id: "ri-3",
      destinations: [dest("prod", "PENDING")],
      pipeline_stages: [plan("prod", "RUNNING", "AWAITINGAPPROVAL"), deploy("prod", "PENDING")],
    },
  },
  {
    key: "rejected",
    title: "Plan rejected",
    why: "A rejected plan is finished, not waiting. It must NOT pulse.",
    expect: { prod: "past" },
    release: {
      slug: "rejected", has_pipeline: true,
      destinations: [],
      pipeline_stages: [plan("prod", "CANCELLED"), deploy("prod", "CANCELLED")],
    },
  },
  {
    key: "superseded-awaiting",
    title: "Superseded — an older release is still awaiting approval",
    why:
      "THE BUG. This release is live on dev and prod. The one below it never " +
      "got its prod approval, so on its own it resolves `awaiting` forever — " +
      "and the gutter kept drawing the amber went-backwards hatch down to it, " +
      "so a healthy prod read as broken. prod has moved on: the lane must be " +
      "settled, while the card below keeps saying what happened to it.",
    expect: { dev: "live", prod: "live" },
    release: {
      slug: "superseded-awaiting", has_pipeline: true,
      destinations: [dest("dev", "SUCCEEDED", true), dest("prod", "SUCCEEDED", true)],
      pipeline_stages: [
        plan("dev", "SUCCEEDED"), deploy("dev", "SUCCEEDED"),
        plan("prod", "SUCCEEDED"), deploy("prod", "SUCCEEDED"),
      ],
    },
    below: [
      {
        slug: "superseded-awaiting-stale", has_pipeline: true, release_intent_id: "ri-4",
        title: "Older release, prod approval never granted",
        destinations: [dest("dev", "SUCCEEDED", false)],
        pipeline_stages: [
          plan("dev", "SUCCEEDED"), deploy("dev", "SUCCEEDED"),
          plan("prod", "RUNNING", "AWAITINGAPPROVAL"), deploy("prod", "PENDING"),
        ],
      },
    ],
    expectBelow: [{ dev: "past", prod: "past" }],
  },
  {
    key: "broken-now",
    title: "Broken now — nothing has superseded the failure",
    why:
      "The other half of the supersession rule. prod's newest release is " +
      "parked on approval and no later release has taken prod over, so the " +
      "lane must still warn. Over-suppressing here would hide a live problem.",
    expect: { dev: "live", prod: "awaiting" },
    release: {
      slug: "broken-now", has_pipeline: true, release_intent_id: "ri-5",
      destinations: [dest("dev", "SUCCEEDED", true)],
      pipeline_stages: [
        plan("dev", "SUCCEEDED"), deploy("dev", "SUCCEEDED"),
        plan("prod", "RUNNING", "AWAITINGAPPROVAL"), deploy("prod", "PENDING"),
      ],
    },
    below: [
      {
        slug: "broken-now-older", has_pipeline: true,
        title: "Older release, failed on prod",
        destinations: [dest("dev", "SUCCEEDED", false), dest("prod", "FAILED")],
        pipeline_stages: [
          plan("dev", "SUCCEEDED"), deploy("dev", "SUCCEEDED"),
          plan("prod", "SUCCEEDED"), deploy("prod", "FAILED"),
        ],
      },
    ],
    expectBelow: [{ dev: "past", prod: "past" }],
  },

  {
    key: "partial-rollout",
    title: "Partial rollout — prod is three places, and one of them failed",
    why:
      "The state the destination layer exists for. Two of prod's three " +
      "placements took the release and the third blew up. At environment " +
      "level this is just `prod`, and a single bar through it would read as " +
      "healthy. Click the prod lane to fan it into its destinations.",
    expect: { prod: "live", dev: "live" },
    release: {
      slug: "partial-rollout", has_pipeline: true,
      destinations: [
        dest("dev", "SUCCEEDED", true),
        dest("staging", "SUCCEEDED", true),
        dest("prod", "SUCCEEDED", true, "prod-eu-north-1"),
        dest("prod", "SUCCEEDED", true, "prod-eu-west-1"),
        dest("prod", "FAILED", false, "prod-us-east-1"),
      ],
      pipeline_stages: [
        deploy("dev", "SUCCEEDED"),
        deploy("staging", "SUCCEEDED"),
        wait(900, "SUCCEEDED"),
        deploy("prod", "FAILED"),
      ],
    },
  },
  {
    key: "fanning-out",
    title: "Rolling across prod, one placement at a time",
    why:
      "A release part-way through prod: eu-north has it, eu-west is taking " +
      "it now, us-east has not been asked yet. The environment lane can only " +
      "say `deploying`; the strands say how far.",
    expect: { prod: "flight", dev: "live" },
    release: {
      slug: "fanning-out", has_pipeline: true,
      destinations: [
        dest("dev", "SUCCEEDED", true),
        dest("staging", "SUCCEEDED", true),
        dest("prod", "SUCCEEDED", true, "prod-eu-north-1"),
        dest("prod", "RUNNING", false, "prod-eu-west-1"),
        dest("prod", "PENDING", false, "prod-us-east-1"),
      ],
      pipeline_stages: [
        deploy("dev", "SUCCEEDED"),
        deploy("staging", "SUCCEEDED"),
        deploy("prod", "RUNNING"),
      ],
    },
  },
  {
    key: "soaking",
    title: "Soaking before prod",
    why:
      "Nothing is running and nobody is blocking it — the pipeline is " +
      "holding the release in staging until it has been there long enough. " +
      "The prod stage below it is drawn, dimmed: what is going to happen is " +
      "as much a part of reading a release as what already did.",
    expect: { staging: "live", prod: "pending", dev: "live" },
    release: {
      slug: "soaking", has_pipeline: true,
      destinations: [dest("dev", "SUCCEEDED", true), dest("staging", "SUCCEEDED", true)],
      pipeline_stages: [
        deploy("dev", "SUCCEEDED"),
        deploy("staging", "SUCCEEDED"),
        wait(1800, "RUNNING"),
        deploy("prod", "PENDING"),
      ],
    },
  },
  {
    key: "rolling-back",
    title: "Rolling back",
    why:
      "prod is being taken back to an older release. Backwards travel is " +
      "amber and its chevrons point the other way — a rollback must not be " +
      "mistakable for a deploy at a glance.",
    expect: { prod: "live", dev: "live" },
    release: {
      slug: "rolling-back-head", has_pipeline: true,
      destinations: [dest("dev", "SUCCEEDED", true), dest("prod", "SUCCEEDED", true)],
      pipeline_stages: [deploy("dev", "SUCCEEDED"), deploy("prod", "SUCCEEDED")],
    },
    below: [
      {
        slug: "rolling-back-target", has_pipeline: true,
        title: "The release prod is going back to",
        destinations: [dest("dev", "SUCCEEDED", false), dest("prod", "RUNNING", false)],
        pipeline_stages: [deploy("dev", "SUCCEEDED"), deploy("prod", "RUNNING")],
      },
    ],
    expectBelow: [{ prod: "flight" }],
  },
  {
    key: "hidden-commits",
    title: "Commits that changed nothing here",
    why:
      "Most commits in a shared repo touch nothing this project deploys. " +
      "They are folded behind one line so the timeline stays a record of what " +
      "moved — Uber calls the same idea \"collapsed in-between commits\". " +
      "Open it and they render as quiet rows with no lane presence at all.",
    expect: { prod: "live", dev: "live" },
    release: {
      slug: "hidden-commits", has_pipeline: true,
      destinations: [dest("dev", "SUCCEEDED", true), dest("prod", "SUCCEEDED", true)],
      pipeline_stages: [deploy("dev", "SUCCEEDED"), deploy("prod", "SUCCEEDED")],
    },
    extra: [
      {
        kind: "hidden",
        count: 6,
        releases: [
          { slug: "h1", title: "docs: correct the retention window in the runbook", commit_sha: "4c1f0b92aa", created_at: new Date(Date.UTC(2026, 8, 15, 10, 41)).toISOString(), source_user: "kjuulh" },
          { slug: "h2", title: "chore(deps): bump serde to 1.0.219", commit_sha: "9ae23d1177", created_at: new Date(Date.UTC(2026, 8, 15, 10, 12)).toISOString(), source_user: "octobot" },
          { slug: "h3", title: "test: cover the empty-destination case", commit_sha: "01bb7fe340", created_at: new Date(Date.UTC(2026, 8, 15, 9, 58)).toISOString(), source_user: "kjuulh" },
        ],
      },
    ],
  },
];

export const byKey = (k) => FIXTURES.find((f) => f.key === k);
