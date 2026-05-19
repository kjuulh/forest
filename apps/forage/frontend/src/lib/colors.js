/** Environment-to-color mapping (matches swim-lanes.js + platform.rs) */
const ENV_COLORS = {
  prod: ["#ec4899", "#fce7f3"],
  production: ["#ec4899", "#fce7f3"],
  preprod: ["#f97316", "#ffedd5"],
  "pre-prod": ["#f97316", "#ffedd5"],
  staging: ["#eab308", "#fef9c3"],
  stage: ["#eab308", "#fef9c3"],
  dev: ["#8b5cf6", "#ede9fe"],
  development: ["#8b5cf6", "#ede9fe"],
  test: ["#06b6d4", "#cffafe"],
};

const DEFAULT_COLORS = ["#6b7280", "#e5e7eb"];

export function envColors(name) {
  const lower = name.toLowerCase();
  if (ENV_COLORS[lower]) return ENV_COLORS[lower];
  for (const [key, colors] of Object.entries(ENV_COLORS)) {
    if (lower.includes(key)) return colors;
  }
  return DEFAULT_COLORS;
}

export function envLaneColor(name) {
  return envColors(name)[0];
}

export function envBadgeClasses(env) {
  const lower = env.toLowerCase();
  if (lower.includes("prod") && !lower.includes("preprod") && !lower.includes("pre-prod")) {
    return { bg: "bg-pink-100 text-pink-800", dot: "bg-pink-500" };
  }
  if (lower.includes("preprod") || lower.includes("pre-prod")) {
    return { bg: "bg-orange-100 text-orange-800", dot: "bg-orange-500" };
  }
  if (lower.includes("stag")) {
    return { bg: "bg-yellow-100 text-yellow-800", dot: "bg-yellow-500" };
  }
  if (lower.includes("dev")) {
    return { bg: "bg-violet-100 text-violet-800", dot: "bg-violet-500" };
  }
  return { bg: "bg-gray-100 text-gray-700", dot: "bg-gray-400" };
}

export function statusDotColor(status) {
  switch (status) {
    case "SUCCEEDED": return "bg-green-500";
    case "RUNNING": return "bg-yellow-500";
    case "FAILED": return "bg-red-500";
    default: return null;
  }
}
