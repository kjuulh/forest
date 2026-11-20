import { afterEach, describe, expect, it, vi } from "vitest";
import { page } from "./page.js";

let open = [];
const saved = {};
function load(html, overrides) {
  const p = page(html, overrides);
  open.push(p);
  return p;
}
afterEach(() => {
  open.forEach((p) => p.close());
  open = [];
  for (const [key, value] of Object.entries(saved)) {
    if (value === undefined) delete globalThis[key];
    else globalThis[key] = value;
  }
});

function links(n) {
  return Array.from({ length: n }, (_, i) => `<a href="/orgs/org-${i}">org-${i}</a>`).join("");
}

const SWITCHER = `<div data-fuzzy-list>${links(9)}</div>`;

async function fuzzyPage() {
  const p = load(`<nav><details data-fuzzy-switcher data-fuzzy-label="Search"><summary>org</summary><div class="menu">${SWITCHER}</div></details></nav>`);
  for (const key of ["document", "window", "requestAnimationFrame"]) {
    if (!(key in saved)) saved[key] = globalThis[key];
  }
  globalThis.document = p.document;
  globalThis.window = p.win;
  globalThis.requestAnimationFrame = (fn) => fn();
  vi.resetModules();
  await import("../../../static/js/fuzzy-switcher.js");
  return p;
}

function arrowDown(p, details) {
  details.dispatchEvent(new p.win.KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true }));
}

describe("fuzzy-switcher.js under sedx", () => {
  it("adds its search field once on the first load", async () => {
    const p = await fuzzyPage();
    p.swap();
    expect(p.document.querySelectorAll("details input")).toHaveLength(1);
  });

  it("re-enhances the switcher the morph reset, with one set of key handlers", async () => {
    const p = await fuzzyPage();
    const details = p.document.querySelector("details");
    details.querySelector(".menu").innerHTML = SWITCHER;
    p.swap();
    expect(details.querySelectorAll("input")).toHaveLength(1);
    arrowDown(p, details);
    const options = details.querySelectorAll("[data-fuzzy-list] > a");
    expect(options[1].hasAttribute("data-fuzzy-active")).toBe(true);
    expect(details.querySelectorAll("[data-fuzzy-active]")).toHaveLength(1);
  });
});

const STAGES_A = JSON.stringify([{ id: "staging", depends_on: [], config: { Deploy: { environment: "staging" } } }]);
const STAGES_B = JSON.stringify([
  { id: "staging", depends_on: [], config: { Deploy: { environment: "staging" } } },
  { id: "prod", depends_on: ["staging"], config: { Deploy: { environment: "prod" } } },
]);

describe("pipeline-builder.js under sedx", () => {
  it("rebuilds from its target when the morph emptied it, and only then", () => {
    const frames = [];
    const flush = () => frames.splice(0).forEach((fn) => fn());
    const p = load(`<form><pipeline-builder data-target="stages" data-readonly="true"></pipeline-builder><textarea id="stages" hidden>${STAGES_A}</textarea></form>`, {
      requestAnimationFrame: (fn) => frames.push(fn),
    });
    p.run("pipeline-builder.js");
    flush();
    const builder = p.document.querySelector("pipeline-builder");
    expect(builder.querySelectorAll(".pb-node")).toHaveLength(1);

    const connected = vi.spyOn(builder, "connectedCallback");
    p.swap();
    expect(connected).not.toHaveBeenCalled();

    builder.replaceChildren();
    p.document.getElementById("stages").value = STAGES_B;
    p.swap();
    flush();
    expect(connected).toHaveBeenCalledTimes(1);
    expect(builder.querySelectorAll(".pb-node")).toHaveLength(2);
  });
});
