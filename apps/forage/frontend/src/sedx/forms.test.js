import { afterEach, describe, expect, it, vi } from "vitest";
import { page } from "./page.js";

let open = [];
function load(html, overrides) {
  const p = page(html, overrides);
  open.push(p);
  return p;
}
afterEach(() => {
  open.forEach((p) => p.close());
  open = [];
});

const POLICY_FORM = `
<main>
  <select id="policy-type">
    <option value="soak_time" data-description="Wait before promoting" selected>Soak</option>
    <option value="approval" data-description="Ask a person">Approval</option>
  </select>
  <p id="policy-type-desc"></p>
  <div id="soak-time-fields"><input name="duration"></div>
  <div id="approval-fields" class="hidden"><input name="target"></div>
</main>`;

function choose(p, select, value) {
  select.value = value;
  select.dispatchEvent(new p.win.Event("change"));
}

describe("policy-form.js under sedx", () => {
  it("binds once on the first load, which also fires sedx:load", () => {
    const p = load(POLICY_FORM);
    p.run("policy-form.js");
    p.swap();
    const approval = p.document.getElementById("approval-fields");
    const toggle = vi.spyOn(approval.classList, "toggle");
    choose(p, p.document.getElementById("policy-type"), "approval");
    expect(toggle).toHaveBeenCalledTimes(1);
    expect(approval.classList.contains("hidden")).toBe(false);
    expect(p.document.querySelector("#soak-time-fields input").disabled).toBe(true);
    expect(p.document.getElementById("policy-type-desc").textContent).toBe("Ask a person");
  });

  it("works on a form a swap brought in", () => {
    const p = load("<main></main>");
    p.run("policy-form.js");
    p.document.querySelector("main").outerHTML = POLICY_FORM;
    p.swap();
    choose(p, p.document.getElementById("policy-type"), "approval");
    expect(p.document.getElementById("approval-fields").classList.contains("hidden")).toBe(false);
  });

  it("rebinds a select the morph kept without stacking listeners", () => {
    const p = load(POLICY_FORM);
    p.run("policy-form.js");
    const select = p.document.getElementById("policy-type");
    select.removeAttribute("data-policy-form-ready");
    p.swap();
    const approval = p.document.getElementById("approval-fields");
    const toggle = vi.spyOn(approval.classList, "toggle");
    choose(p, select, "approval");
    expect(toggle).toHaveBeenCalledTimes(1);
  });
});

const TRIGGER_FORM = `
<main>
  <form id="trigger-form">
    <div id="pattern-rows"></div>
    <button type="button" id="add-pattern-btn">Add</button>
    <input type="hidden" id="trigger-name-hidden">
    <span id="trigger-name-preview"></span>
    <input type="checkbox" name="target_environments" value="prod">
  </form>
</main>`;

describe("trigger-form.js under sedx", () => {
  it("adds its first pattern row once, and the add button adds one row per click", () => {
    const p = load(TRIGGER_FORM);
    p.run("trigger-form.js");
    p.swap();
    const rows = p.document.getElementById("pattern-rows");
    expect(rows.querySelectorAll(".pattern-row")).toHaveLength(1);
    p.document.getElementById("add-pattern-btn").click();
    expect(rows.querySelectorAll(".pattern-row")).toHaveLength(2);
  });

  it("starts over when the morph resets the form", () => {
    const p = load(TRIGGER_FORM);
    p.run("trigger-form.js");
    const form = p.document.getElementById("trigger-form");
    const rows = p.document.getElementById("pattern-rows");
    p.document.getElementById("add-pattern-btn").click();
    form.removeAttribute("data-trigger-form-ready");
    rows.replaceChildren();
    p.swap();
    expect(rows.querySelectorAll(".pattern-row")).toHaveLength(1);
    p.document.getElementById("add-pattern-btn").click();
    expect(rows.querySelectorAll(".pattern-row")).toHaveLength(2);
  });

  it("leaves a page without the form alone and drops the old listeners", () => {
    const p = load(TRIGGER_FORM);
    p.run("trigger-form.js");
    const button = p.document.getElementById("add-pattern-btn");
    const rows = p.document.getElementById("pattern-rows");
    p.document.getElementById("trigger-form").removeAttribute("id");
    p.swap();
    button.click();
    expect(rows.querySelectorAll(".pattern-row")).toHaveLength(1);
  });
});

