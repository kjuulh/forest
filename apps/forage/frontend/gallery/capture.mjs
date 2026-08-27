/**
 * Render every lane-state fixture in a real browser, assert what the DOM
 * actually shows, and save a screenshot per state.
 *
 * This is the half the unit tests cannot cover. `lane-states.test.js` proves the
 * state *resolution* is right; this proves the component *renders* each state
 * distinguishably — which is where the original bug lived: the logic said
 * "pending", and pending happened to look finished.
 *
 *   npm run gallery:capture
 *
 * Starts the gallery server itself unless GALLERY_URL points at one already, so
 * it is a single command locally and in CI. Requiring a second terminal is the
 * kind of friction that keeps a suite from being run.
 */
import { chromium } from "playwright";
import { spawn } from "node:child_process";
import { FIXTURES } from "../src/lib/fixtures.js";
import { isUnfinished } from "../src/lib/lane-states.js";
import { mkdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const SHOTS = join(HERE, "shots");
const EXTERNAL = process.env.GALLERY_URL;
const URL = EXTERNAL || "http://localhost:5178/";

mkdirSync(SHOTS, { recursive: true });

async function reachable(url, ms) {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    try {
      const res = await fetch(url, { signal: AbortSignal.timeout(1500) });
      if (res.ok) return true;
    } catch {}
    await new Promise((r) => setTimeout(r, 250));
  }
  return false;
}

// Start the server unless one is already there. `vite` writes to stderr on
// startup, so its output is only surfaced when it fails to come up.
let server = null;
if (!EXTERNAL && !(await reachable(URL, 1000))) {
  // The vite binary directly, not `npx vite`. Through npx, vite is a
  // *grandchild*: killing the child kills npx and leaves vite running, holding
  // its piped stdio open, and node then never exits. That hung a CI job for
  // eleven minutes on a seven-second task — and did not reproduce on macOS,
  // where the process tree collapses differently.
  //
  // `detached` puts it in its own process group so the whole group can be
  // signalled, in case vite ever spawns children of its own.
  const vite = join(HERE, "..", "node_modules", ".bin", "vite");
  server = spawn(vite, ["--config", "vite.gallery.config.js"], {
    cwd: join(HERE, ".."), stdio: ["ignore", "pipe", "pipe"], detached: true,
  });
  const log = [];
  server.stdout.on("data", (d) => log.push(d.toString()));
  server.stderr.on("data", (d) => log.push(d.toString()));
  if (!(await reachable(URL, 30000))) {
    server.kill();
    console.error(`gallery server never came up at ${URL}\n${log.join("")}`);
    process.exit(1);
  }
}
const stopServer = () => {
  if (!server || server.killed) return;
  try {
    process.kill(-server.pid, "SIGTERM");  // the group, not just the leader
  } catch {
    server.kill("SIGTERM");
  }
};

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
        // What each card put in its avatar slot. The lane dots anchor to
        // [data-avatar], so the tag matters less than the fact that every card
        // still has one — see the assertion below.
        avatars: cards.map((c) => {
          const a = c.querySelector("[data-avatar]");
          return a ? a.tagName.toLowerCase() : null;
        }),
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

    // 3. Every card keeps a [data-avatar] anchor, and the slot renders both
    //    ways: kjuulh's picture as an <img>, octobot's missing one as the
    //    <span> holding their initial. Losing the anchor does not look broken
    //    — the lane dots quietly re-anchor to the whole card and drift.
    if (o.avatars.includes(null)) {
      failures.push(`${f.key}: a card has no [data-avatar] lane anchor — got ${JSON.stringify(o.avatars)}`);
    }
    if (!o.avatars.includes("img") || !o.avatars.includes("span")) {
      failures.push(`${f.key}: expected both a picture and a fallback avatar, got ${JSON.stringify(o.avatars)}`);
    }

    // 4. An awaiting state must say so, in words, on hover.
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
  stopServer();
}

if (failures.length) {
  console.error(`\n${failures.length} failure(s):`);
  for (const f of failures) console.error(`  ✗ ${f}`);
  process.exit(1);
}
console.log(`\nall ${FIXTURES.length} states render distinguishably; screenshots in gallery/shots/`);
// Explicit: a stray handle must not turn a passing run into a hung job.
process.exit(0);
