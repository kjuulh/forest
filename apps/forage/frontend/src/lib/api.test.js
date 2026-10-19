import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { fetchTimeline, timelineUrl } from "./api.js";

const PAYLOAD = { timeline: [{ kind: "release" }], lanes: [{ name: "prod" }] };
const FRESH = { timeline: [], lanes: [] };

let fetchMock;

beforeEach(() => {
  fetchMock = vi.fn(async () => ({ ok: true, status: 200, json: async () => FRESH }));
  vi.stubGlobal("fetch", fetchMock);
  delete globalThis.__forestTimeline;
});

afterEach(() => {
  vi.unstubAllGlobals();
  delete globalThis.__forestTimeline;
});

const prime = (url, value) => {
  globalThis.__forestTimeline = { [url]: Promise.resolve(value) };
};

describe("timelineUrl", () => {
  it("addresses a project timeline", () => {
    expect(timelineUrl("understory", "forage")).toBe(
      "/api/orgs/understory/projects/forage/timeline",
    );
  });

  it("addresses an org-wide timeline when there is no project", () => {
    expect(timelineUrl("understory", "")).toBe("/api/orgs/understory/timeline");
    expect(timelineUrl("understory", undefined)).toBe("/api/orgs/understory/timeline");
  });
});

describe("fetchTimeline", () => {
  it("fetches normally when the page primed nothing", async () => {
    await expect(fetchTimeline("understory", "forage")).resolves.toEqual(FRESH);
    expect(fetchMock).toHaveBeenCalledOnce();
    expect(fetchMock.mock.calls[0][0]).toBe("/api/orgs/understory/projects/forage/timeline");
  });

  it("takes the primed payload instead of making the request", async () => {
    prime(timelineUrl("understory", "forage"), PAYLOAD);
    await expect(fetchTimeline("understory", "forage")).resolves.toEqual(PAYLOAD);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("uses the primed payload once, then goes to the network", async () => {
    prime(timelineUrl("understory", "forage"), PAYLOAD);
    await expect(fetchTimeline("understory", "forage")).resolves.toEqual(PAYLOAD);
    await expect(fetchTimeline("understory", "forage")).resolves.toEqual(FRESH);
    expect(fetchMock).toHaveBeenCalledOnce();
  });

  it("falls back to the network when the prefetch failed", async () => {
    prime(timelineUrl("understory", "forage"), null);
    await expect(fetchTimeline("understory", "forage")).resolves.toEqual(FRESH);
    expect(fetchMock).toHaveBeenCalledOnce();
  });

  it("ignores a payload primed for a different timeline", async () => {
    prime(timelineUrl("understory", "other-project"), PAYLOAD);
    await expect(fetchTimeline("understory", "forage")).resolves.toEqual(FRESH);
    expect(fetchMock).toHaveBeenCalledOnce();
  });

  it("primes the org-wide timeline too", async () => {
    prime(timelineUrl("understory", ""), PAYLOAD);
    await expect(fetchTimeline("understory", "")).resolves.toEqual(PAYLOAD);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("still reports a failed request", async () => {
    fetchMock.mockResolvedValueOnce({ ok: false, status: 503, json: async () => ({}) });
    await expect(fetchTimeline("understory", "forage")).rejects.toThrow("503");
  });
});
