// @vitest-environment happy-dom
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { byKey, PLAN_WAITING_REASON, PLAN_STALE_REASON, PLAN_SUPERSEDED_REASON } from "./lib/fixtures.js";

beforeAll(() => {
  globalThis.EventSource = class {
    addEventListener() {}
    close() {}
  };
  new Function(readFileSync(join(process.cwd(), "../static/js/components/forage-components.js"), "utf8"))();
});

afterEach(() => {
  document.body.innerHTML = "";
});

async function render(key) {
  const release = { ...byKey(key).release, title: key, created_at: new Date().toISOString() };
  globalThis.fetch = vi.fn(async () => new Response(
    JSON.stringify({ lanes: [{ name: "dev" }, { name: "prod" }], timeline: [{ kind: "release", release }] }),
    { status: 200, headers: { "Content-Type": "application/json" } },
  ));
  const el = document.createElement("release-timeline");
  el.setAttribute("org", "acme");
  el.setAttribute("project", key);
  el.setAttribute("csrf", "token");
  document.body.appendChild(el);
  await vi.waitFor(() => {
    if (!el.querySelector(".rt-stages")) throw new Error("timeline not rendered");
  });
  return el;
}

const planRow = (el) =>
  [...el.querySelectorAll(".rt-stage")].find((li) => li.querySelector(".rt-stage-label")?.textContent.match(/plan/i) && li.textContent.includes("prod"));
const buttons = (root) => [...root.querySelectorAll("button")].map((b) => b.textContent.trim());

describe("the release timeline says why a plan is held", () => {
  it("shows who a held plan waits for, with the full reason on hover", async () => {
    const el = await render("plan-waiting");
    const row = planRow(el);
    expect(row.querySelector(".rt-stage-label").textContent).toBe("Waiting to plan");
    const chip = row.querySelector('[data-plan-hold="waiting"]');
    expect(chip.textContent).toBe("Waiting for boyishly-powerful-lizardfish");
    expect(chip.getAttribute("title")).toBe(PLAN_WAITING_REASON);
    expect(el.querySelector(".rt-summary-label").textContent).toBe("Waiting to plan");
  });

  it("shows a destination queued behind another plan as waiting, not failed", async () => {
    const el = await render("plan-waiting");
    const dest = [...el.querySelectorAll(".rt-destination")].find((li) => li.textContent.includes("hetzner-prod"));
    const note = dest.querySelector("[data-dest-note]");
    expect(note.textContent).toBe("Waiting for boyishly-powerful-lizardfish");
    expect(note.getAttribute("title")).toBe(PLAN_WAITING_REASON);
    expect(dest.textContent).not.toContain("Why it failed");
  });

  it("offers only Reject on a stale plan, since forest refuses its approval", async () => {
    const el = await render("plan-stale");
    const row = planRow(el);
    expect(row.querySelector(".rt-stage-label").textContent).toBe("Plan stale");
    expect(row.querySelector('[data-plan-hold="stale"]').getAttribute("title")).toBe(PLAN_STALE_REASON);
    expect(buttons(row)).toContain("Reject");
    expect(buttons(el)).not.toContain("Approve plan");
  });

  it("keeps Approve on a plan that is not stale", async () => {
    const el = await render("awaiting-plan");
    expect(buttons(planRow(el))).toEqual(expect.arrayContaining(["Approve plan", "Reject"]));
  });

  it("names the release that superseded a plan", async () => {
    const el = await render("plan-superseded");
    const row = planRow(el);
    expect(row.querySelector(".rt-stage-label").textContent).toBe("Plan superseded");
    const chip = row.querySelector('[data-plan-hold="superseded"]');
    expect(chip.textContent).toBe("Superseded by boyishly-powerful-lizardfish");
    expect(chip.getAttribute("title")).toBe(PLAN_SUPERSEDED_REASON);
  });
});
