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
  delete globalThis.hljs;
});

function fakeTimers() {
  let next = 1;
  const live = new Map();
  return {
    live,
    setInterval: (fn) => {
      live.set(next, fn);
      return next++;
    },
    clearInterval: (id) => live.delete(id),
    tick: () => Promise.all([...live.values()].map((fn) => fn())),
  };
}

const LIST = (open) => `
<details data-slug="a"${open ? " open" : ""}><summary>a</summary></details>
<details data-slug="b"><summary>b</summary></details>`;

describe("notifications.js under sedx", () => {
  it("polls only while the list is on the page", async () => {
    const timers = fakeTimers();
    const fetch = vi.fn(async () => ({ ok: true, text: async () => LIST(false) }));
    const p = load(`<main><div id="notifications-list">${LIST(false)}</div></main>`, { ...timers, fetch });
    p.run("notifications.js");
    p.swap();
    expect(timers.live.size).toBe(1);

    p.document.querySelector("main").innerHTML = "<p>dashboard</p>";
    await timers.tick();
    expect(timers.live.size).toBe(0);
    expect(fetch).not.toHaveBeenCalled();
    p.swap();
    expect(timers.live.size).toBe(0);

    p.document.querySelector("main").innerHTML = `<div id="notifications-list">${LIST(false)}</div>`;
    p.swap();
    p.swap();
    expect(timers.live.size).toBe(1);
    await timers.tick();
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("keeps a notification the user opened open across refreshes of a swapped-in list", async () => {
    const timers = fakeTimers();
    const fetch = vi.fn(async () => ({ ok: true, text: async () => LIST(false) }));
    const p = load("<main></main>", { ...timers, fetch });
    p.run("notifications.js");
    p.document.querySelector("main").innerHTML = `<div id="notifications-list">${LIST(false)}</div>`;
    p.swap();
    const details = p.document.querySelector('details[data-slug="a"]');
    details.open = true;
    details.dispatchEvent(new p.win.Event("toggle"));
    await timers.tick();
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(p.document.querySelector('details[data-slug="a"]')).not.toBe(details);
    expect(p.document.querySelector('details[data-slug="a"]').open).toBe(true);
    expect(p.document.querySelector('details[data-slug="b"]').open).toBe(false);
  });
});

const README = '<div class="prose"><pre><code class="language-cue">a: 1</code></pre></div>';

function fakeHljs() {
  return {
    highlightElement: vi.fn((el) => {
      el.dataset.highlighted = "yes";
    }),
  };
}

describe("readme-highlight.js under sedx", () => {
  it("highlights each code block once, including ones a swap brings", () => {
    globalThis.hljs = fakeHljs();
    const p = load(`<main>${README}</main>`);
    p.run("readme-highlight.js");
    p.swap();
    expect(globalThis.hljs.highlightElement).toHaveBeenCalledTimes(1);

    p.document.querySelector("main").innerHTML = README + README;
    p.swap();
    expect(globalThis.hljs.highlightElement).toHaveBeenCalledTimes(3);
  });

  it("waits for highlight.js when the swap ran this script first", () => {
    const p = load(`<main>${README}</main><script src="/static/js/vendor/highlight.min.js"></script>`);
    p.run("readme-highlight.js");
    globalThis.hljs = fakeHljs();
    p.document.querySelector("script").dispatchEvent(new p.win.Event("load"));
    expect(globalThis.hljs.highlightElement).toHaveBeenCalledTimes(1);
  });
});

describe("ui.js under sedx", () => {
  it("closes the nav's menus after a swap and leaves other details alone", () => {
    const p = load(`
      <nav><details open><summary>kasper</summary><a href="/settings/account">Account</a></details></nav>
      <main><details id="row" open><summary>pipeline</summary></details><time datetime="2026-01-02T03:04:05Z">x</time></main>`);
    p.run("ui.js");
    p.swap();
    expect(p.document.querySelector("nav details").open).toBe(false);
    expect(p.document.getElementById("row").open).toBe(true);
    expect(p.document.querySelector("time").title).not.toBe("");
  });
});
