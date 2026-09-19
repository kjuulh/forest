/**
 * The lane's shape, tested without a browser.
 *
 * This maths used to live inside the component, mixed in with
 * `getBoundingClientRect` and a retry loop, which meant the only way to find
 * out whether a rollback drew the right segment was to squint at a screenshot.
 * Rows go in newest-first with a y per row — exactly what `measureRows` hands
 * it — and segments come out.
 */
import { describe, it, expect } from "vitest";
import { laneGeometry } from "./lane-geometry.js";

const H = 300;
// The metrics a 14px lane with a 10px head dot hands it: the cap is the
// strand's radius, so the dot sits concentric with the rounded end.
const M = { cap: 7, dot: 10 };
// Newest first, the order the cards are painted in.
const rows = (...pairs) => pairs.map(([y, kind]) => ({ y, kind }));
const runOf = (g, kind) => g.runs.find((r) => r.kind === kind);

describe("the four layers", () => {
  // The whole model: a road, what is coming, what is here, what is leaving.
  // Every case below is the same two rules applied — a run never ends on a dot,
  // and a run never has to butt against another.

  it("a settled lane is one run: what it holds, down to the bottom", () => {
    const g = laneGeometry(rows([40, "live"], [140, "past"]), H, M);
    expect(g.runs).toHaveLength(1);
    expect(g.runs[0]).toMatchObject({ kind: "solid", layer: "hold", top: 40 - M.cap, height: H - (40 - M.cap) });
    expect(g.head).toBe(40);
  });

  it("a deploy on its way up is an approach run above the hold", () => {
    const g = laneGeometry(rows([40, "flight"], [140, "live"]), H, M);
    expect(runOf(g, "travel")).toMatchObject({
      layer: "approach", top: 40 - M.cap, direction: "up", motion: "march",
    });
    expect(runOf(g, "solid")).toMatchObject({ layer: "hold", top: 140 - M.cap });
  });

  // The bug you could see: a run stopped exactly on the dot marking its far
  // end, so the boundary cut the dot in half and the dot read as sitting
  // crooked on the lane. A run goes dot to dot and consumes both.
  it("reaches a cap past the dot at each end", () => {
    const g = laneGeometry(rows([40, "flight"], [140, "live"]), H, M);
    const travel = runOf(g, "travel");
    expect(travel.top).toBe(40 - M.cap);
    expect(travel.top + travel.height).toBe(140 + M.cap);
    expect(runOf(g, "solid").top).toBe(140 - M.cap);
  });

  // Which colour shows at each end is a question of layer, not of extent: the
  // approach is painted under the hold, so where the two meet the environment's
  // own colour wins and its dot sits on solid.
  it("paints an approach under the hold and a rollback over it", () => {
    const fwd = laneGeometry(rows([40, "flight"], [140, "live"]), H, M);
    expect(runOf(fwd, "travel").layer).toBe("approach");
    const back = laneGeometry(rows([40, "live"], [140, "flight"]), H, M);
    expect(runOf(back, "reverse").layer).toBe("override");
  });

  // The approach run continues past where the hold begins, so the hold's
  // rounded end lands on it rather than on the page. No square joins, no fudge.
  it("overlaps the approach and the hold so no end can show the page", () => {
    const g = laneGeometry(rows([40, "flight"], [140, "live"]), H, M);
    const travel = runOf(g, "travel");
    const solid = runOf(g, "solid");
    expect(travel.top + travel.height).toBeGreaterThan(solid.top);
  });

  // The cap is the strand's radius, so the dot it marks sits at the rounded
  // end's centre of curvature: concentric, with the same clearance above the
  // dot as beside it. A larger cap leaves a gap above that is not there at the
  // sides, which is exactly what it looked like.
  it("puts the marker dot at the centre of the rounded end", () => {
    const g = laneGeometry(rows([140, "live"]), H, M);
    expect(140 - g.runs[0].top).toBe(M.cap);
  });

  it("never lifts a run above the top of the timeline", () => {
    expect(laneGeometry(rows([2, "live"]), H, M).runs[0].top).toBe(0);
  });
});

