import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import {
  releaseEnvStates, laneStatesAttr, timelineEnvStates, timelineLaneStatesAttrs,
  isUnfinished, effectiveStatus, isPlanAwaiting, isGateAwaiting, DOT_PRIORITY,
  timelineRollbacks,
} from "./lane-states.js";
import { FIXTURES, byKey } from "./fixtures.js";

const stateOf = (release, env) =>
  releaseEnvStates(release).find((e) => e.env === env)?.kind;

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
    expect(() => laneStatesAttr({})).not.toThrow();
    expect(laneStatesAttr({})).toBe("");
  });

  it("ignores stages with no environment", () => {
    const r = { pipeline_stages: [{ stage_type: "plan", status: "RUNNING", approval_status: "AWAITINGAPPROVAL" }] };
    expect(laneStatesAttr(r)).toBe("");
  });
});

describe("fixtures with `below` resolve across the timeline", () => {
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

  it("leaves an older in-flight leg alone — that is a rollback", () => {
    expect(timelineOf([live, byKey("deploying").release], "prod")).toEqual(["live", "flight"]);
  });

  it("leaves an already-terminal older leg alone", () => {
    expect(timelineOf([live, failed], "prod")).toEqual(["live", "past"]);
  });

  it("reproduces the screenshot's whole prod column as healthy", () => {
    const column = timelineOf(
      [live, byKey("superseded").release, failed, byKey("awaiting-plan").release, failed],
      "prod",
    );
    expect(column).toEqual(["live", "past", "past", "past", "past"]);
    expect(column.filter(isUnfinished)).toEqual([]);
  });

  it("does not leak across environments", () => {
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
    expect(timelineOf([byKey("awaiting-plan").release, byKey("complete").release], "prod"))
      .toEqual(["awaiting", "live"]);
  });

  it("only `live` supersedes — a newer failure does not settle the lane", () => {
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
    expect(timelineLaneStatesAttrs([])).toEqual([]);
    expect(timelineLaneStatesAttrs(undefined)).toEqual([]);
    expect(timelineLaneStatesAttrs([null, {}])).toEqual(["", ""]);
  });
});

describe("every lane state has a rendering", () => {
  const componentSrc = () =>
    readFileSync(new URL("../ReleaseTimeline.svelte", import.meta.url), "utf8");

  it("each DOT_PRIORITY kind is drawn distinctly in ReleaseTimeline.svelte", () => {
    const src = componentSrc();
    for (const kind of Object.keys(DOT_PRIORITY)) {
      expect(src, `no .lane-dot rule for data-kind="${kind}"`)
        .toContain(`.lane-dot[data-kind="${kind}"]`);
    }
  });

  it("each DOT_PRIORITY kind has a label", () => {
    const src = componentSrc();
    const words = src.slice(src.indexOf("const KIND_WORDS"), src.indexOf("};", src.indexOf("const KIND_WORDS")));
    for (const kind of Object.keys(DOT_PRIORITY)) {
      expect(words, `KIND_WORDS has no entry for "${kind}"`).toContain(`${kind}:`);
    }
  });

  it("the rail's unfinished check is driven by isUnfinished, not a local list", () => {
    const src = componentSrc();
    expect(src).toContain("isUnfinished(row.kind)");
  });

  it("the gutter reads lane states resolved across the timeline, not per release", () => {
    const src = componentSrc();
    expect(src).toContain("timelineLaneStatesAttrs");
    expect(src).toContain("data-lane-states={laneStatesBySlug.get(release.slug)");
  });
});

describe("gate stages", () => {
  const gate = (over = {}) => ({
    stage_type: "gate",
    status: "RUNNING",
    gate_waiting_on: ["rollout to be HEALTHY (nothing reported)"],
    ...over,
  });

  it("a running gate with unmet requirements is awaiting a signal", () => {
    expect(isGateAwaiting(gate())).toBe(true);
    expect(effectiveStatus(gate())).toBe("AWAITING_SIGNAL");
  });

  it("a running gate with nothing outstanding is not awaiting", () => {
    expect(isGateAwaiting(gate({ gate_waiting_on: [] }))).toBe(false);
    expect(effectiveStatus(gate({ gate_waiting_on: [] }))).toBe("RUNNING");
  });

  it("a finished gate is not awaiting, whatever it once waited on", () => {
    for (const status of ["SUCCEEDED", "FAILED", "CANCELLED"]) {
      expect(isGateAwaiting(gate({ status }))).toBe(false);
      expect(effectiveStatus(gate({ status }))).toBe(status);
    }
  });

  it("survives gate_waiting_on being absent", () => {
    const { gate_waiting_on, ...noField } = gate();
    expect(isGateAwaiting(noField)).toBe(false);
    expect(effectiveStatus(noField)).toBe("RUNNING");
  });

  it("does not mistake other stage types for gates", () => {
    expect(isGateAwaiting({ ...gate(), stage_type: "wait" })).toBe(false);
    expect(isGateAwaiting({ ...gate(), stage_type: "deploy" })).toBe(false);
  });
});

describe("timelineRollbacks", () => {
  const liveOnProd = byKey("complete").release;
  const deployingProd = byKey("deploying").release;

  it("names an older in-flight leg under a live one as a rollback", () => {
    const [newest, older] = timelineRollbacks([liveOnProd, deployingProd]);
    expect([...newest]).toEqual([]);
    expect([...older]).toEqual(["prod"]);
  });

  it("does not call a forward deploy a rollback", () => {
    const [only] = timelineRollbacks([deployingProd]);
    expect([...only]).toEqual([]);
  });

  it("is per environment", () => {
    const devOnly = {
      destinations: [{ environment: "dev", status: "SUCCEEDED", is_current: true }],
      pipeline_stages: [],
    };
    const [, older] = timelineRollbacks([devOnly, deployingProd]);
    expect([...older]).toEqual([]);
  });

  it("survives holes and missing input rather than throwing", () => {
    expect(timelineRollbacks(undefined)).toEqual([]);
    expect(timelineRollbacks([null, {}]).map((s) => [...s])).toEqual([[], []]);
  });
});
