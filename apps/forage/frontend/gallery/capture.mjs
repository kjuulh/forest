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

// Chromium by default, but switchable: a dot centred with a percentage and a
// transform sat half a pixel off the bar in Firefox and exactly on it in
// Chrome, so a suite that only ever ran one engine could not see it.
//   GALLERY_BROWSER=firefox npm run gallery:capture
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

  // Freeze every animation so screenshots are byte-stable between runs rather
  // than catching a keyframe at a random phase. The assertions below read
  // whether an element *has* an animation, not what frame it is on, so
  // stopping them costs nothing.
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
        // Every card, top to bottom: a superseded state only exists relative to
        // what is above it, so the cards below are the interesting ones.
        allLaneStates: cards.map((c) => c.dataset.laneStates ?? ""),
        dotTitles: [...(sr?.querySelectorAll(".lane-dot") ?? [])].map((d) => d.getAttribute("title")),
        // Lane runs, by what they mean rather than by how they are painted. An
        // earlier version sniffed for an SVG data URI in `background-image`,
        // which quietly stopped matching anything the day the chevrons moved to
        // a CSS mask — a green suite asserting a property no element had.
        travelling: (sr?.querySelectorAll(".lane-run[data-direction]") ?? []).length,
        // Runs never butt against each other: an approach run continues past
        // where the hold begins, so the hold's rounded end lands on it. Lose
        // that overlap and every lane grows a notch of page through its middle.
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
        // Whether each travel segment's chevrons are animated at all. Counting
        // elements is not enough: a lane parked on an approval kept its
        // segment and its pulsing dot — so the count check passed — while the
        // chevrons themselves sat perfectly still, which is the one thing a
        // state that needs a person must never do.
        chevronMotion: [...(sr?.querySelectorAll(".lane-run[data-direction]") ?? [])].map(
          (el) => getComputedStyle(el, "::after").animationName,
        ),
        faults: (sr?.querySelectorAll('.lane-run[data-run="fault"]') ?? []).length,
        pulsing: (sr?.querySelectorAll(".lane-pulse") ?? []).length,
        // Every dot centred on the bar it marks, and sitting on whole pixels.
        //
        // Both halves matter. Asymmetry is the obvious bug. The subtle one is a
        // dot whose size and strand disagree in parity — a 7px dot in a 12px
        // strand insets by 2.5px — which is symmetric on paper and lands half a
        // pixel off the bar once a browser rounds it. Chrome and Firefox round
        // it differently, so this only ever showed up in one of them.
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
        // Production first, then back down the pipeline — see `orderLanes`.
        laneOrder: [...(sr?.querySelectorAll(".rt-lane") ?? [])].map((l) => l.dataset.env),
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

  // A lane that fans out is the one piece of this design a unit test cannot
  // reach: the strands only exist once somebody clicks. `partial-rollout` is
  // the fixture with more than one placement in an environment.
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
    // The reason this layer exists. prod is live overall — two of three
    // placements took the release — so the collapsed lane shows no failure,
    // and the fanned one has to.
    const faultsOpen = await lane.locator('.lane-run[data-run="fault"]').count();
    if (faultsClosed !== 0) {
      failures.push(`fan-out: collapsed prod is live overall and must not draw a failure, got ${faultsClosed}`);
    }
    if (faultsOpen !== 1) {
      failures.push(`fan-out: one prod placement failed and its strand must say so, got ${faultsOpen}`);
    }
    if (labels.length !== 3) failures.push(`fan-out: expected 3 destination labels, got ${labels.length}`);
    // Every placement in prod is called prod-something; the lane already says
    // prod, so the labels must not repeat it.
    if (labels.some((l) => l.startsWith("prod"))) {
      failures.push(`fan-out: destination labels still carry the environment prefix — ${labels.join(", ")}`);
    }
    // A fanned strand is narrower than a lane, so its dots are sized
    // differently — and that is where a dot and its strand can disagree in
    // parity and land the dot on a half pixel. The snapshot above is taken with
    // every lane collapsed and cannot see it.
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

    // Each bubble answers for itself. The hit area covers the whole lane and
    // sits above the dots, so every hover used to resolve to whatever the lane
    // was doing — the newest release — whichever bubble you were pointing at.
    // Fanned out, the answer also has to depend on which strand.
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

    // Every strand names its own placement, not the environment.
    const genericTitles = hovers.filter((h) => !h.title || h.title === "prod");
    if (genericTitles.length > 0) {
      failures.push(
        `fan-out: ${genericTitles.length} hover(s) reported the environment instead of the placement`,
      );
    }
    // And the two bubbles on a strand are different releases — the bug was
    // that every bubble reported the same one.
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
    // us-east-1 failed on the newest release and still holds the one below it.
    if (!hovers.some((h) => h.status === "Failed")) {
      failures.push(`fan-out: no bubble reported the failed placement — got ${hovers.map((h) => h.status).join(", ")}`);
    }

    await page.locator('section[data-fixture="partial-rollout"]').screenshot({
      path: join(SHOTS, "partial-rollout-fanned.png"),
    });
    await lane.locator(".rt-lane-hit").click(); // gather it back up
    await page.waitForTimeout(400);
  }

  // A failed placement says that it failed and where to go, and nowhere on this
  // page does it paste the provider's sentence in.
  //
  // The row is a nowrap flex line of fixed-size parts, and a chained provider
  // error runs to a few hundred characters: rendered inline it wrapped inside
  // the row and turned one placement into a paragraph -- the same shape of bug
  // the release page had. Hence both halves of this check: the link is there,
  // and the row is still one line tall.
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

    // One line. `.rt-destination` is 11.5px at line-height 1.7 plus 1px of
    // padding either side, so anything past ~30px means something wrapped.
    const height = await row.evaluate((el) => el.getBoundingClientRect().height);
    if (height > 30) {
      failures.push(`failed placement: row grew to ${height.toFixed(1)}px — something wrapped inside it`);
    }

    // The hover checks above left a bubble open over the card.
    await page.mouse.move(0, 0);
    await page.waitForTimeout(200);
    await section.screenshot({ path: join(SHOTS, "partial-rollout-failed-placement.png") });
    await section.locator("details.rt-details").first().evaluate((d) => { d.open = false; });
    await page.waitForTimeout(200);
  }

  for (const f of FIXTURES) {
    const o = observed[f.key];
    if (!o) { failures.push(`${f.key}: not rendered`); continue; }

    // 1. The resolved state reached the DOM.
    for (const [env, kind] of Object.entries(f.expect)) {
      if (!o.laneStates.split(",").includes(`${env}:${kind}`)) {
        failures.push(`${f.key}: expected ${env}:${kind} in data-lane-states, got "${o.laneStates}"`);
      }
    }

    // 1b. Cards *below* the fixture carry their resolved state too — this is
    //     where supersession shows up: an older parked leg reading `past`
    //     because a newer release took the environment over.
    for (const [i, exp] of (f.expectBelow || []).entries()) {
      const got = o.allLaneStates[i + 1] ?? "";
      for (const [env, kind] of Object.entries(exp)) {
        if (!got.split(",").includes(`${env}:${kind}`)) {
          failures.push(`${f.key}: card ${i + 1} expected ${env}:${kind}, got "${got}"`);
        }
      }
    }

    // 2. Unfinished states animate; finished ones do not. This is the invariant
    //    the bug violated — a parked pipeline that rendered as settled. The
    //    second bug it catches is the mirror image: a lane that keeps animating
    //    for an older release a newer one has already superseded.
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

    // 2a1. Dots sit on the middle of their lane.
    if (o.offCentreDots.length > 0) {
      failures.push(`${f.key}: dot(s) off centre — ${o.offCentreDots.join(", ")}`);
    }

    // 2a2. No approach run stops short of the hold it runs into.
    if (o.seams.length > 0) {
      failures.push(`${f.key}: ${o.seams.join(", ")} run(s) stop short of the hold — the join will show`);
    }

    // 2a. Every travelling segment is visibly alive — marching when something
    //     is moving, breathing when it is parked on a person. Either way, not
    //     still.
    const still = o.chevronMotion.filter((n) => !n || n === "none").length;
    if (still > 0) {
      failures.push(`${f.key}: ${still} travel segment(s) draw no motion at all`);
    }

    // 2b. Lanes read production-first, whatever order the server sent.
    const ranked = o.laneOrder.filter(Boolean);
    const expectedOrder = [...ranked].sort(
      (a, b) => envRank(a) - envRank(b),
    );
    if (ranked.join(",") !== expectedOrder.join(",")) {
      failures.push(`${f.key}: lanes out of order — got ${ranked.join(",")}, want ${expectedOrder.join(",")}`);
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
// Explicit: a stray handle must not turn a passing run into a hung job.
process.exit(0);
