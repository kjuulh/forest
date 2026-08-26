/**
 * Render every lane-state fixture in a real browser, assert what the DOM
 * actually shows, and save a screenshot per state.
 *
 * This is the half the unit tests cannot cover. `lane-states.test.js` proves the
 * state *resolution* is right; this proves the component *renders* each state
 * distinguishably — which is where the original bug lived: the logic said
 * "pending", and pending happened to look finished.
 *
 *   npm run gallery            # serve (separate terminal)
 *   node gallery/capture.mjs   # assert + screenshot
 */
import { chromium } from "playwright";
import { FIXTURES } from "../src/lib/fixtures.js";
import { isUnfinished } from "../src/lib/lane-states.js";
import { mkdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const SHOTS = join(HERE, "shots");
const URL = process.env.GALLERY_URL || "http://localhost:5178/";

mkdirSync(SHOTS, { recursive: true });

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1180, height: 900 }, deviceScaleFactor: 2 });

const failures = [];
try {
  await page.goto(URL, { waitUntil: "load" });
  await page.waitForSelector("body[data-gallery-ready='true']", { timeout: 20000 });

  // Freeze the pulse animation so screenshots are byte-stable between runs
  // rather than catching the keyframe at a random phase.
  await page.addStyleTag({ content: `.lane-pulse { animation: none !important; }` });

  const observed = await page.evaluate(() => {
    const out = {};
    for (const s of document.querySelectorAll(".fixture")) {
      const el = s.querySelector("release-timeline");
      const sr = el?.shadowRoot ?? el;
      const cards = [...(sr?.querySelectorAll("[data-release]") ?? [])];
      out[s.dataset.fixture] = {
        laneStates: cards[0]?.dataset.laneStates ?? "",
        dotTitles: [...(sr?.querySelectorAll(".lane-dot") ?? [])].map((d) => d.getAttribute("title")),
        hatched: [...(sr?.querySelectorAll(".lane-bar") ?? [])]
          .filter((b) => (b.style.backgroundImage || "").includes("svg")).length,
        pulsing: (sr?.querySelectorAll(".lane-pulse") ?? []).length,
      };
    }
    return out;
  });

  for (const f of FIXTURES) {
    const o = observed[f.key];
    if (!o) { failures.push(`${f.key}: not rendered`); continue; }

    // 1. The resolved state reached the DOM.
    for (const [env, kind] of Object.entries(f.expect)) {
      if (!o.laneStates.split(",").includes(`${env}:${kind}`)) {
        failures.push(`${f.key}: expected ${env}:${kind} in data-lane-states, got "${o.laneStates}"`);
      }
    }

    // 2. Unfinished states animate; finished ones do not. This is the invariant
    //    the bug violated — a parked pipeline that rendered as settled.
    const wantsMotion = Object.values(f.expect).some(isUnfinished);
    if (wantsMotion && (o.hatched === 0 || o.pulsing === 0)) {
      failures.push(`${f.key}: unfinished but nothing animates (hatched=${o.hatched} pulsing=${o.pulsing})`);
    }
    if (!wantsMotion && (o.hatched > 0 || o.pulsing > 0)) {
      failures.push(`${f.key}: finished but something animates (hatched=${o.hatched} pulsing=${o.pulsing})`);
    }

    // 3. An awaiting state must say so, in words, on hover.
    if (Object.values(f.expect).includes("awaiting")) {
      if (!o.dotTitles.some((t) => (t || "").startsWith("Awaiting approval for"))) {
        failures.push(`${f.key}: no "Awaiting approval" dot title — got ${JSON.stringify(o.dotTitles)}`);
      }
    }

    await page.locator(`section[data-fixture="${f.key}"]`).screenshot({
      path: join(SHOTS, `${f.key}.png`),
    });
    console.log(`  ${failures.length ? "·" : "✓"} ${f.key.padEnd(22)} ${o.laneStates}`);
  }
} finally {
  await browser.close();
}

if (failures.length) {
  console.error(`\n${failures.length} failure(s):`);
  for (const f of failures) console.error(`  ✗ ${f}`);
  process.exit(1);
}
console.log(`\nall ${FIXTURES.length} states render distinguishably; screenshots in gallery/shots/`);
