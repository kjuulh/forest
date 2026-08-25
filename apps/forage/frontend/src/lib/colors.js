/** Environment-to-color mapping (matches swim-lanes.js + platform.rs) */
const ENV_COLORS = {
  "platform-dev": ["#6366f1", "#e0e7ff"],
  "platform_dev": ["#6366f1", "#e0e7ff"],
  "platform dev": ["#6366f1", "#e0e7ff"],
  prod: ["#ec4899", "#fce7f3"],
  production: ["#ec4899", "#fce7f3"],
  preprod: ["#f97316", "#ffedd5"],
  "pre-prod": ["#f97316", "#ffedd5"],
  staging: ["#eab308", "#fef9c3"],
  stage: ["#eab308", "#fef9c3"],
  dev: ["#8b5cf6", "#ede9fe"],
  data: ["#0ea5e9", "#e0f2fe"],
  finance: ["#f59e0b", "#fef3c7"],
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
  if (lower.includes("platform-dev") || lower.includes("platform_dev") || lower.includes("platform dev")) {
    return { bg: "bg-indigo-100 text-indigo-800", dot: "bg-indigo-500" };
  }
  if (lower.includes("dev")) {
    return { bg: "bg-violet-100 text-violet-800", dot: "bg-violet-500" };
  }
  if (lower.includes("data")) {
    return { bg: "bg-sky-100 text-sky-800", dot: "bg-sky-500" };
  }
  if (lower.includes("finance")) {
    return { bg: "bg-amber-100 text-amber-800", dot: "bg-amber-500" };
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
