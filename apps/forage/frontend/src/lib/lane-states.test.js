import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import {
  releaseEnvStates, laneStatesAttr, timelineEnvStates, timelineLaneStatesAttrs,
  isUnfinished, effectiveStatus, isPlanAwaiting, DOT_PRIORITY,
} from "./lane-states.js";
import { FIXTURES, byKey } from "./fixtures.js";

const stateOf = (release, env) =>
  releaseEnvStates(release).find((e) => e.env === env)?.kind;

// Newest first, the order the timeline renders in.
const timelineOf = (releases, env) =>
  timelineEnvStates(releases).map((states) => states.find((e) => e.env === env)?.kind);

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

describe("fixtures with `below` resolve across the timeline", () => {
  // Same fixtures the gallery renders, so the words and the pixels cannot
  // drift: `expect` is the top card, `expectBelow` the older cards under it.
  for (const f of FIXTURES.filter((x) => x.below)) {
    it(`${f.key}: ${f.title}`, () => {
      const resolved = timelineEnvStates([f.release, ...f.below]);
      const wanted = [f.expect, ...f.expectBelow];
      expect(resolved).toHaveLength(wanted.length);
      wanted.forEach((exp, i) => {
        for (const [env, kind] of Object.entries(exp)) {
          expect(resolved[i].find((e) => e.env === env)?.kind, `${f.key} / card ${i} / ${env}`)
            .toBe(kind);
        }
      });
    });
  }
});

describe("a newer release supersedes an older leg on the same environment", () => {
  // The screenshot: prod's newest release is complete, and two older ones below
  // it never finished prod — one parked on approval, one failed. The gutter kept
  // drawing the amber "went backwards" hatch down to them, so a healthy prod
  // read as broken.
  const live = byKey("complete").release;
  const awaiting = byKey("awaiting-plan").release;
  const failed = byKey("failed").release;
  const stale = byKey("queued").release;

  it("demotes an older `awaiting` leg to `past`", () => {
    expect(timelineOf([live, awaiting], "prod")).toEqual(["live", "past"]);
  });

  it("demotes an older `pending` leg to `past`", () => {
    expect(timelineOf([live, stale], "prod")).toEqual(["live", "past"]);
  });

  it("demotes an older in-flight leg to `past`", () => {
    expect(timelineOf([live, byKey("deploying").release], "prod")).toEqual(["live", "past"]);
  });

  it("leaves an already-terminal older leg alone", () => {
    expect(timelineOf([live, failed], "prod")).toEqual(["live", "past"]);
  });

  it("reproduces the screenshot's whole prod column as healthy", () => {
    // newest → oldest: complete, superseded-success, failed, complete, failed
    const column = timelineOf(
      [live, byKey("superseded").release, failed, byKey("awaiting-plan").release, failed],
      "prod",
    );
    expect(column).toEqual(["live", "past", "past", "past", "past"]);
    expect(column.filter(isUnfinished)).toEqual([]);
  });

  it("does not leak across environments", () => {
    // `awaiting-plan` is live on dev and parked on prod. A newer release that is
    // live on prod only must not settle dev.
    const prodOnly = {
      destinations: [{ environment: "prod", status: "SUCCEEDED", is_current: true }],
      pipeline_stages: [],
    };
    const dev = {
      destinations: [],
      pipeline_stages: [
        { stage_type: "plan", environment: "dev", status: "RUNNING", approval_status: "AWAITINGAPPROVAL" },
      ],
    };
    expect(timelineOf([prodOnly, dev], "dev")).toEqual([undefined, "awaiting"]);
  });
});

describe("supersession must not over-suppress", () => {
  const older = byKey("superseded").release;

  it("keeps the newest release's own `awaiting` — nothing has superseded it", () => {
    expect(timelineOf([byKey("awaiting-plan").release, older], "prod")).toEqual(["awaiting", "past"]);
  });

  it("keeps the newest release's own in-flight leg", () => {
    expect(timelineOf([byKey("deploying").release, older], "prod")).toEqual(["flight", "past"]);
  });

  it("an older release that is still what prod runs stays `live`", () => {
    // Not a contradiction: the newest release is parked on prod approval, so
    // prod is genuinely still on the release below it. Demoting that to `past`
    // would leave the lane claiming prod runs nothing at all.
    expect(timelineOf([byKey("awaiting-plan").release, byKey("complete").release], "prod"))
      .toEqual(["awaiting", "live"]);
  });

  it("only `live` supersedes — a newer failure does not settle the lane", () => {
    // prod is genuinely broken: the newest release failed, and the one below it
    // is still parked. Neither leg may be swallowed.
    const column = timelineOf([byKey("failed").release, byKey("awaiting-plan").release], "prod");
    expect(column).toEqual(["past", "awaiting"]);
    expect(column.some(isUnfinished)).toBe(true);
  });

  it("a newer in-flight release does not supersede the leg below it", () => {
    expect(timelineOf([byKey("deploying").release, byKey("awaiting-plan").release], "prod"))
      .toEqual(["flight", "awaiting"]);
  });

  it("an environment with no live release anywhere keeps every leg", () => {
    expect(timelineOf([byKey("awaiting-underscore").release, byKey("queued").release], "prod"))
      .toEqual(["awaiting", "pending"]);
  });
});

describe("supersession leaves per-release history intact", () => {
  // Scope guard: the *card* must keep reporting its own outcome. Only the
  // aggregate gutter goes quiet.
  it("releaseEnvStates is unchanged by what sits above a release", () => {
    const awaiting = byKey("awaiting-plan").release;
    expect(stateOf(awaiting, "prod")).toBe("awaiting");
    timelineEnvStates([byKey("complete").release, awaiting]);
    expect(stateOf(awaiting, "prod")).toBe("awaiting");
  });

  it("does not mutate the release objects it is given", () => {
    const awaiting = byKey("awaiting-plan").release;
    const before = JSON.stringify(awaiting);
    timelineEnvStates([byKey("complete").release, awaiting]);
    expect(JSON.stringify(awaiting)).toBe(before);
  });
});

describe("timelineLaneStatesAttrs", () => {
  it("encodes one attribute per release, in order, superseded", () => {
    expect(timelineLaneStatesAttrs([byKey("complete").release, byKey("awaiting-plan").release]))
      .toEqual(["dev:live,prod:live", "dev:past,prod:past"]);
  });

  it("agrees with laneStatesAttr when nothing supersedes anything", () => {
    const r = byKey("awaiting-plan").release;
    expect(timelineLaneStatesAttrs([r])).toEqual([laneStatesAttr(r)]);
  });

  it("survives holes and missing input rather than throwing", () => {
    // The timeline renders optimistically from SSE deltas.
    expect(timelineLaneStatesAttrs([])).toEqual([]);
    expect(timelineLaneStatesAttrs(undefined)).toEqual([]);
    expect(timelineLaneStatesAttrs([null, {}])).toEqual(["", ""]);
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

  it("the gutter reads lane states resolved across the timeline, not per release", () => {
    // `laneStatesAttr(release)` alone cannot see that prod has moved on, and
    // wiring the attribute back to it would silently restore the stale warning.
    const src = componentSrc();
    expect(src).toContain("timelineLaneStatesAttrs");
    expect(src).toContain("data-lane-states={laneStatesBySlug.get(release.slug)");
  });
});
