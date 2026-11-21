const NODE_WIDTH = 144;
const NODE_HEIGHT = 70;
const DESTINATION_HEIGHT = 46;
const COLUMN_GAP = 22;
const ROW_GAP = 22;
const INSET = 8;

const STATE_LABELS = {
  PENDING: "Pending",
  QUEUED: "Queued",
  ASSIGNED: "Running",
  RUNNING: "Running",
  SUCCEEDED: "Succeeded",
  FAILED: "Failed",
  TIMED_OUT: "Timed out",
  CANCELLED: "Cancelled",
  SKIPPED: "Skipped",
  SUPERSEDED: "Superseded",
  WAITING: "Waiting",
  AWAITING_APPROVAL: "Awaiting approval",
  BLOCKED: "Blocked",
};

function stateFor(row, execution = "stage") {
  let status = row.status || "PENDING";
  if (row.stage_type === "plan" &&
      ["PENDING", "RUNNING", "ASSIGNED", "SUCCEEDED", "AWAITING_APPROVAL"].includes(status) &&
      ["AWAITING_APPROVAL", "AWAITINGAPPROVAL"].includes(row.approval_status)) {
    status = "AWAITING_APPROVAL";
  }
  const reason = row.error_message || row.blocked_by || "";
  if (row.stage_type === "plan") {
    if (status === "AWAITING_APPROVAL" && reason.startsWith("stale:")) status = "BLOCKED";
    else if (["RUNNING", "ASSIGNED", "QUEUED"].includes(status) && reason.trim()) status = "WAITING";
  }
  const tone = ["FAILED", "TIMED_OUT"].includes(status) ? "failed"
    : status === "SUCCEEDED" ? "succeeded"
    : ["RUNNING", "ASSIGNED"].includes(status) ? "running"
    : ["AWAITING_APPROVAL", "BLOCKED", "WAITING"].includes(status) ? "waiting"
    : "pending";
  return {
    status,
    tone,
    reason,
    stateLabel: status === "SUCCEEDED" && execution !== "stage"
      ? execution === "deploy" ? "Deployed" : execution === "plan" ? "Plan complete" : "Run succeeded"
      : STATE_LABELS[status] || status,
  };
}

function releasesIn(timeline) {
  return timeline.flatMap(item => item?.kind === "release" && item.release ? [item.release]
    : item?.kind === "hidden" && Array.isArray(item.releases) ? item.releases : []);
}

function stageType(type) {
  return type ? type.charAt(0).toUpperCase() + type.slice(1) : "Stage";
}