describe("what is on its way in", () => {
  it("fills the whole lane when nothing is held yet", () => {
    const g = laneGeometry(rows([40, "flight"]), H, M);
    expect(runOf(g, "travel")).toMatchObject({ top: 40 - M.cap, height: H - (40 - M.cap) });
    expect(runOf(g, "solid")).toBeUndefined();
  });

  it("marks a parked pipeline as waiting, and still animates it", () => {
    const g = laneGeometry(rows([40, "awaiting"], [140, "live"]), H, M);
    expect(runOf(g, "wait")).toMatchObject({ direction: "up", motion: "breathe" });
    expect(runOf(g, "travel")).toBeUndefined();
    expect(g.runs.some((r) => r.motion !== null)).toBe(true);
  });

  it("draws nothing moving when nothing is unfinished", () => {
    const g = laneGeometry(rows([40, "live"], [140, "past"]), H, M);
    expect(g.runs.every((r) => r.motion === null)).toBe(true);
  });
});

describe("failures", () => {
  it("sits above the hold, in its own run", () => {
    const g = laneGeometry(rows([40, "stopped"], [140, "live"]), H, M);
    const fault = runOf(g, "fault");
    expect(fault).toMatchObject({ layer: "approach", top: 40 - M.cap });
    expect(fault.top + fault.height).toBe(140 + M.cap);
    expect(runOf(g, "solid")).toMatchObject({ top: 140 - M.cap });
  });

  // The bug: a placement whose only row was a failed deploy drew a full-height
  // solid bar — the healthiest-looking thing on the page.
  it("never anchors the hold", () => {
    const g = laneGeometry(rows([40, "stopped"]), H, M);
    expect(runOf(g, "solid")).toBeUndefined();
    expect(runOf(g, "fault")).toBeDefined();
  });

  it("is a mark at a point when the lane has never held anything", () => {
    expect(runOf(laneGeometry(rows([40, "stopped"]), H, M), "fault").height).toBeLessThan(20);
  });

  // A failure something newer has already replaced is history, and the card
  // keeps saying so. The lane must not still be shouting about it.
  it("is not drawn when a newer release is live above it", () => {
    expect(runOf(laneGeometry(rows([40, "live"], [140, "stopped"]), H, M), "fault"))
      .toBeUndefined();
  });
});

describe("rollbacks", () => {
  const g = () => laneGeometry(rows([40, "live"], [140, "flight"]), H, M);

  it("overlays the hold rather than replacing it", () => {
    expect(runOf(g(), "reverse")).toMatchObject({ layer: "override", direction: "down" });
    // prod is still on the newer release until the rollback lands.
    expect(runOf(g(), "solid")).toMatchObject({ top: 40 - M.cap, height: H - (40 - M.cap) });
  });

  // Both ends: the release the environment is on, and the one it is going back
  // to. Painted over the hold, so the whole stretch reads as the rollback
  // rather than as the environment with a band laid across it.
  it("consumes the dot at each end", () => {
    const rev = runOf(g(), "reverse");
    expect(rev.top).toBe(40 - M.cap);
    expect(rev.top + rev.height).toBe(140 + M.cap);
  });

  // Uber's UI says `Rolling back in production ▼` for the same reason: the two
  // directions need opposite reactions and look identical otherwise.
  it("never points the same way as a deploy", () => {
    expect(runOf(g(), "reverse").direction).toBe("down");
    const fwd = laneGeometry(rows([40, "flight"], [140, "live"]), H, M);
    expect(runOf(fwd, "travel").direction).toBe("up");
  });

  it("colours the dot it is heading for, and leaves the head alone", () => {
    const dots = g().dots;
    expect(dots.find((d) => d.kind === "flight").tone).toBe("attention");
    expect(dots.find((d) => d.kind === "live").tone).toBeNull();
  });

  it("does not tone a dot on an ordinary forward deploy", () => {
    const fwd = laneGeometry(rows([40, "flight"], [140, "live"]), H, M);
    expect(fwd.dots.every((d) => d.tone === null)).toBe(true);
  });
});

describe("input the component actually hands it", () => {
  it("survives an empty lane", () => {
    expect(laneGeometry([], H, M)).toEqual({ runs: [], head: null, dots: [] });
  });

  it("survives missing rows and rows with no state", () => {
    expect(laneGeometry(undefined, H, M).dots).toEqual([]);
    expect(laneGeometry([null, { y: 10 }, { y: 20, kind: "live" }], H, M).dots).toHaveLength(1);
  });

  it("survives being given no metrics at all", () => {
    expect(() => laneGeometry(rows([40, "live"]), H)).not.toThrow();
  });
});
