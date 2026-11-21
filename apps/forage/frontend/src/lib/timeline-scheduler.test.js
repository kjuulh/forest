import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createTimelineScheduler } from "./timeline-scheduler.js";

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(0);
});
afterEach(() => vi.useRealTimers());

const flush = () => vi.advanceTimersByTimeAsync(0);

describe("canonical timeline refresh scheduling", () => {
  it("loads immediately and coalesces an event burst into one refresh per ten seconds", async () => {
    const run = vi.fn(async () => {});
    const scheduler = createTimelineScheduler(run);
    scheduler.request();
    await flush();
    expect(run).toHaveBeenCalledTimes(1);
    for (let i = 0; i < 100; i += 1) scheduler.request();
    await vi.advanceTimersByTimeAsync(9999);
    expect(run).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(run).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(30000);
    expect(run).toHaveBeenCalledTimes(2);
    scheduler.destroy();
  });

  it("never overlaps a slow request and preserves one dirty refresh", async () => {
    let finish;
    const run = vi.fn().mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }))
      .mockResolvedValue(undefined);
    const scheduler = createTimelineScheduler(run);
    scheduler.request();
    await flush();
    scheduler.request();
    scheduler.request();
    await vi.advanceTimersByTimeAsync(30000);
    expect(run).toHaveBeenCalledTimes(1);
    finish();
    await flush();
    expect(run).toHaveBeenCalledTimes(2);
    scheduler.destroy();
  });

  it("defers initial loading while hidden and resumes only one dirty request", async () => {
    const run = vi.fn(async () => {});
    const scheduler = createTimelineScheduler(run, { visible: false });
    scheduler.request();
    scheduler.request();
    await vi.advanceTimersByTimeAsync(60000);
    expect(run).not.toHaveBeenCalled();
    scheduler.setVisible(true);
    await flush();
    expect(run).toHaveBeenCalledTimes(1);
    scheduler.request();
    scheduler.setVisible(false);
    await vi.advanceTimersByTimeAsync(60000);
    expect(run).toHaveBeenCalledTimes(1);
    scheduler.setVisible(true);
    await flush();
    expect(run).toHaveBeenCalledTimes(2);
    scheduler.destroy();
  });

  it("aborts work on hide without overlapping it when the page resumes", async () => {
    let finish;
    const run = vi.fn().mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }))
      .mockResolvedValue(undefined);
    const scheduler = createTimelineScheduler(run);
    scheduler.request();
    await flush();
    scheduler.setVisible(false);
    expect(run.mock.calls[0][0].aborted).toBe(true);
    scheduler.setVisible(true);
    await vi.advanceTimersByTimeAsync(10000);
    expect(run).toHaveBeenCalledTimes(1);
    finish();
    await flush();
    expect(run).toHaveBeenCalledTimes(2);
    scheduler.destroy();
  });

  it("reports a failure once and allows a later event to refresh without a retry loop", async () => {
    const failure = new Error("unavailable");
    const run = vi.fn().mockRejectedValueOnce(failure).mockResolvedValue(undefined);
    const onError = vi.fn();
    const scheduler = createTimelineScheduler(run, { onError });
    scheduler.request();
    await flush();
    expect(onError).toHaveBeenCalledWith(failure);
    await vi.advanceTimersByTimeAsync(20000);
    expect(run).toHaveBeenCalledTimes(1);
    scheduler.request();
    await flush();
    expect(run).toHaveBeenCalledTimes(2);
    scheduler.destroy();
  });

  it("aborts navigation work and never starts queued work after destruction", async () => {
    const run = vi.fn(async () => {});
    const scheduler = createTimelineScheduler(run);
    scheduler.request();
    await flush();
    scheduler.request();
    scheduler.destroy();
    expect(run.mock.calls[0][0].aborted).toBe(false);
    scheduler.request();
    scheduler.setVisible(true);
    await vi.advanceTimersByTimeAsync(20000);
    expect(run).toHaveBeenCalledTimes(1);

    const pending = vi.fn(() => new Promise(() => {}));
    const navigating = createTimelineScheduler(pending);
    navigating.request();
    await flush();
    navigating.destroy();
    expect(pending.mock.calls[0][0].aborted).toBe(true);
  });
});
