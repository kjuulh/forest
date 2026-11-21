import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { describe, expect, it, vi } from "vitest";

const source = readFileSync(new URL("../../../static/js/timeline-prefetch.js", import.meta.url), "utf8");

function page({ visible = true, graph = false } = {}) {
  const listeners = new Set();
  const document = {
    visibilityState: visible ? "visible" : "hidden",
    currentScript: { getAttribute: name => name === "data-timeline-url"
      ? "/api/orgs/test/timeline" : graph ? "true" : "false" },
    addEventListener: (name, listener) => { if (name === "visibilitychange") listeners.add(listener); },
    removeEventListener: (name, listener) => { if (name === "visibilitychange") listeners.delete(listener); },
  };
  const fetch = vi.fn(async () => ({ ok: true, json: async () => ({ timeline: [] }) }));
  const window = {};
  runInNewContext(source, { document, window, fetch });
  return { window, fetch, visibility(value) {
    document.visibilityState = value ? "visible" : "hidden";
    for (const listener of [...listeners]) listener();
  } };
}

describe("timeline prefetch visibility", () => {
  it("makes no hidden initial request and primes once on visibility", async () => {
    const hidden = page({ visible: false, graph: true });
    expect(hidden.fetch).not.toHaveBeenCalled();
    hidden.visibility(false);
    expect(hidden.fetch).not.toHaveBeenCalled();
    hidden.visibility(true);
    expect(hidden.fetch).toHaveBeenCalledTimes(1);
    expect(hidden.fetch.mock.calls[0][0]).toBe("/api/orgs/test/timeline?include_graph=true");
    await expect(hidden.window.__forestTimeline["/api/orgs/test/timeline?include_graph=true"])
      .resolves.toEqual({ timeline: [] });
    hidden.visibility(false);
    hidden.visibility(true);
    expect(hidden.fetch).toHaveBeenCalledTimes(1);
  });

  it("keeps default prefetch requests compatible without graph work", async () => {
    const visible = page();
    expect(visible.fetch).toHaveBeenCalledTimes(1);
    expect(visible.fetch.mock.calls[0][0]).toBe("/api/orgs/test/timeline");
    await expect(visible.window.__forestTimeline["/api/orgs/test/timeline"])
      .resolves.toEqual({ timeline: [] });
  });
});
