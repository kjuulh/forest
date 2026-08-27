/**
 * Visual gallery for the release timeline's lane states.
 *
 * Mounts the REAL <release-timeline> component — not a mockup — once per
 * fixture, with fetch and EventSource stubbed. A mockup would happily show the
 * states we intended rather than the ones the component actually renders, which
 * defeats the point.
 */
import { FIXTURES } from "../src/lib/fixtures.js";
import "../src/main.js";

const LANES = [{ name: "dev" }, { name: "prod" }];

// One release per fixture is not enough to see the swim-lane bar: the bar is
// drawn *between* releases, so each gallery entry renders its fixture on top of
// a settled release beneath it.
const BASELINE = {
  kind: "release",
  release: {
    slug: "baseline",
    title: "Earlier release (already live)",
    // Two deployers on purpose: kjuulh has a picture behind the stub avatar
    // endpoint, octobot does not, so every shot carries both halves of the
    // avatar slot — the image and the initial it falls back to.
    source_user: "kjuulh",
    version: "main-0000000",
    has_pipeline: true,
    created_at: new Date(Date.UTC(2026, 7, 26, 9, 0, 0)).toISOString(),
    destinations: [
      { name: "dev-dest", environment: "dev", status: "SUCCEEDED", is_current: false },
      { name: "prod-dest", environment: "prod", status: "SUCCEEDED", is_current: false },
    ],
    dest_envs: "dev:SUCCEEDED,prod:SUCCEEDED",
    pipeline_stages: [
      { id: "b1", stage_type: "plan", environment: "dev", status: "SUCCEEDED" },
      { id: "b2", stage_type: "deploy", environment: "dev", status: "SUCCEEDED" },
      { id: "b3", stage_type: "plan", environment: "prod", status: "SUCCEEDED" },
      { id: "b4", stage_type: "deploy", environment: "prod", status: "SUCCEEDED" },
    ],
  },
};

// `dest_envs` is a server-side string the rail reads back off `data-envs`;
// derive it from the destinations so a fixture only has to state them once.
//
// `source_user` picks which half of the avatar slot a card exercises: octobot
// has no picture behind the stub endpoint and falls back to an initial, kjuulh
// has one. BASELINE is kjuulh, so every shot carries both.
function withDefaults(release, title, version, minutesAgo, sourceUser) {
  return {
    title,
    version,
    source_user: sourceUser,
    created_at: new Date(Date.UTC(2026, 7, 26, 13, 30 - minutesAgo, 0)).toISOString(),
    dest_envs: (release.destinations || [])
      .map((d) => `${d.environment}:${d.status || "PENDING"}`)
      .join(","),
    ...release,
  };
}

function payloadFor(fixture) {
  const r = withDefaults(fixture.release, fixture.title, "main-1111111", 0, "octobot");
  // Older releases the fixture wants underneath it, newest first. The gutter
  // resolves lane states across the whole list, so a superseded state only
  // exists when there is something above it to do the superseding.
  const below = (fixture.below || []).map((rel, i) => ({
    kind: "release",
    release: withDefaults(
      rel, rel.title || "Earlier release", `main-22222${i}2`, 30 * (i + 1), "kjuulh",
    ),
  }));
  return { timeline: [{ kind: "release", release: r }, ...below, BASELINE], lanes: LANES };
}

// Stub the network. The component calls fetch() for the timeline and opens an
// EventSource for live updates; neither exists here.
let current = payloadFor(FIXTURES[0]);
window.fetch = async () => new Response(JSON.stringify(current), {
  status: 200, headers: { "Content-Type": "application/json" },
});
class DeadEventSource {
  constructor() { this.readyState = 0; }
  addEventListener() {} removeEventListener() {} close() {}
}
window.EventSource = DeadEventSource;

const root = document.getElementById("gallery");

(async function render() {
  for (const f of FIXTURES) {
    current = payloadFor(f);

    const section = document.createElement("section");
    section.className = "fixture";
    section.setAttribute("data-fixture", f.key);
    section.innerHTML = `
      <h2>${f.title}</h2>
      <p class="why">${f.why}</p>
      <p class="expect">${Object.entries(f.expect).map(([e, k]) => `<code>${e} → ${k}</code>`).join(" ")}</p>
    `;

    const el = document.createElement("release-timeline");
    el.setAttribute("org", "understory");
    el.setAttribute("project", "infrastructure-hetzner");
    el.setAttribute("username", "kjuulh");
    el.setAttribute("role", "admin");
    el.setAttribute("csrf", "gallery");
    section.appendChild(el);
    root.appendChild(section);

    // Let this instance fetch before the next one swaps `current`.
    await new Promise((r) => setTimeout(r, 120));
  }
  // Lane bars measure the DOM on rAF; give them a couple of frames to settle
  // before anything screenshots the page.
  await new Promise((r) => setTimeout(r, 900));
  document.body.setAttribute("data-gallery-ready", "true");
})();
