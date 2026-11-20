const STAGES = [
  { stage: "prod", names: ["prod", "production", "live"], light: "#2563eb", dark: "#60a5fa" },
  { stage: "preprod", names: ["preprod", "pre-prod", "preproduction", "canary"], light: "#7c3aed", dark: "#a78bfa" },
  { stage: "staging", names: ["staging", "stage", "stg"], light: "#c026d3", dark: "#e879f9" },
  { stage: "test", names: ["test", "testing", "qa", "sandbox"], light: "#0891b2", dark: "#22d3ee" },
  { stage: "dev", names: ["dev", "development", "local"], light: "#0d9488", dark: "#2dd4bf" },
];

const UNRANKED = STAGES.length;

const UNSTAGED = [
  ["#0284c7", "#38bdf8"],
  ["#4f46e5", "#818cf8"],
  ["#0d9488", "#5eead4"],
  ["#7e22ce", "#c084fc"],
  ["#0369a1", "#7dd3fc"],
  ["#5b21b6", "#a5b4fc"],
];

export const SIGNAL = {
  attention: { light: "#d97706", dark: "#fbbf24" },
  failure: { light: "#dc2626", dark: "#f87171" },
  success: { light: "#059669", dark: "#34d399" },
};

const NEUTRAL = ["#64748b", "#94a3b8"];

function segments(name) {
  return String(name || "")
    .toLowerCase()
    .split(/[-_\s./]+/)
    .filter(Boolean);
}

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

export function envRank(name) {
  const stage = envStage(name);
  if (!stage) return UNRANKED;
  return STAGES.findIndex((s) => s.stage === stage);
}

function hash(name) {
  let h = 0;
  for (const ch of String(name || "")) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
  return h;
}

export function envColorPair(name) {
  const stage = envStage(name);
  if (stage) {
    const hit = STAGES.find((s) => s.stage === stage);
    return [hit.light, hit.dark];
  }
  if (!name) return NEUTRAL;
  return UNSTAGED[hash(name) % UNSTAGED.length];
}

export function orderLanes(lanes) {
  return [...(lanes || [])]
    .map((lane, i) => ({ lane, i, rank: envRank(lane.name) }))
    .sort((a, b) => a.rank - b.rank || a.i - b.i)
    .map((entry) => entry.lane);
}

export function envChipStyle(name) {
  const [light, dark] = envColorPair(name);
  return `--env: ${light}; --env-dark: ${dark};`;
}
