/** Status display configuration — matches live-events.js */
export const STATUS_CONFIG = {
  SUCCEEDED: { label: "Deployed to", stageLabel: "Deployed to", color: "text-green-600", icon: "check-circle", iconColor: "text-green-500" },
  RUNNING: { label: "Deploying to", stageLabel: "Deploying to", color: "text-yellow-700", icon: "pulse", iconColor: "text-yellow-500" },
  ASSIGNED: { label: "Deploying to", stageLabel: "Deploying to", color: "text-yellow-700", icon: "pulse", iconColor: "text-yellow-500" },
  QUEUED: { label: "Queued for", stageLabel: "Queued for", color: "text-blue-600", icon: "clock", iconColor: "text-blue-400" },
  FAILED: { label: "Failed on", stageLabel: "Failed on", color: "text-red-600", icon: "x-circle", iconColor: "text-red-500" },
  TIMED_OUT: { label: "Timed out on", stageLabel: "Timed out on", color: "text-orange-600", icon: "clock", iconColor: "text-orange-500" },
  CANCELLED: { label: "Cancelled", stageLabel: "Cancelled", color: "text-gray-500", icon: "ban", iconColor: "text-gray-400" },
};

export function pipelineSummary(stages) {
  if (!stages || stages.length === 0) return null;
  let allDone = true, anyFailed = false, anyRunning = false, anyWaiting = false, anyQueued = false;
  let done = 0;
  const total = stages.length;

  for (const s of stages) {
    if (s.status === "SUCCEEDED") done++;
    if (s.status !== "SUCCEEDED") allDone = false;
    if (s.status === "FAILED") anyFailed = true;
    if (s.status === "RUNNING") anyRunning = true;
    if (s.status === "QUEUED") anyQueued = true;
    if (s.stage_type === "wait" && s.status === "RUNNING") anyWaiting = true;
  }

  let anyApprovalBlocked = stages.some(s => s.blocked_by);
  let anyPlanAwaiting = stages.some(s => s.stage_type === "plan" && (s.status === "AWAITING_APPROVAL" || s.approval_status === "AWAITINGAPPROVAL" || s.approval_status === "AWAITING_APPROVAL"));

  if (allDone) return { label: "Pipeline complete", color: "text-gray-600", icon: "check-circle", iconColor: "text-green-500", done, total };
  if (anyFailed) return { label: "Pipeline failed", color: "text-red-600", icon: "x-circle", iconColor: "text-red-500", done, total };
  if (anyPlanAwaiting) return { label: "Awaiting plan approval", color: "text-purple-700", icon: "shield", iconColor: "text-purple-500", done, total };
  if (anyApprovalBlocked) return { label: "Awaiting approval", color: "text-emerald-700", icon: "shield", iconColor: "text-emerald-500", done, total };
  if (anyWaiting) return { label: "Waiting for time window", color: "text-yellow-700", icon: "clock", iconColor: "text-yellow-500", done, total };
  if (anyRunning) return { label: "Deploying to", color: "text-yellow-700", icon: "pulse", iconColor: "text-yellow-500", done, total };
  if (anyQueued) return { label: "Queued", color: "text-blue-600", icon: "clock", iconColor: "text-blue-400", done, total };
  return { label: "Pipeline pending", color: "text-gray-400", icon: "pending", iconColor: "text-gray-300", done, total };
}

export function envGroupSummary(envGroups) {
  if (!envGroups || envGroups.length === 0) return null;
  return envGroups.map(g => ({
    ...g,
    config: STATUS_CONFIG[g.status] || STATUS_CONFIG.SUCCEEDED,
  }));
}

export function waitStageLabel(status) {
  switch (status) {
    case "SUCCEEDED": return "Waited";
    case "RUNNING": return "Waiting";
    case "FAILED": return "Wait failed";
    case "CANCELLED": return "Wait cancelled";
    default: return "Wait";
  }
}

export function deployStageLabel(status) {
  switch (status) {
    case "SUCCEEDED": return "Deployed to";
    case "RUNNING": return "Deploying to";
    case "QUEUED": return "Queued for";
    case "FAILED": return "Failed on";
    case "TIMED_OUT": return "Timed out on";
    case "CANCELLED": return "Cancelled";
    default: return "Deploy to";
  }
}

export function planStageLabel(status) {
  switch (status) {
    case "SUCCEEDED": return "Plan approved";
    case "RUNNING": return "Planning";
    case "AWAITING_APPROVAL": return "Awaiting plan approval";
    case "FAILED": return "Plan failed";
    case "CANCELLED": return "Plan cancelled";
    default: return "Plan";
  }
}
