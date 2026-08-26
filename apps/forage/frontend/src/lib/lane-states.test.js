import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import {
  releaseEnvStates, laneStatesAttr, isUnfinished,
  effectiveStatus, isPlanAwaiting, DOT_PRIORITY,
} from "./lane-states.js";
import { FIXTURES, byKey } from "./fixtures.js";

const stateOf = (release, env) =>
  releaseEnvStates(release).find((e) => e.env === env)?.kind;

describe("fixtures resolve to their documented lane states", () => {
  for (const f of FIXTURES) {
    it(`${f.key}: ${f.title}`, () => {
      for (const [env, kind] of Object.entries(f.expect)) {
        expect(stateOf(f.release, env), `${f.key} / ${env}`).toBe(kind);
      }
    });
  }
});

describe("a plan awaiting approval is not finished", () => {
  // The regression this module exists for: prod's plan is parked, prod's
  // destination has never been touched, and the lane used to draw the release
  // as settled — indistinguishable from one that had completed.
  it("marks the gated environment `awaiting`, not `pending`", () => {
    expect(stateOf(byKey("awaiting-plan").release, "prod")).toBe("awaiting");
  });

  it("counts as unfinished, so the rail draws its in-flight segment", () => {
    expect(isUnfinished("awaiting")).toBe(true);
  });

  it("does not disturb environments that already finished", () => {
    expect(stateOf(byKey("awaiting-plan").release, "dev")).toBe("live");
  });

  it("outranks a PENDING destination row for the same environment", () => {
    expect(stateOf(byKey("awaiting-over-pending").release, "prod")).toBe("awaiting");
    expect(DOT_PRIORITY.awaiting).toBeGreaterThan(DOT_PRIORITY.pending);
  });

  it("outranks flight, because only one of them needs a person", () => {
    expect(DOT_PRIORITY.awaiting).toBeGreaterThan(DOT_PRIORITY.flight);
  });

  it("accepts both API spellings of the approval status", () => {
    expect(stateOf(byKey("awaiting-plan").release, "prod")).toBe("awaiting");
    expect(stateOf(byKey("awaiting-underscore").release, "prod")).toBe("awaiting");
  });
});

describe("states that must NOT read as unfinished", () => {
  it.each([
    ["complete",   "prod", "live"],
    ["failed",     "prod", "past"],
    ["superseded", "prod", "past"],
    ["rejected",   "prod", "past"],
  ])("%s / %s resolves to %s", (key, env, kind) => {
    expect(stateOf(byKey(key).release, env)).toBe(kind);
    expect(isUnfinished(kind)).toBe(false);
  });

  it("a rejected plan does not linger as awaiting", () => {
    // CANCELLED with no approval_status: the plan is over, not parked.
    expect(stateOf(byKey("rejected").release, "prod")).not.toBe("awaiting");
  });
});

describe("states that must read as unfinished", () => {
  it.each([
    ["deploying",     "prod", "flight"],
    ["awaiting-plan", "prod", "awaiting"],
  ])("%s / %s resolves to %s", (key, env, kind) => {
    expect(stateOf(byKey(key).release, env)).toBe(kind);
    expect(isUnfinished(kind)).toBe(true);
  });

  it("`pending` is not unfinished-for-rail purposes", () => {
    // Queued behind an upstream stage draws no in-flight segment: nothing is
    // happening on that lane yet.
    expect(isUnfinished("pending")).toBe(false);
  });
});

describe("effectiveStatus / isPlanAwaiting", () => {
  it("maps RUNNING + AWAITINGAPPROVAL to AWAITING_APPROVAL", () => {
    expect(effectiveStatus({ stage_type: "plan", status: "RUNNING", approval_status: "AWAITINGAPPROVAL" }))
      .toBe("AWAITING_APPROVAL");
  });

  it("leaves a deploy stage alone even if it carries an approval_status", () => {
    expect(effectiveStatus({ stage_type: "deploy", status: "RUNNING", approval_status: "AWAITINGAPPROVAL" }))
      .toBe("RUNNING");
    expect(isPlanAwaiting({ stage_type: "deploy", status: "RUNNING", approval_status: "AWAITINGAPPROVAL" }))
      .toBe(false);
  });

  it("an approved plan is not awaiting", () => {
    expect(isPlanAwaiting({ stage_type: "plan", status: "SUCCEEDED" })).toBe(false);
  });
});

describe("laneStatesAttr encoding", () => {
  it("emits env:kind pairs the DOM attribute parser can read back", () => {
    expect(laneStatesAttr(byKey("awaiting-plan").release)).toBe("dev:live,prod:awaiting");
  });

  it("is empty for a release with no environments", () => {
    expect(laneStatesAttr({ destinations: [], pipeline_stages: [] })).toBe("");
  });

  it("survives missing fields rather than throwing", () => {
    // The timeline renders optimistically from SSE deltas, so partial release
    // objects reach this function in practice.
    expect(() => laneStatesAttr({})).not.toThrow();
    expect(laneStatesAttr({})).toBe("");
  });

  it("ignores stages with no environment", () => {
    const r = { pipeline_stages: [{ stage_type: "plan", status: "RUNNING", approval_status: "AWAITINGAPPROVAL" }] };
    expect(laneStatesAttr(r)).toBe("");
  });
});

describe("every lane state has a rendering", () => {
  // Guards the seam the original bug fell through: the resolver gained a state
  // the template did not handle, so it fell to the `{:else}` branch and rendered
  // as a settled historical dot. Adding a kind to DOT_PRIORITY without a
  // matching branch now fails here rather than in someone's browser.
  const componentSrc = () =>
    readFileSync(new URL("../ReleaseTimeline.svelte", import.meta.url), "utf8");

  it("each DOT_PRIORITY kind is handled in ReleaseTimeline.svelte", () => {
    const src = componentSrc();
    // `past` is the intentional {:else} fallback and has no explicit branch.
    const explicit = Object.keys(DOT_PRIORITY).filter((k) => k !== "past" && k !== "stopped");
    for (const kind of explicit) {
      expect(src, `no template branch for dot.kind === "${kind}"`)
        .toContain(`dot.kind === "${kind}"`);
    }
  });

  it("the rail's unfinished check is driven by isUnfinished, not a local list", () => {
    const src = componentSrc();
    expect(src).toContain("isUnfinished(laneState.status)");
  });
});
