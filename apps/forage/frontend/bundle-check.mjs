import { chromium } from "playwright";
import { writeFileSync, rmSync } from "node:fs";
import { join } from "node:path";

const DIR = join(process.cwd(), "..", "static", "js", "components");
const page_html = join(DIR, "__bundlecheck.html");

const PREFETCH_URL_ESCAPED = "&#x2f;api&#x2f;orgs&#x2f;understory&#x2f;projects&#x2f;demo&#x2f;timeline";
writeFileSync(page_html, `<!doctype html><meta charset="utf-8">
<div id="host"></div>
<script data-timeline-url="${PREFETCH_URL_ESCAPED}">
(function () {
    var el = document.currentScript;
    if (!el) return;
    var url = el.getAttribute("data-timeline-url");
    var store = (window.__forestTimeline = window.__forestTimeline || {});
    store[url] = fetch(url, { credentials: "same-origin" })
        .then(function (r) { return r.ok ? r.json() : null; })
        .catch(function () { return null; });
})();
</script>
<script src="./forage-components.js"></script>`);

const browser = await chromium.launch();
const page = await browser.newPage();

await page.addInitScript(() => {
  const payload = {
    lanes: [{ name: "dev" }, { name: "prod" }],
    timeline: [
      { kind: "release", release: {
        slug: "a", title: "A release", source_user: "octobot", has_pipeline: true,
        created_at: new Date().toISOString(), commit_sha: "abc1234", branch: "main",
        dest_envs: "dev:SUCCEEDED,prod:RUNNING",
        destinations: [
          { name: "dev-main", environment: "dev", status: "SUCCEEDED", is_current: true },
          { name: "prod-main", environment: "prod", status: "RUNNING", is_current: false },
        ],
        env_groups: [],
        pipeline_stages: [
          { id: "1", stage_type: "deploy", environment: "dev", status: "SUCCEEDED" },
          { id: "2", stage_type: "deploy", environment: "prod", status: "RUNNING" },
        ],
      }},
      { kind: "release", release: {
        slug: "b", title: "An older release", source_user: "kjuulh", has_pipeline: true,
        created_at: new Date(Date.now() - 3.6e6).toISOString(), commit_sha: "def5678", branch: "main",
        dest_envs: "dev:SUCCEEDED,prod:SUCCEEDED",
        destinations: [
          { name: "dev-main", environment: "dev", status: "SUCCEEDED", is_current: false },
          { name: "prod-main", environment: "prod", status: "SUCCEEDED", is_current: true },
        ],
        env_groups: [],
        pipeline_stages: [
          { id: "3", stage_type: "deploy", environment: "dev", status: "SUCCEEDED" },
          { id: "4", stage_type: "deploy", environment: "prod", status: "SUCCEEDED" },
        ],
      }},
    ],
  };
  window.__fetches = [];
  window.fetch = async (input) => {
    window.__fetches.push(typeof input === "string" ? input : input.url);
    return new Response(JSON.stringify(payload), {
      status: 200, headers: { "Content-Type": "application/json" },
    });
  };
  class Dead { constructor(){this.readyState=0;} addEventListener(){} removeEventListener(){} close(){} }
  window.EventSource = Dead;
});

const errors = [];
page.on("pageerror", (e) => errors.push(String(e)));
await page.goto("file://" + page_html, { waitUntil: "load" });

const defined = await page.evaluate(() => Boolean(customElements.get("release-timeline")));
await page.evaluate(() => {
  const el = document.createElement("release-timeline");
  el.setAttribute("org", "understory");
  el.setAttribute("project", "demo");
  document.getElementById("host").appendChild(el);
});
await page.waitForTimeout(1500);

const seen = await page.evaluate(() => ({
  cards: document.querySelectorAll("[data-release]").length,
  lanes: document.querySelectorAll(".rt-lane").length,
  runs: document.querySelectorAll(".lane-run").length,
  dots: document.querySelectorAll(".lane-dot").length,
  laneStates: [...document.querySelectorAll("[data-release]")].map((c) => c.dataset.laneStates),
  laneWidth: document.querySelector(".rt-lane")?.getBoundingClientRect().width ?? 0,
  fetches: window.__fetches.filter((u) => u.includes("/timeline")),
}));

await browser.close();
rmSync(page_html, { force: true });

const fail = [];
if (!defined) fail.push("custom element `release-timeline` was never defined");
if (errors.length) fail.push(`page errors: ${errors.join(" | ")}`);
if (seen.cards !== 2) fail.push(`expected 2 cards, got ${seen.cards}`);
if (seen.lanes !== 2) fail.push(`expected 2 lanes, got ${seen.lanes}`);
if (seen.runs === 0) fail.push("no lane runs drawn");
if (seen.dots === 0) fail.push("no lane dots drawn");
if (seen.laneWidth < 4) fail.push(`lane has no width (${seen.laneWidth}) — styles missing`);
if (seen.laneStates[0] !== "dev:live,prod:flight") fail.push(`lane states wrong: ${JSON.stringify(seen.laneStates)}`);
if (seen.fetches.length !== 1) {
  fail.push(`expected the page's single prefetch to serve the component, got ${seen.fetches.length}: ${JSON.stringify(seen.fetches)}`);
}
if (seen.fetches[0] !== "/api/orgs/understory/projects/demo/timeline") {
  fail.push(`prefetch URL did not decode: ${JSON.stringify(seen.fetches[0])}`);
}

console.log(JSON.stringify({ defined, ...seen }, null, 2));
if (fail.length) { console.error("\nFAIL:\n  " + fail.join("\n  ")); process.exit(1); }
console.log("\nthe built bundle defines and renders the component");