function orgRules(projects, rules) {
  return `
<main>
  <div data-rule-preview="web"></div>
  <form id="org-rule-form">
    <input name="include_projects"><input name="exclude_projects"><input name="name_regex">
    <textarea name="metadata_match"></textarea><input name="tags">
    <input type="hidden" name="policies_json"><input type="hidden" name="triggers_json"><input type="hidden" name="pipelines_json">
    <div id="new-rule-preview"></div>
    <div id="org-trigger-pattern-rows"></div>
    <button type="button" id="org-add-pattern-btn">Add</button>
  </form>
  <script type="application/json" id="org-rule-projects-data">${JSON.stringify(projects)}</script>
  <script type="application/json" id="org-rule-sets-data">${JSON.stringify(rules)}</script>
</main>`;
}

describe("org-rules.js under sedx", () => {
  const rules = [{ name: "web", selector: { name_regex: "^web" } }];

  it("re-reads the page's JSON data on every swap", () => {
    const p = load(orgRules([{ name: "web-a" }, { name: "api" }], rules));
    p.run("org-rules.js");
    p.swap();
    const preview = () => p.document.querySelector("[data-rule-preview]").textContent;
    expect(preview()).toContain("1 matching project");

    p.document.querySelector("main").outerHTML = orgRules(
      [{ name: "web-a" }, { name: "web-b" }, { name: "web-c" }],
      rules,
    );
    p.swap();
    expect(preview()).toContain("3 matching projects");
    expect(preview()).toContain("web-c");
  });

  it("adds one trigger pattern row per click after a swap that kept the button", () => {
    const p = load(orgRules([{ name: "web-a" }], rules));
    p.run("org-rules.js");
    const rows = p.document.getElementById("org-trigger-pattern-rows");
    expect(rows.children).toHaveLength(1);
    p.document.getElementById("org-rule-projects-data").removeAttribute("data-org-rules-ready");
    rows.replaceChildren();
    p.swap();
    expect(rows.children).toHaveLength(1);
    p.document.getElementById("org-add-pattern-btn").click();
    expect(rows.children).toHaveLength(2);
  });
});

function destinations(types) {
  return `
<main>
  <select id="destination_type_0" class="dest-type-select" data-index="0">
    <option value="">Pick</option>
    ${types.map((t) => `<option value="${t.organisation}/${t.name}@${t.version}">${t.name}</option>`).join("")}
  </select>
  <div id="type-fields-0" class="hidden"></div>
  <div id="custom-fields-0"></div>
  <div id="extra-fields-0"></div>
  <input id="type_organisation_0"><input id="type_name_0"><input id="type_version_0">
  <form id="metadata-form"><div id="metadata-rows"></div></form>
  <button type="button" id="add-row-btn">Add</button>
  <script type="application/json" id="destination-types">${JSON.stringify(types)}</script>
</main>`;
}

describe("destinations.js under sedx", () => {
  const k8s = { organisation: "acme", name: "k8s", version: "1", fields: [{ name: "namespace" }] };
  const vm = { organisation: "acme", name: "vm", version: "2", fields: [{ name: "host" }] };

  it("uses the destination types of the page it is on after a swap", () => {
    const p = load(destinations([k8s]));
    p.run("destinations.js");
    p.swap();
    p.document.querySelector("main").outerHTML = destinations([vm]);
    p.swap();
    choose(p, p.document.getElementById("destination_type_0"), "acme/vm@2");
    expect(p.document.getElementById("type_name_0").value).toBe("vm");
    expect(p.document.querySelector('#type-fields-0 input[name="metadata_keys"]').value).toBe("host");
  });

  it("adds one metadata row per click on a button the morph kept", () => {
    const p = load(destinations([k8s]));
    p.run("destinations.js");
    p.document.querySelector("main").removeAttribute("data-destinations-ready");
    p.swap();
    p.document.getElementById("add-row-btn").click();
    expect(p.document.querySelectorAll("#metadata-rows .metadata-row")).toHaveLength(1);
  });
});
