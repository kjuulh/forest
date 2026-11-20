export const IN_FLIGHT = new Set(["QUEUED", "RUNNING", "ASSIGNED"]);
export const DEPLOYED = new Set(["SUCCEEDED"]);
export const STOPPED = new Set(["FAILED", "TIMED_OUT", "CANCELLED"]);

export const DOT_PRIORITY = { awaiting: 6, flight: 5, live: 4, stopped: 3, pending: 2, past: 1 };

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

export function isGateAwaiting(stage) {
  return (
    stage.stage_type === "gate" &&
    stage.status === "RUNNING" &&
    Array.isArray(stage.gate_waiting_on) &&
    stage.gate_waiting_on.length > 0
  );
}

export function releaseEnvStates(release) {
  const byEnv = new Map();
  const put = (env, kind) => {
    if (!env) return;
    const prev = byEnv.get(env);
    if (prev === undefined || DOT_PRIORITY[kind] > DOT_PRIORITY[prev]) byEnv.set(env, kind);
  };

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

  for (const s of release.pipeline_stages || []) {
    if (isPlanAwaiting(s)) put(s.environment, "awaiting");
  }

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

export function timelineLaneStatesAttrs(releases) {
  return timelineEnvStates(releases).map(encodeLaneStates);
}

export function isUnfinished(kind) {
  return kind === "flight" || kind === "awaiting";
}

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

    if (kind === "past" && !liveByEnv.has(d.environment)) kind = "past";

    return { name: d.name, env: d.environment, kind };
  });
}

export function destinationsByEnv(release) {
  const byEnv = new Map();
  for (const d of releaseDestinationStates(release)) {
    if (!byEnv.has(d.env)) byEnv.set(d.env, []);
    byEnv.get(d.env).push(d);
  }
  return byEnv;
}

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

export function timelineRollbacks(releases) {
  const liveSeen = new Set();
  return (releases || []).map((release) => {
    const states = release ? releaseEnvStates(release) : [];
    const backwards = new Set();
    for (const { env, kind } of states) {
      if (kind === "flight" && liveSeen.has(env)) backwards.add(env);
    }
    for (const { env, kind } of states) {
      if (kind === "live") liveSeen.add(env);
    }
    return backwards;
  });
}
