/**
 * Every lane state the timeline can be in, on one page.
 *
 * This is what `npm run gallery:capture` asserts against, and what to open when
 * you want to see a state rather than read about it. It mounts the REAL
 * <release-timeline> once per fixture with fetch and EventSource stubbed — a
 * mockup would happily show the states we intended rather than the ones the
 * component renders, which is the whole thing it exists to catch.
 *
 * It renders with the app's own stylesheet, so a colour checked here is the
 * colour that ships, and the toolbar switches the app's dark palette (see
 * gallery/dark-mirror.js) because that palette is otherwise unreachable from a
 * page.
 */
import { FIXTURES } from "../src/lib/fixtures.js";
import "../src/main.js";

// ── Toolbar ───────────────────────────────────────────────────────────────

const stage = document.getElementById("gallery");

// The app themes on `prefers-color-scheme` alone, which a page cannot toggle.
// `gallery/dark-mirror.js` re-serves the app's own dark block scoped to
// `[data-theme="dark"]` so this button drives the real palette, not a copy.
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
      /* private window — the button still works, it just won't be remembered */
    }
  };

  for (const b of buttons) b.addEventListener("click", () => select(b.dataset.value));
  select(buttons.some((b) => b.dataset.value === stored) ? stored : "light");
}

// ── Stubbed server ────────────────────────────────────────────────────────
//
// Each fixture mounts its own <release-timeline> under its own project name, so
// the stub can answer per fixture rather than relying on mount order. The
// previous version swapped a single shared payload between mounts and slept
// between them, which raced whenever a mount was slow.

// Deliberately *not* in gutter order. The server sends environments in its own
// order and the component is what sorts them production-first, so a fixture
// that arrives pre-sorted would let a broken sort pass unnoticed.
const LANES = [{ name: "dev" }, { name: "prod" }, { name: "staging" }];

/** project name → payload */
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
  // Avatars are served by a vite middleware; everything else falls through so a
  // genuine 404 still reads as a 404.
  return realFetch(input, init);
};

// The component opens an EventSource on mount. There is no server behind these
// fixtures, so it gets one that never delivers anything.
const sources = new Map(); // project name → Set<FakeEventSource>

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

// ── Fixture payloads ──────────────────────────────────────────────────────

/**
 * The placements a fixture talks about, so the release underneath it can hold
 * exactly those.
 *
 * A fixed baseline was wrong in a way that showed: it always deployed to three
 * regional prod destinations, so a fixture whose own release used a single
 * `prod-main` made the prod lane look like an environment with *four* distinct
 * placements that never overlap — and nothing in the picture was actually
 * superseding anything.
 *
 * Environments a fixture reaches only through a pipeline stage get a
 * `<env>-main` placement, so every lane on screen has a prior release beneath
 * it. A lane with nothing under it is a legitimate state, but it is not the one
 * these fixtures are about.
 */
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

// One release per fixture is not enough to see the swim-lane bar: the bar is
// drawn *between* releases, so each entry renders its fixture on top of a
// settled release beneath it.
//
// `is_current` true throughout: whatever the fixture has taken over gets
// demoted to `past` by the resolver, and whatever it has not — a placement it
// failed on, say — is still held here. Hard-coding `false` would tell the
// gutter that nothing holds this release, and a failed placement would have no
// earlier release to fall back to.
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
    // Two deployers on purpose: kjuulh has a picture behind the stub avatar
    // endpoint, octobot does not, so every shot carries both halves of the
    // avatar slot — the image and the initial it falls back to.
    "kjuulh",
  ),
});

/** A stable, plausible-looking sha. Fixtures are read by people. */
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

// `dest_envs` is a server-side string the rail reads back off `data-envs`;
// derive it from the destinations so a fixture only has to state them once.
function withDefaults(release, title, version, minutesAgo, sourceUser) {
  return {
    title,
    version,
    source_user: sourceUser,
    commit_sha: shaFor(version + title),
    branch: "main",
    // Relative to now, not a fixed date: a hard-coded timestamp drifts into
    // the future and every card in the gallery reads "just now".
    created_at: new Date(Date.now() - minutesAgo * 60_000).toISOString(),
    dest_envs: (release.destinations || [])
      .map((d) => `${d.environment}:${d.status || "PENDING"}`)
      .join(","),
    ...release,
  };
}

function payloadFor(fixture) {
  const top = withDefaults(fixture.release, fixture.title, "main-1111111", 4, "octobot");
  // Older releases the fixture wants underneath it, newest first. The gutter
  // resolves lane states across the whole list, so a superseded state only
  // exists when there is something above it to do the superseding.
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

// ── Render ────────────────────────────────────────────────────────────────

for (const f of FIXTURES) {
  payloads.set(f.key, payloadFor(f));
  mount(f.key, f);
}

(async function ready() {
  // Lane runs measure the DOM on rAF; give every mount a couple of frames to
  // fetch and settle before anything screenshots the page.
  await new Promise((r) => setTimeout(r, 600));
  document.body.setAttribute("data-gallery-ready", "true");
})();
