import { describe, it, expect } from "vitest";
import { pipelineSummary, planHold, planHoldChip, planStageLabel, queuedDestinationNote } from "./status.js";
import {
  byKey, PLAN_WAITING_REASON, PLAN_STALE_REASON, PLAN_SUPERSEDED_REASON,
} from "./fixtures.js";

const prodPlan = (key) => byKey(key).release.pipeline_stages.find((s) => s.id === "plan-prod");

describe("a plan held by the plan guard says why", () => {
  it("reads a RUNNING plan with a reason as waiting, and names the release it waits for", () => {
    const hold = planHold(prodPlan("plan-waiting"));
    expect(hold).toEqual({
      kind: "waiting", reason: PLAN_WAITING_REASON, release: "boyishly-powerful-lizardfish",
    });
    expect(planStageLabel("RUNNING", hold)).toBe("Waiting to plan");
    expect(planHoldChip(hold)).toBe("Waiting for boyishly-powerful-lizardfish");
  });

  it("summarises the release as waiting to plan, not deploying", () => {
    expect(pipelineSummary(byKey("plan-waiting").release.pipeline_stages).label).toBe("Waiting to plan");
  });

  it("leaves a plain running plan alone", () => {
    const stage = { stage_type: "plan", status: "RUNNING" };
    expect(planHold(stage)).toBeNull();
    expect(planStageLabel("RUNNING", planHold(stage))).toBe("Planning");
  });
});

describe("a stale plan cannot be approved", () => {
  it("marks an awaiting plan whose reason starts with `stale:`", () => {
    const hold = planHold(prodPlan("plan-stale"));
    expect(hold).toEqual({
      kind: "stale", reason: PLAN_STALE_REASON, release: "boyishly-powerful-lizardfish",
    });
    expect(planStageLabel("AWAITING_APPROVAL", hold)).toBe("Plan stale");
  });

  it("does not ask for an approval forest would refuse", () => {
    const summary = pipelineSummary(byKey("plan-stale").release.pipeline_stages);
    expect(summary.label).toBe("Plan stale");
  });

  it("still asks for approval while another plan is fresh", () => {
    const stages = [
      ...byKey("plan-stale").release.pipeline_stages,
      { id: "plan-dev", stage_type: "plan", environment: "dev", status: "RUNNING", approval_status: "AWAITINGAPPROVAL" },
    ];
    expect(pipelineSummary(stages).label).toBe("Awaiting plan approval");
  });

  it("ignores a reason on an awaiting plan that is not a stale marker", () => {
    expect(planHold({ stage_type: "plan", status: "AWAITING_APPROVAL", error_message: "boom" })).toBeNull();
  });
});

describe("a superseded plan shows what superseded it", () => {
  it("reads a PENDING plan with a `superseded` reason as superseded", () => {
    const hold = planHold(prodPlan("plan-superseded"));
    expect(hold).toEqual({
      kind: "superseded", reason: PLAN_SUPERSEDED_REASON, release: "boyishly-powerful-lizardfish",
    });
    expect(planStageLabel("PENDING", hold)).toBe("Plan superseded");
  });

  it("ignores stages that are not plans", () => {
    expect(planHold({ stage_type: "deploy", status: "RUNNING", error_message: PLAN_WAITING_REASON })).toBeNull();
  });
});

describe("a destination queued behind another release's plan", () => {
  it("gets a waiting note naming that release", () => {
    expect(queuedDestinationNote({ status: "QUEUED", error_message: PLAN_WAITING_REASON })).toEqual({
      reason: PLAN_WAITING_REASON, chip: "Waiting for boyishly-powerful-lizardfish",
    });
  });

  it("leaves a failed destination's error to the failure link", () => {
    expect(queuedDestinationNote({ status: "FAILED", error_message: "boom" })).toBeNull();
    expect(queuedDestinationNote({ status: "QUEUED" })).toBeNull();
  });
});
