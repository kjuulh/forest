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
const dest = (env, status, isCurrent = false) => ({
  name: `${env}-dest`, environment: env, status, is_current: isCurrent,
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
    title: "Superseded",
    why: "It did deploy, but a later release took over. Historical.",
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
];

export const byKey = (k) => FIXTURES.find((f) => f.key === k);
