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
  if (isGateAwaiting(stage)) {
    return "AWAITING_SIGNAL";
  }
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
 * A gate that is running and still missing something.
 *
 * The server keeps a gate ACTIVE while it waits and lists what it is missing in
 * `gate_waiting_on`, the same way a plan stage stays ACTIVE with an
 * `approval_status`. An empty list on a running gate means everything reported
 * and the stage is about to succeed, so it is not "awaiting" — it is finishing.
 */
export function isGateAwaiting(stage) {
  return (
    stage.stage_type === "gate" &&
    stage.status === "RUNNING" &&
    Array.isArray(stage.gate_waiting_on) &&
    stage.gate_waiting_on.length > 0
  );
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
    else if (status === "PENDING" || status === "AWAITING_APPROVAL" || status === "AWAITING_SIGNAL") put(s.environment, "pending");
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
 * And only *idle* legs are demoted. A leg that is `awaiting` or `pending` has
 * nothing executing behind it, so once a newer release goes live it is history
 * and saying otherwise cries wolf — that is the bug this function was written
 * for. A leg that is `flight` is a job running right now against an older
 * commit, which is what a rollback looks like from here: prod is live on the
 * newest release and a deploy is actively taking it back. Hiding that would be
 * the same failure in the other direction, on the state that matters most.
 *
 * Ordering is the caller's timeline order, which is also the order the cards
 * are painted in and the order `computeLaneBars` measures, so the rail and the
 * dots cannot disagree about which release is newer.
 *
 * @param {Array<object|null|undefined>} releases newest first; holes tolerated
 * @returns {Array<Array<{env: string, kind: string}>>} one entry per input
 */
/**
 * Legs a newer `live` release does *not* demote.
 *
 * `past` is already history. `flight` is the exception that matters: a job
 * running against an older commit while a newer one is live is a rollback in
 * progress, and the gutter must never hide work that is actually executing.
 * Everything else — a leg that was live, one parked on approval, one merely
 * queued, one that failed — is history the moment something newer takes the
 * environment over.
 */
const NOT_SUPERSEDED = new Set(["flight", "past"]);

export function timelineEnvStates(releases) {
  const supersedes = new Set();
  return (releases || []).map((release) => {
    const states = release ? releaseEnvStates(release) : [];
    const resolved = states.map(({ env, kind }) =>
      supersedes.has(env) && !NOT_SUPERSEDED.has(kind) ? { env, kind: "past" } : { env, kind },
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

// ── Destinations: the layer under an environment ──────────────────────────
//
// An environment is not a place a release lands; it is a set of them. `prod`
// may be three destinations in three regions, each with its own status, its own
// queue position and its own way to fail. The gutter shows environments by
// default because that is the question people usually have, but a partial
// rollout — two destinations on the new release, one stuck — is invisible at
// that level, and it is the exact state somebody is staring at during an
// incident.

/**
 * Resolve each destination this release touches to a lane state.
 *
 * Same vocabulary as `releaseEnvStates`, one level down. A destination has no
 * plan stage of its own, so it inherits `awaiting` from its environment's
 * parked plan: the placement genuinely is waiting on a person, it is just that
 * the person is asked about the environment.
 *
 * @returns {Array<{name: string, env: string, kind: string}>}
 */
export function releaseDestinationStates(release) {
  const awaitingEnvs = new Set(
    (release.pipeline_stages || []).filter(isPlanAwaiting).map((s) => s.environment),
  );

  const dests = release.destinations || [];
  const liveByEnv = new Set(
    dests.filter((d) => d.is_current && DEPLOYED.has(d.status)).map((d) => d.environment),
  );

  return dests.map((d) => {
    const status = d.status || "PENDING";
    let kind;
    if (IN_FLIGHT.has(status)) kind = "flight";
    else if (DEPLOYED.has(status)) kind = d.is_current ? "live" : "past";
    else if (STOPPED.has(status)) kind = "stopped";
    else kind = awaitingEnvs.has(d.environment) ? "awaiting" : "pending";

    // A succeeded destination inside an environment some *other* destination
    // still holds is history, not a failure — `past` already says that.
    if (kind === "past" && !liveByEnv.has(d.environment)) kind = "past";

    return { name: d.name, env: d.environment, kind };
  });
}

/**
 * Group a release's destinations by environment, in a stable order.
 * Used both to fan a lane out and to nest destinations under their
 * environment's stage row on the card.
 */
export function destinationsByEnv(release) {
  const byEnv = new Map();
  for (const d of releaseDestinationStates(release)) {
    if (!byEnv.has(d.env)) byEnv.set(d.env, []);
    byEnv.get(d.env).push(d);
  }
  return byEnv;
}

/**
 * Every destination name the timeline knows about, per environment.
 *
 * A strand has to exist for a destination across the *whole* list, not just on
 * the releases that happen to mention it — otherwise a lane gains and loses
 * strands as you scroll, and the gutter stops being a diagram of anything.
 */
export function timelineDestinations(releases) {
  const byEnv = new Map();
  for (const release of releases || []) {
    if (!release) continue;
    for (const d of release.destinations || []) {
      if (!d.environment || !d.name) continue;
      if (!byEnv.has(d.environment)) byEnv.set(d.environment, []);
      const list = byEnv.get(d.environment);
      if (!list.includes(d.name)) list.push(d.name);
    }
  }
  for (const list of byEnv.values()) list.sort();
  return byEnv;
}

/**
 * Supersession, applied one level down.
 *
 * Same rule as `timelineEnvStates`, and for the same reason: an older release
 * whose leg to `prod-eu-west-1` never got approved is history once a newer one
 * is live there, and a strand that keeps pulsing for it cries wolf.
 *
 * @returns {Array<Map<string, string>>} one destination→kind map per release
 */
export function timelineDestinationStates(releases) {
  const supersedes = new Set();
  return (releases || []).map((release) => {
    const states = release ? releaseDestinationStates(release) : [];
    const resolved = new Map();
    for (const { name, kind } of states) {
      resolved.set(name, supersedes.has(name) && !NOT_SUPERSEDED.has(kind) ? "past" : kind);
    }
    for (const [name, kind] of resolved) {
      if (kind === "live") supersedes.add(name);
    }
    return resolved;
  });
}

/**
 * Environments this release is *currently broken on*.
 *
 * `releaseEnvStates` resolves a terminal failure to `past` — "not live here",
 * which is true, and which the card already explains. But the gutter is what
 * somebody scans during an incident, and a lane that says only "not live"
 * cannot distinguish "a newer release took over" from "the deploy blew up and
 * nothing has replaced it".
 *
 * So failure is reported separately and the caller decides: the component marks
 * the newest row for an environment as `stopped` only when no `live` row sits
 * above it. Kept out of the state vocabulary itself so supersession keeps its
 * single meaning, and so the existing resolution is unchanged.
 */
export function releaseStoppedEnvs(release) {
  const envs = new Set();
  for (const d of release?.destinations || []) {
    if (d.environment && STOPPED.has(d.status)) envs.add(d.environment);
  }
  for (const s of release?.pipeline_stages || []) {
    if (s.stage_type === "deploy" && s.environment && STOPPED.has(s.status)) envs.add(s.environment);
  }
  return envs;
}

/**
 * Which environments each release is *rolling back* to.
 *
 * forest has no rollback flag; a rollback is a shape in the timeline. An older
 * release with a deploy running against an environment that a newer release is
 * already live on is, by definition, that environment being taken backwards.
 *
 * It is worth naming because the two directions need opposite reactions and
 * look identical otherwise: "Deploying to prod" on a release from last Tuesday
 * is reassuring, and it should not be. Uber's UI says `Rolling back in
 * production ▼` for the same reason — see design/RELEASE-SWIMLANE.md.
 *
 * @param {Array<object|null|undefined>} releases newest first
 * @returns {Array<Set<string>>} one set of environment names per input
 */
export function timelineRollbacks(releases) {
  const liveSeen = new Set();
  return (releases || []).map((release) => {
    const states = release ? releaseEnvStates(release) : [];
    const backwards = new Set();
    for (const { env, kind } of states) {
      if (kind === "flight" && liveSeen.has(env)) backwards.add(env);
    }
    // Added after the check, not before: a release cannot roll back to itself.
    for (const { env, kind } of states) {
      if (kind === "live") liveSeen.add(env);
    }
    return backwards;
  });
}
