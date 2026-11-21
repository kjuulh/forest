import { describe, expect, it } from "vitest";
import { latestReleases, releaseFlows } from "./release-flow.js";

const stage = (id, stage_type, environment, depends_on = [], status = "PENDING") =>
  ({ id, stage_type, environment, depends_on, status });

const release = (overrides = {}) => ({
  slug: "v2",
  artifact_id: "artifact-2",
  project_name: "api",
  version: "v2.0.0",
  created_at: "2026-11-21T12:00:00Z",
  pipeline_stages: [],
  destinations: [],
  ...overrides,
});

const item = (value) => ({ kind: "release", release: value });
const flowOf = (value) => releaseFlows([item(value)], { org: "acme" })[0];
const dependencyPairs = (flow) => flow.edges.filter(edge => edge.kind === "dependency")
  .map(edge => [edge.source, edge.target]);

function expectReadableLayout(flow) {
  for (const node of flow.nodes) {
    expect(Number.isFinite(node.x) && Number.isFinite(node.y)).toBe(true);
    expect(node.x + flow.nodeWidth).toBeLessThanOrEqual(flow.width);
    expect(node.y + node.height).toBeLessThanOrEqual(flow.height);
    for (const other of flow.nodes) {
      if (other === node) continue;
      const separate = node.x + flow.nodeWidth <= other.x || other.x + flow.nodeWidth <= node.x ||
        node.y + node.height <= other.y || other.y + other.height <= node.y;
      expect(separate).toBe(true);
    }
  }
}

describe("latest releases shared by overview workspaces and flows", () => {
  it("selects hidden releases per project without conflating identical release slugs", () => {
    const older = release({ slug: "same", created_at: "2026-11-20T12:00:00Z", graph_html: "<section>older</section>" });
    const latest = release({ slug: "same", graph_html: "<section>latest</section>" });
    const worker = release({ slug: "same", project_name: "worker" });
    const selected = latestReleases([
      item(older),
      item(worker),
      { kind: "hidden", releases: [latest] },
    ]);

    expect(selected).toEqual([latest, worker]);
    expect(selected[0]).toBe(latest);
    expect(selected[1]).toBe(worker);
  });

  it("does not substitute an older workspace when the latest release has no server fragment", () => {
    const latest = release({ graph_html: undefined });
    const selected = latestReleases([
      item(release({ created_at: "2026-11-20T12:00:00Z", graph_html: "<section>older</section>" })),
      item(latest),
    ]);

    expect(selected).toEqual([latest]);
    expect(selected[0].graph_html).toBeUndefined();
  });

  it("uses the current project for unnamed releases and ignores malformed timeline entries", () => {
    const first = release({ project_name: "", created_at: null });
    expect(latestReleases([
      null,
      { kind: "hidden" },
      item(first),
      item(release({ project_name: "", created_at: null })),
    ], { project: "api" })).toEqual([first]);
    expect(latestReleases(null)).toEqual([]);
  });
});