function projectFlow(release, org, project) {
  const stages = Array.isArray(release.pipeline_stages) ? release.pipeline_stages : [];
  const destinations = Array.isArray(release.destinations) ? release.destinations : [];
  const nodes = [];
  const edges = [];
  const byStage = new Map();
  const projectName = release.project_name || project || "";
  const releaseLabel = release.version || release.slug || release.title || "Release";
  const href = org && projectName && release.slug
    ? `/orgs/${encodeURIComponent(org)}/projects/${encodeURIComponent(projectName)}/releases/${encodeURIComponent(release.slug)}#release-graph`
    : null;

  for (const stage of stages) {
    if (!stage?.id || byStage.has(stage.id)) continue;
    const node = {
      key: `stage:${stage.id}`,
      height: NODE_HEIGHT,
      id: stage.id,
      kind: "stage",
      label: stage.id,
      context: [stageType(stage.stage_type), stage.environment].filter(Boolean).join(" · "),
      environment: stage.environment || "",
      stageType: stage.stage_type,
      dependencies: Array.isArray(stage.depends_on) ? stage.depends_on : [],
      ...stateFor(stage),
    };
    nodes.push(node);
    byStage.set(stage.id, node);
  }

  for (const node of nodes) {
    for (const dependency of new Set(node.dependencies)) {
      if (dependency !== node.id && byStage.has(dependency)) {
        edges.push({ source: byStage.get(dependency).key, target: node.key, kind: "dependency" });
      }
    }
  }

  const visiting = new Set();
  const layers = new Map();
  function layerFor(node) {
    if (layers.has(node.key)) return layers.get(node.key);
    if (visiting.has(node.key)) return 0;
    visiting.add(node.key);
    let layer = 0;
    for (const id of node.dependencies) {
      const dependency = byStage.get(id);
      if (dependency && dependency !== node && !visiting.has(dependency.key)) {
        layer = Math.max(layer, layerFor(dependency) + 1);
      }
    }
    visiting.delete(node.key);
    layers.set(node.key, layer);
    return layer;
  }

  const destinationsByStage = new Map();
  const attached = new Set();
  for (const node of nodes) {
    node.layer = layerFor(node);
    if (!node.environment) continue;
    const targets = [];
    destinations.forEach((destination, index) => {
      if (!destination?.name || destination.stage_id !== node.id || destination.environment !== node.environment) return;
      const target = {
        key: `destination:${node.id}:${index}`,
        kind: "destination",
        height: DESTINATION_HEIGHT,
        label: destination.name,
        context: [stageType(node.stageType), destination.environment].filter(Boolean).join(" · "),
        environment: destination.environment || "",
        layer: node.layer,
        ...stateFor(destination, node.stageType || "unknown"),
      };
      targets.push(target);
      attached.add(index);
      edges.push({ source: node.key, target: target.key, kind: "destination" });
    });
    destinationsByStage.set(node.key, targets);
  }

  const columnBottoms = new Map();
  for (const node of [...nodes]) {
    const y = columnBottoms.get(node.layer) || INSET;
    node.x = INSET + node.layer * (NODE_WIDTH + COLUMN_GAP);
    node.y = y;
    let bottom = y + NODE_HEIGHT + ROW_GAP;
    for (const target of destinationsByStage.get(node.key) || []) {
      target.x = node.x + 18;
      target.y = bottom;
      nodes.push(target);
      bottom += DESTINATION_HEIGHT + 10;
    }
    columnBottoms.set(node.layer, bottom);
  }

  const orphanLayer = nodes.length ? Math.max(...nodes.map(node => node.layer)) + 1 : 0;
  destinations.forEach((destination, index) => {
    if (!destination?.name || attached.has(index)) return;
    const y = columnBottoms.get(orphanLayer) || INSET;
    nodes.push({
      key: `destination:unattached:${index}`,
      kind: "destination",
      height: DESTINATION_HEIGHT,
      label: destination.name,
      context: `${destination.environment || "No environment"} · execution stage unavailable`,
      environment: destination.environment || "",
      layer: orphanLayer,
      x: INSET + orphanLayer * (NODE_WIDTH + COLUMN_GAP),
      y,
      ...stateFor(destination, "unknown"),
    });
    columnBottoms.set(orphanLayer, y + DESTINATION_HEIGHT + 10);
  });

  const byKey = new Map(nodes.map(node => [node.key, node]));
  for (const edge of edges) {
    const source = byKey.get(edge.source);
    const target = byKey.get(edge.target);
    if (edge.kind === "destination") {
      const startX = source.x + NODE_WIDTH / 2;
      const startY = source.y + NODE_HEIGHT;
      const laneX = source.x + 6;
      const endY = target.y + target.height / 2;
      edge.path = `M ${startX} ${startY} V ${startY + 10} H ${laneX} V ${endY} H ${target.x - 6}`;
      edge.arrow = `${target.x - 6},${endY - 3} ${target.x},${endY} ${target.x - 6},${endY + 3}`;
    } else {
      const startX = source.x + NODE_WIDTH;
      const startY = source.y + NODE_HEIGHT / 2;
      const endX = target.x;
      const endY = target.y + NODE_HEIGHT / 2;
      const middle = (startX + endX) / 2;
      edge.path = `M ${startX} ${startY} C ${middle} ${startY}, ${middle} ${endY}, ${endX - 6} ${endY}`;
      edge.arrow = `${endX - 6},${endY - 3} ${endX},${endY} ${endX - 6},${endY + 3}`;
    }
  }

  const width = Math.max(240, ...nodes.map(node => node.x + NODE_WIDTH + INSET));
  const height = Math.max(94, ...nodes.map(node => node.y + node.height + INSET));
  return {
    key: `${projectName}:${release.slug || release.artifact_id || releaseLabel}`,
    project: projectName || "Unknown project",
    release: releaseLabel,
    href,
    nodes,
    edges,
    width,
    height,
    nodeWidth: NODE_WIDTH,
    nodeHeight: NODE_HEIGHT,
    stageCount: byStage.size,
    destinationCount: destinations.filter(destination => destination?.name).length,
    dense: height > 340 || nodes.length > 12,
    incompleteDependencies: stages.some(stage => stage?.id && (!Array.isArray(stage.depends_on) ||
      stage.depends_on.some(id => !byStage.has(id)))),
  };
}

export function latestReleases(timeline = [], { project = "" } = {}) {
  const latest = new Map();
  for (const release of releasesIn(Array.isArray(timeline) ? timeline : [])) {
    if (!release) continue;
    const key = release.project_name || project || "";
    const previous = latest.get(key);
    const createdAt = Date.parse(release.created_at);
    const previousAt = Date.parse(previous?.created_at);
    if (!previous || (Number.isFinite(createdAt) && (!Number.isFinite(previousAt) || createdAt > previousAt))) {
      latest.set(key, release);
    }
  }
  return [...latest.values()];
}

export function releaseFlows(timeline = [], { org = "", project = "" } = {}) {
  return latestReleases(timeline, { project }).map(release => projectFlow(release, org, project));
}
