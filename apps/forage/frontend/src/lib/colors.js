/**
 * Colour for the release timeline.
 *
 * Two rules hold the whole system together:
 *
 *   1. **Cool means the pipeline is working. Warm means you are needed.**
 *      Environments only ever get cool hues, so amber and red are free to mean
 *      one thing each — parked on a person, and broken — anywhere they appear.
 *      The previous palette spent amber on an environment called `finance` and
 *      on "this deploy went backwards", which made the gutter unreadable at the
 *      one moment it mattered.
 *
 *   2. **Weight tracks blast radius.** Production is the darkest, most
 *      saturated mark in the gutter and everything upstream of it is lighter,
 *      so the eye lands on production first without having to read a label.
 *
 * Environments are classified by their *stage* — the last dash- or
 * underscore-separated segment that names one. `data-prod` is production,
 * `platform-dev` is development. The old substring scan got those two right by
 * luck and `prod-metrics` wrong.
 */

// Stage → [light, dark] pairs. Ordered prod-first; `STAGES` order is also the
// lane order, see `envRank`.
const STAGES = [
  { stage: "prod", names: ["prod", "production", "live"], light: "#2563eb", dark: "#60a5fa" },
  { stage: "preprod", names: ["preprod", "pre-prod", "preproduction", "canary"], light: "#7c3aed", dark: "#a78bfa" },
  { stage: "staging", names: ["staging", "stage", "stg"], light: "#c026d3", dark: "#e879f9" },
  { stage: "test", names: ["test", "testing", "qa", "sandbox"], light: "#0891b2", dark: "#22d3ee" },
  { stage: "dev", names: ["dev", "development", "local"], light: "#0d9488", dark: "#2dd4bf" },
];

const UNRANKED = STAGES.length;

/**
 * Hues left for environments that name no stage — `infrastructure-hetzner`,
 * `finance`, an org's own coinage. Still cool, still in one lightness band, so
 * they read as peers of each other and never as an alert.
 */
const UNSTAGED = [
  ["#0284c7", "#38bdf8"], // sky
  ["#4f46e5", "#818cf8"], // indigo
  ["#0d9488", "#5eead4"], // teal, lighter than the dev teal
  ["#7e22ce", "#c084fc"], // purple
  ["#0369a1", "#7dd3fc"], // deep sky
  ["#5b21b6", "#a5b4fc"], // deep violet
];

/** Warm is reserved. These are the only warm colours in the timeline. */
export const SIGNAL = {
  /** Parked on a person, or travelling backwards. */
  attention: { light: "#d97706", dark: "#fbbf24" },
  /** Failed, timed out. */
  failure: { light: "#dc2626", dark: "#f87171" },
  /** A stage completed. The tick, never a lane. */
  success: { light: "#059669", dark: "#34d399" },
};

const NEUTRAL = ["#64748b", "#94a3b8"];

/** Split `data-prod` into its segments, longest-suffix-first. */
function segments(name) {
  return String(name || "")
    .toLowerCase()
    .split(/[-_\s./]+/)
    .filter(Boolean);
}

/**
 * Which stage an environment belongs to, or null.
 *
 * Checks the last segment first, then the first, then the whole name. The last
 * segment wins because forest's environments are named `<domain>-<stage>` —
 * `data-prod`, `platform-dev` — so the tail is the part that says what the
 * environment *is*.
 */
export function envStage(name) {
  const parts = segments(name);
  if (parts.length === 0) return null;
  const whole = parts.join("-");
  const candidates = [parts[parts.length - 1], parts[0], whole];
  for (const candidate of candidates) {
    const hit = STAGES.find((s) => s.names.includes(candidate));
    if (hit) return hit.stage;
  }
  return null;
}

/**
 * Lane order: production first, then back down the pipeline, with anything
 * unstaged after. The card column sits to the right of the gutter, so code
 * flows right-to-left — from the commit that caused it out to production.
 *
 * Ties keep the server's environment order, which is the caller's array order.
 */
export function envRank(name) {
  const stage = envStage(name);
  if (!stage) return UNRANKED;
  return STAGES.findIndex((s) => s.stage === stage);
}

/** Stable small hash, so an unstaged environment keeps its colour run to run. */
function hash(name) {
  let h = 0;
  for (const ch of String(name || "")) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
  return h;
}

/**
 * `[light, dark]` for an environment.
 *
 * Both are returned rather than one resolved value because the component hands
 * them to CSS custom properties and lets a media query pick — a colour resolved
 * in JS at mount time would be stuck on whatever the theme was then.
 */
export function envColorPair(name) {
  const stage = envStage(name);
  if (stage) {
    const hit = STAGES.find((s) => s.stage === stage);
    return [hit.light, hit.dark];
  }
  if (!name) return NEUTRAL;
  return UNSTAGED[hash(name) % UNSTAGED.length];
}

/**
 * Sort a list of `{ name }` lanes into gutter order.
 * Stable: equal ranks keep their incoming order.
 */
export function orderLanes(lanes) {
  return [...(lanes || [])]
    .map((lane, i) => ({ lane, i, rank: envRank(lane.name) }))
    .sort((a, b) => a.rank - b.rank || a.i - b.i)
    .map((entry) => entry.lane);
}

// ── Chips ─────────────────────────────────────────────────────────────────

/**
 * Inline style for an environment chip: its colour pair, for CSS to pick from.
 *
 * A style string rather than Tailwind classes because the palette is computed —
 * a class-per-environment table cannot cover an org's own environment names,
 * which is what the old `envBadgeClasses` ran into. It fell back to grey for
 * every environment it had not been told about by name, so a real deployment
 * target looked like an unknown one.
 */
export function envChipStyle(name) {
  const [light, dark] = envColorPair(name);
  return `--env: ${light}; --env-dark: ${dark};`;
}
