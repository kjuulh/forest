import { chromium, firefox, webkit } from "playwright";
import { spawn } from "node:child_process";
import { FIXTURES } from "../src/lib/fixtures.js";
import { isUnfinished } from "../src/lib/lane-states.js";
import { envRank } from "../src/lib/colors.js";
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

let server = null;
if (!EXTERNAL && !(await reachable(URL, 1000))) {
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
    process.kill(-server.pid, "SIGTERM");
  } catch {
    server.kill("SIGTERM");
  }
};

const ENGINES = { chromium, firefox, webkit };
const engineName = process.env.GALLERY_BROWSER || "chromium";
const engine = ENGINES[engineName];
if (!engine) {
  console.error(`unknown GALLERY_BROWSER "${engineName}" — one of ${Object.keys(ENGINES).join(", ")}`);
  process.exit(1);
}
const browser = await engine.launch();
const page = await browser.newPage({ viewport: { width: 1180, height: 900 }, deviceScaleFactor: 2 });

const failures = [];
try {
  await page.goto(URL, { waitUntil: "load" });
  await page.waitForSelector("body[data-gallery-ready='true']", { timeout: 20000 });

  await page.addStyleTag({
    content: `*, *::before, *::after {
      animation-play-state: paused !important;
      animation-delay: -0.01s !important;
      transition: none !important;
    }`,
  });

  const observed = await page.evaluate(() => {
    const out = {};
    for (const s of document.querySelectorAll(".fixture")) {
      const el = s.querySelector("release-timeline");
      const sr = el?.shadowRoot ?? el;
      const cards = [...(sr?.querySelectorAll("[data-release]") ?? [])];
      out[s.dataset.fixture] = {
        laneStates: cards[0]?.dataset.laneStates ?? "",
        allLaneStates: cards.map((c) => c.dataset.laneStates ?? ""),
        dotTitles: [...(sr?.querySelectorAll(".lane-dot") ?? [])].map((d) => d.getAttribute("title")),
        travelling: (sr?.querySelectorAll(".lane-run[data-direction]") ?? []).length,
        seams: [...(sr?.querySelectorAll(".rt-strand") ?? [])].flatMap((strand) => {
          const runs = [...strand.querySelectorAll(".lane-run")];
          const hold = runs.find((r) => r.dataset.layer === "hold");
          if (!hold) return [];
          const holdTop = parseFloat(hold.style.top);
          return runs
            .filter((r) => r.dataset.layer === "approach")
            .filter((r) => parseFloat(r.style.top) + parseFloat(r.style.height) <= holdTop)
            .map((r) => r.dataset.run);
        }),
        chevronMotion: [...(sr?.querySelectorAll(".lane-run[data-direction]") ?? [])].map(
          (el) => getComputedStyle(el, "::after").animationName,
        ),
        faults: (sr?.querySelectorAll('.lane-run[data-run="fault"]') ?? []).length,
        pulsing: (sr?.querySelectorAll(".lane-pulse") ?? []).length,
        offCentreDots: [...(sr?.querySelectorAll(".rt-strand") ?? [])].flatMap((strand) => {
          const run = strand.querySelector(".lane-run");
          if (!run) return [];
          const rr = run.getBoundingClientRect();
          return [...strand.querySelectorAll(".lane-dot")]
            .map((d) => {
              const r = d.getBoundingClientRect();
              const left = r.left - rr.left;
              const right = rr.right - r.right;
              if (Math.abs(left - right) > 0.01) {
                return `${d.dataset.kind} off centre by ${(left - right).toFixed(2)}px`;
              }
              if (Math.abs(left - Math.round(left)) > 0.01) {
                return `${d.dataset.kind} inset ${left.toFixed(2)}px — not a whole pixel`;
              }
              return null;
            })
            .filter(Boolean);
        }),
        laneOrder: [...(sr?.querySelectorAll(".rt-lane") ?? [])].map((l) => l.dataset.env),
        avatars: cards.map((c) => {
          const a = c.querySelector("[data-avatar]");
          return a ? a.tagName.toLowerCase() : null;
        }),
      };
    }
    return out;
  });

  {
    const lane = page.locator('section[data-fixture="partial-rollout"] .rt-lane[data-env="prod"]');
    const before = await lane.locator(".rt-strand").count();
    const faultsClosed = await lane.locator('.lane-run[data-run="fault"]').count();

    await lane.locator(".rt-lane-hit").click();
    await page.waitForTimeout(400);

    const after = await lane.locator(".rt-strand").count();
    const labels = await page
      .locator('section[data-fixture="partial-rollout"] .rt-lane-label-dest')
      .allTextContents();
    if (before !== 1) failures.push(`fan-out: prod should start as one strand, got ${before}`);
    if (after !== 3) failures.push(`fan-out: prod should open into 3 strands, got ${after}`);
    const faultsOpen = await lane.locator('.lane-run[data-run="fault"]').count();
    if (faultsClosed !== 0) {
      failures.push(`fan-out: collapsed prod is live overall and must not draw a failure, got ${faultsClosed}`);
    }
    if (faultsOpen !== 1) {
      failures.push(`fan-out: one prod placement failed and its strand must say so, got ${faultsOpen}`);
    }
    if (labels.length !== 3) failures.push(`fan-out: expected 3 destination labels, got ${labels.length}`);
    if (labels.some((l) => l.startsWith("prod"))) {
      failures.push(`fan-out: destination labels still carry the environment prefix — ${labels.join(", ")}`);
    }
    const fannedDots = await lane.evaluate((el) =>
      [...el.querySelectorAll(".rt-strand")].flatMap((strand) => {
        const run = strand.querySelector(".lane-run");
        if (!run) return [];
        const rr = run.getBoundingClientRect();
        return [...strand.querySelectorAll(".lane-dot")]
          .map((d) => {
            const r = d.getBoundingClientRect();
            const left = r.left - rr.left;
            const right = rr.right - r.right;
            if (Math.abs(left - right) > 0.01) return `${d.dataset.kind} off centre`;
            if (Math.abs(left - Math.round(left)) > 0.01) {
              return `${d.dataset.kind} inset ${left.toFixed(2)}px — not a whole pixel`;
            }
            return null;
          })
          .filter(Boolean);
      }),
    );
    if (fannedDots.length > 0) {
      failures.push(`fan-out: ${fannedDots.join(", ")}`);
    }

    await page.locator('section[data-fixture="partial-rollout"]').scrollIntoViewIfNeeded();
    const hovers = await lane.evaluate(async (el) => {
      const hit = el.querySelector(".rt-lane-hit");
      const card = () => el.closest(".fixture").querySelector(".rt-hovercard");
      const rect = hit.getBoundingClientRect();
      const strands = [...el.querySelectorAll(".rt-strand")];
      const out = [];
      for (const strand of strands) {
        const x = rect.left + 3 + parseFloat(strand.style.left) + parseFloat(strand.style.width) / 2;
        for (const dot of strand.querySelectorAll(".lane-dot")) {
          const y =
            rect.top + parseFloat(dot.style.top) + parseFloat(dot.style.height) / 2;
          hit.dispatchEvent(new MouseEvent("mousemove", { clientX: x, clientY: y, bubbles: true }));
          await new Promise((r) => setTimeout(r, 30));
          const c = card();
          out.push({
            strand: strand.querySelector(".lane-dot") ? strand.style.left : null,
            dot: dot.dataset.kind,
            title: c?.querySelector(".rt-hovercard-title")?.textContent.trim().split(/\s+/)[0],
            status: c?.querySelector("dd")?.textContent.trim(),
            commit: c?.querySelectorAll("dd")[1]?.textContent.trim(),
          });
        }
      }
      hit.dispatchEvent(new MouseEvent("mouseleave", { bubbles: true }));
      return out;
    });

    const genericTitles = hovers.filter((h) => !h.title || h.title === "prod");
    if (genericTitles.length > 0) {
      failures.push(
        `fan-out: ${genericTitles.length} hover(s) reported the environment instead of the placement`,
      );
    }
    const perStrand = new Map();
    for (const h of hovers) {
      if (!perStrand.has(h.strand)) perStrand.set(h.strand, new Set());
      perStrand.get(h.strand).add(h.commit);
    }
    for (const [strand, commits] of perStrand) {
      if (commits.size < 2) {
        failures.push(`fan-out: strand at ${strand} reported one release for every bubble`);
      }
    }
    if (!hovers.some((h) => h.status === "Failed")) {
      failures.push(`fan-out: no bubble reported the failed placement — got ${hovers.map((h) => h.status).join(", ")}`);
    }

    await page.locator('section[data-fixture="partial-rollout"]').screenshot({
      path: join(SHOTS, "partial-rollout-fanned.png"),
    });
    await lane.locator(".rt-lane-hit").click();
    await page.waitForTimeout(400);
  }

  {
    const section = page.locator('section[data-fixture="partial-rollout"]');
    await section.locator("details.rt-details").first().evaluate((d) => { d.open = true; });
    await page.waitForTimeout(200);

    const row = section.locator(".rt-destination", { hasText: "us-east-1" }).first();
    const why = row.locator("a.rt-why");

    if ((await why.count()) !== 1) {
      failures.push(`failed placement: expected one "Why it failed" link, got ${await why.count()}`);
    } else {
      const href = await why.getAttribute("href");
      if (!/\/releases\/partial-rollout$/.test(href || "")) {
        failures.push(`failed placement: link should reach the release page, got "${href}"`);
      }
    }

    const text = await row.innerText();
    if (text.includes("exact decimal string") || text.includes("provider reported failure")) {
      failures.push(`failed placement: the provider's message is pasted into the lane — "${text}"`);
    }

    const height = await row.evaluate((el) => el.getBoundingClientRect().height);
    if (height > 30) {
      failures.push(`failed placement: row grew to ${height.toFixed(1)}px — something wrapped inside it`);
    }

    await page.mouse.move(0, 0);
    await page.waitForTimeout(200);
    await section.screenshot({ path: join(SHOTS, "partial-rollout-failed-placement.png") });
    await section.locator("details.rt-details").first().evaluate((d) => { d.open = false; });
    await page.waitForTimeout(200);
  }

  for (const f of FIXTURES) {
    const o = observed[f.key];
    if (!o) { failures.push(`${f.key}: not rendered`); continue; }

    for (const [env, kind] of Object.entries(f.expect)) {
      if (!o.laneStates.split(",").includes(`${env}:${kind}`)) {
        failures.push(`${f.key}: expected ${env}:${kind} in data-lane-states, got "${o.laneStates}"`);
      }
    }

    for (const [i, exp] of (f.expectBelow || []).entries()) {
      const got = o.allLaneStates[i + 1] ?? "";
      for (const [env, kind] of Object.entries(exp)) {
        if (!got.split(",").includes(`${env}:${kind}`)) {
          failures.push(`${f.key}: card ${i + 1} expected ${env}:${kind}, got "${got}"`);
        }
      }
    }

    const wantsMotion = [f.expect, ...(f.expectBelow || [])]
      .flatMap((e) => Object.values(e))
      .some(isUnfinished);
    if (wantsMotion && (o.travelling === 0 || o.pulsing === 0)) {
      failures.push(
        `${f.key}: unfinished but nothing is drawn moving (travelling=${o.travelling} pulsing=${o.pulsing})`,
      );
    }
    if (!wantsMotion && (o.travelling > 0 || o.pulsing > 0)) {
      failures.push(
        `${f.key}: finished but something is drawn moving (travelling=${o.travelling} pulsing=${o.pulsing})`,
      );
    }

    if (o.offCentreDots.length > 0) {
      failures.push(`${f.key}: dot(s) off centre — ${o.offCentreDots.join(", ")}`);
    }

    if (o.seams.length > 0) {
      failures.push(`${f.key}: ${o.seams.join(", ")} run(s) stop short of the hold — the join will show`);
    }

    const still = o.chevronMotion.filter((n) => !n || n === "none").length;
    if (still > 0) {
      failures.push(`${f.key}: ${still} travel segment(s) draw no motion at all`);
    }

    const ranked = o.laneOrder.filter(Boolean);
    const expectedOrder = [...ranked].sort(
      (a, b) => envRank(a) - envRank(b),
    );
    if (ranked.join(",") !== expectedOrder.join(",")) {
      failures.push(`${f.key}: lanes out of order — got ${ranked.join(",")}, want ${expectedOrder.join(",")}`);
    }

    if (o.avatars.includes(null)) {
      failures.push(`${f.key}: a card has no [data-avatar] lane anchor — got ${JSON.stringify(o.avatars)}`);
    }
    if (!o.avatars.includes("img") || !o.avatars.includes("span")) {
      failures.push(`${f.key}: expected both a picture and a fallback avatar, got ${JSON.stringify(o.avatars)}`);
    }

    if (Object.values(f.expect).includes("awaiting")) {
      if (!o.dotTitles.some((t) => (t || "").startsWith("Awaiting approval"))) {
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
console.log(`\nall ${FIXTURES.length} states render distinguishably in ${engineName}; screenshots in gallery/shots/`);
process.exit(0);
