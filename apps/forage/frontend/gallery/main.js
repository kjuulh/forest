import { FIXTURES } from "../src/lib/fixtures.js";
import "../src/main.js";

const stage = document.getElementById("gallery");

{
  const group = document.querySelector('[data-control="theme"]');
  const buttons = [...group.querySelectorAll("button")];
  const stored = (() => {
    try {
      return localStorage.getItem("gallery:theme");
    } catch {
      return null;
    }
  })();

  const select = (value) => {
    document.documentElement.setAttribute("data-theme", value);
    for (const b of buttons) b.setAttribute("aria-pressed", String(b.dataset.value === value));
    try {
      localStorage.setItem("gallery:theme", value);
    } catch {
    }
  };

  for (const b of buttons) b.addEventListener("click", () => select(b.dataset.value));
  select(buttons.some((b) => b.dataset.value === stored) ? stored : "light");
}

const LANES = [{ name: "dev" }, { name: "prod" }, { name: "staging" }];

const payloads = new Map();

const realFetch = window.fetch.bind(window);

window.fetch = async (input, init) => {
  const url = typeof input === "string" ? input : input.url;
  const match = /\/api\/orgs\/[^/]+\/projects\/([^/]+)\/timeline/.exec(url);
  if (match) {
    const payload = payloads.get(decodeURIComponent(match[1]));
    return new Response(JSON.stringify(payload ?? { timeline: [], lanes: [] }), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  }
  return realFetch(input, init);
};

const sources = new Map();

class FakeEventSource {
  constructor(url) {
    this.url = url;
    this.readyState = 1;
    this.listeners = new Map();
    const project = /\/projects\/([^/]+)\/events/.exec(url)?.[1];
    this.project = project ? decodeURIComponent(project) : null;
    if (this.project) {
      if (!sources.has(this.project)) sources.set(this.project, new Set());
      sources.get(this.project).add(this);
    }
  }
  addEventListener(type, fn) {
    if (!this.listeners.has(type)) this.listeners.set(type, new Set());
    this.listeners.get(type).add(fn);
  }
  removeEventListener(type, fn) {
    this.listeners.get(type)?.delete(fn);
  }
  close() {
    this.readyState = 2;
    sources.get(this.project)?.delete(this);
  }
  emit(type, data) {
    for (const fn of this.listeners.get(type) ?? []) fn({ data: JSON.stringify(data) });
  }
}
window.EventSource = FakeEventSource;

function push(project, type, data) {
  for (const es of sources.get(project) ?? []) es.emit(type, data);
}

function placementsFor(fixture) {
  const releases = [fixture.release, ...(fixture.below || [])];
  const byName = new Map();
  const envs = new Set();

  for (const r of releases) {
    for (const d of r?.destinations || []) {
      if (!d.name || !d.environment) continue;
      envs.add(d.environment);
      byName.set(d.name, d.environment);
    }
    for (const stage of r?.pipeline_stages || []) {
      if (stage.environment) envs.add(stage.environment);
    }
  }

  for (const env of envs) {
    const covered = [...byName.values()].includes(env);
    if (!covered) byName.set(`${env}-main`, env);
  }
  if (byName.size === 0) {
    for (const env of ["dev", "staging", "prod"]) byName.set(`${env}-main`, env);
  }
  return [...byName].map(([name, environment]) => ({ name, environment }));
}

const baseline = (fixture) => ({
  kind: "release",
  release: withDefaults(
    {
      slug: "baseline",
      has_pipeline: true,
      destinations: placementsFor(fixture).map((p) => ({
        ...p,
        status: "SUCCEEDED",
        is_current: true,
      })),
      pipeline_stages: [...new Set(placementsFor(fixture).map((p) => p.environment))].map(
        (environment, i) => ({
          id: `b${i}`,
          stage_type: "deploy",
          environment,
          status: "SUCCEEDED",
        }),
      ),
    },
    "Bump opentelemetry-collector to 0.108.0",
    "main-0000000",
    240,
    "kjuulh",
  ),
});

function shaFor(seed) {
  let h = 0x811c9dc5;
  for (const ch of seed) h = Math.imul(h ^ ch.charCodeAt(0), 0x01000193) >>> 0;
  let out = "";
  for (let i = 0; i < 10; i++) {
    h = Math.imul(h ^ (h >>> 13), 0x5bd1e995) >>> 0;
    out += (h % 16).toString(16);
  }
  return out;
}

function withDefaults(release, title, version, minutesAgo, sourceUser) {
  return {
    title,
    version,
    source_user: sourceUser,
    commit_sha: shaFor(version + title),
    branch: "main",
    created_at: new Date(Date.now() - minutesAgo * 60_000).toISOString(),
    dest_envs: (release.destinations || [])
      .map((d) => `${d.environment}:${d.status || "PENDING"}`)
      .join(","),
    ...release,
  };
}

function payloadFor(fixture) {
  const top = withDefaults(fixture.release, fixture.title, "main-1111111", 4, "octobot");
  const below = (fixture.below || []).map((rel, i) => ({
    kind: "release",
    release: withDefaults(
      rel,
      rel.title || "Earlier release",
      `main-22222${i}2`,
      30 * (i + 1),
      "kjuulh",
    ),
  }));
  return {
    timeline: [{ kind: "release", release: top }, ...below, ...(fixture.extra || []), baseline(fixture)],
    lanes: fixture.lanes || LANES,
  };
}

function mount(key, { title, why, expect }) {
  const section = document.createElement("section");
  section.className = "fixture";
  section.setAttribute("data-fixture", key);
  section.innerHTML = `
    <h2>${title}</h2>
    <p class="why">${why}</p>
    <p class="expect">${Object.entries(expect || {})
      .map(([e, k]) => `<code>${e} → ${k}</code>`)
      .join("")}</p>
  `;

  const el = document.createElement("release-timeline");
  el.setAttribute("org", "understory");
  el.setAttribute("project", key);
  el.setAttribute("username", "kjuulh");
  el.setAttribute("role", "admin");
  el.setAttribute("csrf", "gallery");
  section.appendChild(el);
  stage.appendChild(section);
  return el;
}

for (const f of FIXTURES) {
  payloads.set(f.key, payloadFor(f));
  mount(f.key, f);
}

(async function ready() {
  await new Promise((r) => setTimeout(r, 600));
  document.body.setAttribute("data-gallery-ready", "true");
})();