describe("latest release flow per project", () => {
  it("draws explicit branch and join dependencies without inventing a sequential pipeline", () => {
    const flow = flowOf(release({ pipeline_stages: [
      stage("deploy-prod", "deploy", "prod", ["checks", "soak"]),
      stage("plan-dev", "plan", "dev"),
      stage("checks", "gate", "dev", ["plan-dev"]),
      stage("soak", "wait", "dev", ["plan-dev"]),
    ] }));

    expect(dependencyPairs(flow)).toEqual([
      ["stage:checks", "stage:deploy-prod"],
      ["stage:soak", "stage:deploy-prod"],
      ["stage:plan-dev", "stage:checks"],
      ["stage:plan-dev", "stage:soak"],
    ]);
    const nodes = Object.fromEntries(flow.nodes.map(node => [node.id, node]));
    expect(nodes.checks.x).toBe(nodes.soak.x);
    expect(nodes.checks.y).not.toBe(nodes.soak.y);
    expect(nodes["deploy-prod"].x).toBeGreaterThan(nodes.soak.x);
    expectReadableLayout(flow);
  });

  it("fans deploy stages out only to this release's destinations in the same environment", () => {
    const flow = flowOf(release({
      pipeline_stages: [
        stage("plan-prod", "plan", "prod", [], "SUCCEEDED"),
        stage("deploy-prod", "deploy", "prod", ["plan-prod"], "RUNNING"),
        stage("deploy-dev", "deploy", "dev", [], "SUCCEEDED"),
      ],
      destinations: [
        { name: "prod-eu", environment: "prod", stage_id: "deploy-prod", status: "SUCCEEDED" },
        { name: "prod-us", environment: "prod", stage_id: "deploy-prod", status: "QUEUED" },
        { name: "dev-eu", environment: "dev", stage_id: "deploy-dev", status: "FAILED" },
      ],
    }));
    const byKey = new Map(flow.nodes.map(node => [node.key, node]));
    const fanout = flow.edges.filter(edge => edge.kind === "destination");
    expect(fanout.map(edge => [byKey.get(edge.source).id, byKey.get(edge.target).label])).toEqual([
      ["deploy-prod", "prod-eu"], ["deploy-prod", "prod-us"], ["deploy-dev", "dev-eu"],
    ]);
    expect(flow.nodes.find(node => node.label === "prod-eu").stateLabel).toBe("Deployed");
    expect(flow.nodes.find(node => node.label === "prod-us").stateLabel).toBe("Queued");
    expect(flow.nodes.find(node => node.label === "dev-eu").stateLabel).toBe("Failed");
    expectReadableLayout(flow);
  });

  it("keeps projects distinct even when slugs match and includes the latest release inside hidden groups", () => {
    const flows = releaseFlows([
      item(release({ slug: "same", project_name: "api", created_at: "2026-11-20T12:00:00Z", destinations: [
        { name: "prod", environment: "prod", status: "SUCCEEDED" },
      ] })),
      item(release({ slug: "same", project_name: "worker", version: "worker-v8" })),
      { kind: "hidden", releases: [release({ slug: "new", version: "api-v3", destinations: [
        { name: "prod", environment: "prod", status: "PENDING" },
      ] })] },
    ], { org: "acme" });

    expect(flows.map(flow => [flow.project, flow.release])).toEqual([
      ["api", "api-v3"], ["worker", "worker-v8"],
    ]);
    expect(flows[0].href).toBe("/orgs/acme/projects/api/releases/new#release-graph");
    expect(flows[1].href).toBe("/orgs/acme/projects/worker/releases/same#release-graph");
    expect(flows[0].nodes[0].stateLabel).toBe("Pending");
  });

  it("leaves independent stages disconnected rather than treating their array order as dependencies", () => {
    const flow = flowOf(release({ pipeline_stages: [
      stage("deploy-dev", "deploy", "dev"), stage("plan-prod", "plan", "prod"),
      stage("audit", "gate", null),
    ] }));
    expect(flow.edges).toEqual([]);
    expect(new Set(flow.nodes.map(node => node.x)).size).toBe(1);
    expectReadableLayout(flow);
  });

  it("does not let succeeded plans or deployments mask failed and pending stages", () => {
    const flow = flowOf(release({ summary_status: "SUCCEEDED", pipeline_stages: [
      stage("plan-dev", "plan", "dev", [], "SUCCEEDED"),
      stage("deploy-dev", "deploy", "dev", ["plan-dev"], "SUCCEEDED"),
      stage("soak", "wait", "dev", ["deploy-dev"], "FAILED"),
      stage("plan-prod", "plan", "prod", ["soak"], "PENDING"),
      stage("deploy-prod", "deploy", "prod", ["plan-prod"], "PENDING"),
    ], destinations: [{ name: "prod-eu", environment: "prod", status: null }] }));

    expect(flow.nodes.filter(node => node.kind === "stage").map(node => node.stateLabel)).toEqual([
      "Succeeded", "Succeeded", "Failed", "Pending", "Pending",
    ]);
    expect(flow.nodes.find(node => node.kind === "destination").stateLabel).toBe("Pending");
    expect(flow.nodes.find(node => node.id === "soak").tone).toBe("failed");
  });

  it("attaches plan executions to their actual stage and never calls an untyped success deployed", () => {
    const flow = flowOf(release({
      pipeline_stages: [
        stage("plan-prod", "plan", "prod", [], "SUCCEEDED"),
        stage("deploy-prod", "deploy", "prod", ["plan-prod"], "PENDING"),
      ],
      destinations: [
        { name: "planned-eu", environment: "prod", stage_id: "plan-prod", status: "SUCCEEDED" },
        { name: "untyped-us", environment: "prod", status: "SUCCEEDED" },
      ],
    }));
    const planned = flow.nodes.find(node => node.label === "planned-eu");
    const untyped = flow.nodes.find(node => node.label === "untyped-us");
    expect(planned.stateLabel).toBe("Plan complete");
    expect(untyped.stateLabel).toBe("Run succeeded");
    expect(flow.edges.filter(edge => edge.kind === "destination").map(edge => [edge.source, edge.target]))
      .toEqual([["stage:plan-prod", planned.key]]);
    expect(flow.nodes.find(node => node.id === "deploy-prod").stateLabel).toBe("Pending");
  });

  it("shows a completed plan awaiting approval as waiting without hiding a failed plan", () => {
    const flow = flowOf(release({ pipeline_stages: [
      { ...stage("plan-prod", "plan", "prod", [], "SUCCEEDED"), approval_status: "AWAITING_APPROVAL" },
      { ...stage("failed-plan", "plan", "dev", [], "FAILED"), approval_status: "AWAITING_APPROVAL" },
    ] }));
    expect(flow.nodes.map(node => node.stateLabel)).toEqual(["Awaiting approval", "Failed"]);
    expect(flow.nodes[0].tone).toBe("waiting");
    expect(flow.destinationCount).toBe(0);
  });

  it("handles empty and partial records without fabricating dependencies or deployments", () => {
    expect(releaseFlows()).toEqual([]);
    expect(releaseFlows(null)).toEqual([]);
    expect(releaseFlows([null, { kind: "hidden" }])).toEqual([]);
    const empty = flowOf(release({ pipeline_stages: null, destinations: null }));
    expect(empty.nodes).toEqual([]);
    expect(empty.edges).toEqual([]);
    const partial = flowOf(release({ pipeline_stages: [
      { id: "partial", stage_type: "plan" },
      stage("deploy", "deploy", "prod", ["not-recorded"]),
    ], destinations: [{ name: "orphan", environment: "other" }] }));
    expect(partial.edges).toEqual([]);
    expect(partial.incompleteDependencies).toBe(true);
    expect(partial.nodes.map(node => node.stateLabel)).toEqual(["Pending", "Pending", "Pending"]);
    expectReadableLayout(partial);
  });

  it("keeps first timeline order when dates are absent and encodes native release links", () => {
    const flows = releaseFlows([
      item(release({ project_name: "", created_at: null, slug: "v/2" })),
      item(release({ project_name: "", created_at: null, slug: "older" })),
    ], { org: "my org", project: "my/api" });
    expect(flows).toHaveLength(1);
    expect(flows[0].href).toBe("/orgs/my%20org/projects/my%2Fapi/releases/v%2F2#release-graph");
  });
});

it("keeps plan-lock and stale-approval holds distinct from running and failure", () => {
  const flow = flowOf(release({ pipeline_stages: [
    { ...stage("held", "plan", "prod", [], "RUNNING"), error_message: "waiting for release older to apply its plan" },
    { ...stage("stale", "plan", "prod", [], "RUNNING"), approval_status: "AWAITING_APPROVAL", error_message: "stale: a newer plan replaced this plan" },
    { ...stage("failed", "plan", "prod", [], "FAILED"), error_message: "provider refused" },
  ] }));
  expect(flow.nodes.map(node => node.stateLabel)).toEqual(["Waiting", "Blocked", "Failed"]);
  expect(flow.nodes[0].reason).toContain("release older");
});
