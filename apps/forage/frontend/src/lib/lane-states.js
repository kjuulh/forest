/**
 * Per-environment lane state for the release timeline.
 *
 * Extracted from ReleaseTimeline.svelte so it can be tested directly: the
 * swim-lane rail is the only thing on the page that says whether a release is
 * finished, and getting it wrong is silent — the timeline still renders, it just
 * lies. See lane-states.test.js.
 */

// Status vocabulary shared with the server (`release_states.status`) and with
// pipeline stage status.
export const IN_FLIGHT = new Set(["QUEUED", "RUNNING", "ASSIGNED"]);
export const DEPLOYED = new Set(["SUCCEEDED"]);
export const STOPPED = new Set(["FAILED", "TIMED_OUT", "CANCELLED"]);

// What a dot on a lane can mean, highest priority first when an environment
// resolves to more than one:
//
//   awaiting a plan for this environment is parked, waiting for a person
//   flight   a deploy is queued, assigned or running
//   live     the release is on that environment now
//   pending  it is headed there but nothing has started
//   past     it was released there, and a later release has since taken over
//
// `awaiting` outranks `flight` deliberately. Both mean "not finished", but only
// one of them will still be unfinished tomorrow unless somebody acts, so it is
// the more important thing to surface.
//
// Terminal failures render as `past` — "not live here", which is true. The
// failure itself is carried by the card and by the lane bar.
export const DOT_PRIORITY = { awaiting: 6, flight: 5, live: 4, stopped: 3, pending: 2, past: 1 };

/**
 * Normalize plan stage status.
 *
 * The API returns status="RUNNING" with approval_status="AWAITINGAPPROVAL" (no
 * underscore — it is Rust's Debug format). Both spellings are accepted because
 * both have been observed in the wild.
 */
export function effectiveStatus(stage) {
  if (
    stage.stage_type === "plan" &&
    stage.approval_status &&
    (stage.approval_status === "AWAITINGAPPROVAL" || stage.approval_status === "AWAITING_APPROVAL")
  ) {
    return "AWAITING_APPROVAL";
  }
  return stage.status;
}

export function isPlanAwaiting(stage) {
  return stage.stage_type === "plan" && effectiveStatus(stage) === "AWAITING_APPROVAL";
}

/**
 * Resolve each environment this release touches to a single lane state.
 */
export function releaseEnvStates(release) {
  const byEnv = new Map();
  const put = (env, kind) => {
    if (!env) return;
    const prev = byEnv.get(env);
    if (prev === undefined || DOT_PRIORITY[kind] > DOT_PRIORITY[prev]) byEnv.set(env, kind);
  };

  // Destination rows are authoritative wherever they exist.
  const dests = release.destinations || [];
  const liveEnvs = new Set(
    dests.filter((d) => d.is_current && DEPLOYED.has(d.status)).map((d) => d.environment),
  );
  for (const d of dests) {
    const status = d.status || "PENDING";
    if (IN_FLIGHT.has(status)) put(d.environment, "flight");
    else if (DEPLOYED.has(status)) put(d.environment, liveEnvs.has(d.environment) ? "live" : "past");
    else if (STOPPED.has(status)) put(d.environment, "past");
    else put(d.environment, "pending");
  }

  // A plan parked on approval is the whole point of this module.
  //
  // It gets its own state rather than falling through to `pending`, and it is
  // read off the *plan* stage rather than a destination status, because a
  // destination behind an unapproved plan has not been touched — it reports
  // PENDING, which is also what a release that is merely queued behind an
  // upstream stage reports. Collapsing the two makes a pipeline that is waiting
  // on a human indistinguishable from one that is waiting on a machine, and the
  // first needs somebody to go and look at it.
  //
  // Deliberately not gated on `byEnv.has(env)`: the destination loop above may
  // already have written `pending` for this environment, and `awaiting` must
  // win. DOT_PRIORITY handles that.
  for (const s of release.pipeline_stages || []) {
    if (isPlanAwaiting(s)) put(s.environment, "awaiting");
  }

  // Deploy stages fill in environments the release is headed for but has not
  // reached — a freshly queued or approval-blocked release has a pipeline stage
  // naming the environment before any release_states row exists, and without
  // this it would sit on the timeline with no bubble at all.
  for (const s of release.pipeline_stages || []) {
    if (s.stage_type !== "deploy" || !s.environment) continue;
    if (byEnv.has(s.environment)) continue;
    const status = effectiveStatus(s);
    if (IN_FLIGHT.has(status)) put(s.environment, "flight");
    else if (STOPPED.has(status)) put(s.environment, "past");
    else if (status === "PENDING" || status === "AWAITING_APPROVAL") put(s.environment, "pending");
  }

  return [...byEnv].map(([env, kind]) => ({ env, kind }));
}

/**
 * Apply "the latest release per environment wins" across an ordered timeline.
 *
 * `releaseEnvStates` sees one release at a time, so it cannot know that an
 * environment has moved on since. An older release whose prod leg is
 * `awaiting`, `pending` or `flight` is telling the truth about *itself* — that
 * pipeline never finished, and its card should keep saying so — but the
 * swim-lane aggregates across releases, and once a newer release is live on
 * prod that older leg is history. Left alone it keeps the rail's amber
 * "went backwards" hatch, and a dashed pending dot, on a healthy environment.
 *
 * Walks newest → oldest and demotes every leg for an environment some newer
 * release is already `live` on to `past` — the state that already means
 * exactly this: "not live here, a later release took over".
 *
 * Only `live` supersedes. A newer release that failed, or is still in flight,
 * has not taken the environment over, so it leaves the legs below it alone —
 * an environment that is genuinely broken right now must still warn.
 *
 * Ordering is the caller's timeline order, which is also the order the cards
 * are painted in and the order `computeLaneBars` measures, so the rail and the
 * dots cannot disagree about which release is newer.
 *
 * @param {Array<object|null|undefined>} releases newest first; holes tolerated
 * @returns {Array<Array<{env: string, kind: string}>>} one entry per input
 */
export function timelineEnvStates(releases) {
  const supersedes = new Set();
  return (releases || []).map((release) => {
    const states = release ? releaseEnvStates(release) : [];
    const resolved = states.map(({ env, kind }) =>
      supersedes.has(env) && kind !== "past" ? { env, kind: "past" } : { env, kind },
    );
    for (const { env, kind } of resolved) {
      if (kind === "live") supersedes.add(env);
    }
    return resolved;
  });
}

function encodeLaneStates(states) {
  return states.map(({ env, kind }) => `${env}:${kind}`).join(",");
}

export function laneStatesAttr(release) {
  return encodeLaneStates(releaseEnvStates(release));
}

/**
 * `laneStatesAttr` for a whole timeline, with supersession applied.
 *
 * The lane rail reads its per-release state back off `data-lane-states`, so
 * this is where the cross-release view has to land: one attribute per release,
 * in the same order, already resolved.
 */
export function timelineLaneStatesAttrs(releases) {
  return timelineEnvStates(releases).map(encodeLaneStates);
}

/**
 * Does this lane state mean "the release is not finished here"?
 *
 * The swim-lane bar uses this to decide whether to draw the hatched, animated
 * in-flight segment. `awaiting` counts: no job is executing, but the pipeline
 * has not finished, and drawing it as settled is the bug this module exists to
 * prevent.
 */
export function isUnfinished(kind) {
  return kind === "flight" || kind === "awaiting";
}
