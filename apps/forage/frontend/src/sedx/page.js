import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { Window } from "happy-dom";

const STATIC_JS = fileURLToPath(new URL("../../../static/js/", import.meta.url));

const GLOBALS = [
  "window",
  "document",
  "location",
  "navigator",
  "performance",
  "localStorage",
  "setTimeout",
  "clearTimeout",
  "setInterval",
  "clearInterval",
  "requestAnimationFrame",
  "fetch",
  "alert",
  "customElements",
  "HTMLElement",
  "Element",
  "Node",
  "Event",
  "CustomEvent",
  "AbortController",
  "ResizeObserver",
  "MutationObserver",
  "getComputedStyle",
  "CSS",
];

export function page(html, overrides = {}) {
  const win = new Window({ url: "https://forest.test/" });
  win.document.body.innerHTML = html;
  const scope = { ...overrides };

  return {
    win,
    document: win.document,
    run(file) {
      const source = readFileSync(STATIC_JS + file, "utf8");
      const values = GLOBALS.map((name) => {
        if (name in scope) return scope[name];
        if (name === "window") return win;
        return win[name] ?? globalThis[name];
      });
      new Function(...GLOBALS, source)(...values);
    },
    swap() {
      win.document.dispatchEvent(new win.CustomEvent("sedx:load", { detail: { url: win.location.href } }));
    },
    close() {
      win.happyDOM.abort();
      win.close();
    },
  };
}
