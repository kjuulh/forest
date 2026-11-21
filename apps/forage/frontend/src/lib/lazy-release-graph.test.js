import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createLazyReleaseGraph, releaseGraphActive, releaseGraphUrl } from "./lazy-release-graph.js";

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(0);
});
afterEach(() => vi.useRealTimers());
const flush = () => vi.advanceTimersByTimeAsync(0);
const state = overrides => ({ expanded: true, active: true, visible: true, revision: 1, ...overrides });
const response = html => ({ ok: true, text: async () => html });
const release = overrides => ({
  slug: "v1", project_name: "api", has_pipeline: true, release_intent_id: "intent-1",
  summary_status: "PENDING", pipeline_stages: [{ stage_type: "deploy", status: "PENDING" }],
  ...overrides,
});

describe("expanded pipeline workspaces", () => {
  it("requests only an active expanded entry that is visible", async () => {
    const fetchGraph = vi.fn(async () => response("pipeline"));
    const loader = createLazyReleaseGraph("/graph", vi.fn(), { fetchGraph });
    loader.update(state({ expanded: false }));
    await flush();
    loader.update(state({ active: false }));
    await flush();
    loader.update(state({ visible: false }));
    await vi.advanceTimersByTimeAsync(30000);
    expect(fetchGraph).not.toHaveBeenCalled();
    loader.update(state());
    await flush();
    expect(fetchGraph).toHaveBeenCalledTimes(1);
    expect(fetchGraph.mock.calls[0][1]).toMatchObject({ credentials: "same-origin", headers: { Accept: "text/html" } });
    loader.destroy();
  });

  it("refreshes a mounted pipeline to its final state and retains it until collapse", async () => {
    const fetchGraph = vi.fn().mockResolvedValueOnce(response("running")).mockResolvedValueOnce(response("done"));
    const changed = vi.fn();
    const loader = createLazyReleaseGraph("/graph", changed, { fetchGraph });
    loader.update(state());
    await flush();
    expect(changed).toHaveBeenLastCalledWith({ html: "running", error: null });
    loader.update(state({ active: false, revision: 2 }));
    await vi.advanceTimersByTimeAsync(9999);
    expect(fetchGraph).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(fetchGraph).toHaveBeenCalledTimes(2);
    expect(changed).toHaveBeenLastCalledWith({ html: "done", error: null });
    loader.update(state({ active: false, revision: 3 }));
    await vi.advanceTimersByTimeAsync(30000);
    expect(fetchGraph).toHaveBeenCalledTimes(2);
    expect(changed).toHaveBeenLastCalledWith({ html: "done", error: null });
    loader.update(state({ expanded: false, active: false }));
    expect(changed).toHaveBeenLastCalledWith({ html: "", error: null });
    loader.update(state({ active: false }));
    await flush();
    expect(fetchGraph).toHaveBeenCalledTimes(2);
    loader.destroy();
  });

  it("aborts collapse and discards a late response rather than replacing a reopened workspace", async () => {
    let finish;
    const fetchGraph = vi.fn().mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }))
      .mockResolvedValue(response("new"));
    const changed = vi.fn();
    const loader = createLazyReleaseGraph("/graph", changed, { fetchGraph });
    loader.update(state());
    await flush();
    const signal = fetchGraph.mock.calls[0][1].signal;
    loader.update(state({ expanded: false }));
    expect(signal.aborted).toBe(true);
    loader.update(state());
    await flush();
    expect(changed).toHaveBeenLastCalledWith({ html: "new", error: null });
    finish(response("stale"));
    await flush();
    expect(changed).toHaveBeenLastCalledWith({ html: "new", error: null });
    loader.destroy();
  });

  it("keeps a response that finishes after the expanded release becomes inactive", async () => {
    let finish;
    const fetchGraph = vi.fn().mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }))
      .mockResolvedValue(response("final"));
    const changed = vi.fn();
    const loader = createLazyReleaseGraph("/graph", changed, { fetchGraph });
    loader.update(state());
    await flush();
    loader.update(state({ active: false, revision: 2 }));
    expect(fetchGraph.mock.calls[0][1].signal.aborted).toBe(false);
    finish(response("final"));
    await flush();
    expect(changed).toHaveBeenLastCalledWith({ html: "final", error: null });
    await vi.advanceTimersByTimeAsync(10000);
    expect(fetchGraph).toHaveBeenCalledTimes(2);
    expect(changed).toHaveBeenLastCalledWith({ html: "final", error: null });
    loader.update(state({ expanded: false, active: false }));
    expect(changed).toHaveBeenLastCalledWith({ html: "", error: null });
    loader.destroy();
  });

  it("coalesces snapshot changes, pauses offscreen, and aborts navigation", async () => {
    const fetchGraph = vi.fn(async () => response("running"));
    const loader = createLazyReleaseGraph("/graph", vi.fn(), { fetchGraph });
    loader.update(state());
    await flush();
    loader.update(state({ revision: 2 }));
    loader.update(state({ revision: 3, visible: false }));
    await vi.advanceTimersByTimeAsync(30000);
    expect(fetchGraph).toHaveBeenCalledTimes(1);
    loader.update(state({ revision: 4, visible: true }));
    await flush();
    expect(fetchGraph).toHaveBeenCalledTimes(2);
    loader.destroy();

    const pending = vi.fn(() => new Promise(() => {}));
    const navigating = createLazyReleaseGraph("/graph", vi.fn(), { fetchGraph: pending });
    navigating.update(state());
    await flush();
    navigating.destroy();
    expect(pending.mock.calls[0][1].signal.aborted).toBe(true);
  });
});

describe("release execution eligibility", () => {
  it("includes queued, running, approval and signal waits but excludes completed or stopped history", () => {
    expect(releaseGraphActive(release())).toBe(true);
    for (const status of ["RUNNING", "ASSIGNED", "QUEUED", "AWAITING_APPROVAL", "AWAITING_SIGNAL"]) {
      expect(releaseGraphActive(release({ pipeline_stages: [{ status }] }))).toBe(true);
    }
    expect(releaseGraphActive(release({ pipeline_stages: [{ stage_type: "plan", status: "SUCCEEDED", approval_status: "AWAITINGAPPROVAL" }] }))).toBe(true);
    expect(releaseGraphActive(release({ summary_status: "SUCCEEDED", pipeline_stages: [{ status: "SUCCEEDED" }] }))).toBe(false);
    expect(releaseGraphActive(release({ summary_status: "FAILED" }))).toBe(false);
    expect(releaseGraphActive(release({ pipeline_stages: [{ status: "FAILED" }, { status: "PENDING" }] }))).toBe(false);
    expect(releaseGraphActive(release({ release_intent_id: null }))).toBe(false);
    expect(releaseGraphActive(release({ has_pipeline: false }))).toBe(false);
  });

  it("targets each release's project and encodes path segments", () => {
    expect(releaseGraphUrl("org name", "fallback", release({ project_name: "worker", slug: "v/2" })))
      .toBe("/orgs/org%20name/projects/worker/releases/v%2F2/graph");
    expect(releaseGraphUrl("org", "fallback", release({ project_name: "" })))
      .toBe("/orgs/org/projects/fallback/releases/v1/graph");
  });
});
