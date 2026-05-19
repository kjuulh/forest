/**
 * Live event updates via SSE.
 *
 * Connects to the project events endpoint and updates the deployment UI
 * in real-time when destination statuses change.
 *
 * Usage: <script src="/static/js/live-events.js"
 *          data-org="rawpotion" data-project="my-app"></script>
 */
(function () {
  const script = document.currentScript;
  const org = script?.dataset.org;
  const project = script?.dataset.project;
  if (!org || !project) return;

  const url = `/orgs/${org}/projects/${project}/events`;
  let lastSequence = 0;
  let retryDelay = 1000;

  function connect() {
    const es = new EventSource(url);

    es.addEventListener("open", () => {
      retryDelay = 1000;
    });

    // destination status_changed events update inline badges
    es.addEventListener("destination", (e) => {
      try {
        const data = JSON.parse(e.data);
        lastSequence = Math.max(lastSequence, data.sequence || 0);
        handleDestinationEvent(data);
      } catch (err) {
        console.warn("[live-events] bad destination event:", err);
      }
    });

    // release events
    es.addEventListener("release", (e) => {
      try {
        const data = JSON.parse(e.data);
        lastSequence = Math.max(lastSequence, data.sequence || 0);
        if (data.action === "created") {
          window.location.reload();
        } else if (
          data.action === "status_changed" ||
          data.action === "updated"
        ) {
          handleReleaseEvent(data);
        }
      } catch (err) {
        console.warn("[live-events] bad release event:", err);
      }
    });

    // artifact events -> reload to show new artifacts
    es.addEventListener("artifact", (e) => {
      try {
        const data = JSON.parse(e.data);
        if (data.action === "created" || data.action === "updated") {
          window.location.reload();
        }
      } catch (err) {
        console.warn("[live-events] bad artifact event:", err);
      }
    });

    // pipeline events (pipeline run progress)
    es.addEventListener("pipeline", (e) => {
      try {
        const data = JSON.parse(e.data);
        lastSequence = Math.max(lastSequence, data.sequence || 0);
        handlePipelineEvent(data);
      } catch (err) {
        console.warn("[live-events] bad pipeline event:", err);
      }
    });

    es.addEventListener("error", () => {
      es.close();
      // Reconnect with exponential backoff
      setTimeout(connect, retryDelay);
      retryDelay = Math.min(retryDelay * 2, 30000);
    });
  }

  // ── Status update helpers ──────────────────────────────────────────

  const STATUS_CONFIG = {
    SUCCEEDED: {
      icon: "check-circle",
      iconColor: "text-green-500",
      label: "Deployed",
      labelColor: "text-green-600",
      summaryIcon: "check-circle",
      summaryColor: "text-green-500",
      summaryLabel: "Deployed to",
      summaryLabelColor: "text-gray-600",
    },
    RUNNING: {
      icon: "pulse",
      iconColor: "text-yellow-500",
      label: "Deploying",
      labelColor: "text-yellow-600",
      summaryIcon: "pulse",
      summaryColor: "text-yellow-500",
      summaryLabel: "Deploying to",
      summaryLabelColor: "text-yellow-700",
    },
    ASSIGNED: {
      icon: "pulse",
      iconColor: "text-yellow-500",
      label: "Assigned",
      labelColor: "text-yellow-600",
      summaryIcon: "pulse",
      summaryColor: "text-yellow-500",
      summaryLabel: "Deploying to",
      summaryLabelColor: "text-yellow-700",
    },
    QUEUED: {
      icon: "clock",
      iconColor: "text-blue-400",
      label: "Queued",
      labelColor: "text-blue-600",
      summaryIcon: "clock",
      summaryColor: "text-blue-400",
      summaryLabel: "Queued for",
      summaryLabelColor: "text-blue-600",
    },
    FAILED: {
      icon: "x-circle",
      iconColor: "text-red-500",
      label: "Failed",
      labelColor: "text-red-600",
      summaryIcon: "x-circle",
      summaryColor: "text-red-500",
      summaryLabel: "Failed on",
      summaryLabelColor: "text-red-600",
    },
    TIMED_OUT: {
      icon: "clock",
      iconColor: "text-orange-500",
      label: "Timed out",
      labelColor: "text-orange-600",
      summaryIcon: "clock",
      summaryColor: "text-orange-500",
      summaryLabel: "Timed out on",
      summaryLabelColor: "text-orange-600",
    },
    CANCELLED: {
      icon: "ban",
      iconColor: "text-gray-400",
      label: "Cancelled",
      labelColor: "text-gray-500",
      summaryIcon: "ban",
      summaryColor: "text-gray-400",
      summaryLabel: "Cancelled",
      summaryLabelColor: "text-gray-500",
    },
  };

  function makeStatusIcon(type, colorClass) {
    if (type === "pulse") {
      const span = document.createElement("span");
      span.className = "w-4 h-4 shrink-0 flex items-center justify-center";
      span.innerHTML =
        '<span class="w-2.5 h-2.5 rounded-full bg-yellow-500 animate-pulse"></span>';
      return span;
    }
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("class", `w-4 h-4 ${colorClass} shrink-0`);
    svg.setAttribute("fill", "none");
    svg.setAttribute("stroke", "currentColor");
    svg.setAttribute("viewBox", "0 0 24 24");
    const path = document.createElementNS(
      "http://www.w3.org/2000/svg",
      "path"
    );
    path.setAttribute("stroke-linecap", "round");
    path.setAttribute("stroke-linejoin", "round");
    path.setAttribute("stroke-width", "2");
    const paths = {
      "check-circle": "M9 12l2 2 4-4m6 2a9 9 0 11-18 0 9 9 0 0118 0z",
      "x-circle":
        "M10 14l2-2m0 0l2-2m-2 2l-2-2m2 2l2 2m7-2a9 9 0 11-18 0 9 9 0 0118 0z",
      clock: "M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z",
      ban: "M18.364 18.364A9 9 0 005.636 5.636m12.728 12.728A9 9 0 015.636 5.636m12.728 12.728L5.636 5.636",
    };
    path.setAttribute("d", paths[type] || paths["check-circle"]);
    svg.appendChild(path);
    return svg;
  }

  function handleDestinationEvent(data) {
    if (data.action !== "status_changed") return;
    const status = data.metadata?.status;
    const destName = data.metadata?.destination_name || data.resource_id;
    const env = data.metadata?.environment;
    if (!status || !destName) return;

    const config = STATUS_CONFIG[status];
    if (!config) return;

    // Find all destination rows that match
    document
      .querySelectorAll("[data-release] details .px-4.py-2")
      .forEach((row) => {
        const nameSpan = row.querySelector(".text-gray-400.text-xs");
        if (!nameSpan || nameSpan.textContent.trim() !== destName) return;

        // Update the status icon (first child element)
        const oldIcon = row.firstElementChild;
        if (oldIcon) {
          const newIcon = makeStatusIcon(config.icon, config.iconColor);
          row.replaceChild(newIcon, oldIcon);
        }

        // Update the status label text
        const labels = row.querySelectorAll("span[class*='text-xs text-']");
        labels.forEach((label) => {
          const text = label.textContent.trim();
          if (
            [
              "Deployed",
              "Deploying",
              "Assigned",
              "Queued",
              "Failed",
              "Timed out",
              "Cancelled",
            ].some((s) => text.startsWith(s))
          ) {
            label.textContent = config.label;
            // Reset classes
            label.className = `text-xs ${config.labelColor}`;
          }
        });
      });

    // Update pipeline stage rows that match this environment
    if (env) {
      updatePipelineStages(env, status, config);
    }

    // Also update the summary line for the parent release card
    updateReleaseSummary(data);
  }

  function updatePipelineStages(env, status, config) {
    document
      .querySelectorAll(
        `[data-pipeline-stage][data-stage-type="deploy"][data-stage-env="${env}"]`
      )
      .forEach((row) => {
        // Update data attributes
        row.dataset.stageStatus = status;

        // Set started_at if transitioning to an active state and not already set
        if (
          (status === "RUNNING" || status === "QUEUED") &&
          !row.dataset.startedAt
        ) {
          row.dataset.startedAt = new Date().toISOString();
        }
        // Set completed_at when reaching a terminal state
        if (
          ["SUCCEEDED", "FAILED", "TIMED_OUT", "CANCELLED"].includes(status) &&
          !row.dataset.completedAt
        ) {
          row.dataset.completedAt = new Date().toISOString();
        }

        // Ensure elapsed span exists for active stages
        if (
          (status === "RUNNING" || status === "QUEUED") &&
          !row.querySelector("[data-elapsed]")
        ) {
          const pipelineLabel = row.querySelector("span.ml-auto");
          if (pipelineLabel) {
            const el = document.createElement("span");
            el.className = "text-xs text-gray-400 tabular-nums";
            el.dataset.elapsed = "";
            pipelineLabel.before(el);
          }
        }

        // Toggle opacity for pending vs active
        if (status === "PENDING") {
          row.classList.add("opacity-50");
        } else {
          row.classList.remove("opacity-50");
        }

        // Replace status icon (first child element)
        const oldIcon = row.firstElementChild;
        if (oldIcon) {
          const newIcon = makeStatusIcon(config.icon, config.iconColor);
          row.replaceChild(newIcon, oldIcon);
        }

        // Update the status text span (e.g. "Deploying to" -> "Deployed to")
        const textSpan = row.querySelector("span.text-sm");
        if (textSpan) {
          const labels = {
            SUCCEEDED: "Deployed to",
            RUNNING: "Deploying to",
            QUEUED: "Queued for",
            FAILED: "Failed on",
            TIMED_OUT: "Timed out on",
            CANCELLED: "Cancelled",
          };
          if (labels[status]) textSpan.textContent = labels[status];
          // Update text color
          const colors = {
            SUCCEEDED: "text-gray-700",
            RUNNING: "text-yellow-700",
            QUEUED: "text-blue-600",
            FAILED: "text-red-700",
            TIMED_OUT: "text-orange-600",
            CANCELLED: "text-gray-500",
          };
          textSpan.className = `text-sm ${colors[status] || "text-gray-600"}`;
        }

        // Update the env badge dot color
        const badge = row.querySelector(
          "span.inline-flex span.rounded-full:last-child"
        );
        if (badge) {
          const dotColors = {
            SUCCEEDED: "bg-green-500",
            RUNNING: "bg-yellow-500",
            FAILED: "bg-red-500",
          };
          if (dotColors[status]) {
            badge.className = `w-1.5 h-1.5 rounded-full ${dotColors[status]}`;
          }
        }
      });
  }

  function updateReleaseSummary(_data) {
    // Re-compute summaries by scanning pipeline stage rows or destination rows.
    document.querySelectorAll("[data-release]").forEach((card) => {
      const summary = card.querySelector("details > summary");
      if (!summary) return;

      const pipelineStages = card.querySelectorAll("[data-pipeline-stage]");
      const hasPipeline = pipelineStages.length > 0;

      if (hasPipeline) {
        updatePipelineSummary(summary, pipelineStages);
      } else {
        updateDestinationSummary(summary, card);
      }
    });
  }

  function updatePipelineSummary(summary, stages) {
    let allDone = true;
    let anyFailed = false;
    let anyRunning = false;
    let anyWaiting = false;
    let done = 0;
    const total = stages.length;
    const envBadges = [];

    stages.forEach((row) => {
      const status = row.dataset.stageStatus || "PENDING";
      const stageType = row.dataset.stageType;
      const env = row.dataset.stageEnv;

      if (status === "SUCCEEDED") done++;
      if (status !== "SUCCEEDED") allDone = false;
      if (status === "FAILED") anyFailed = true;
      if (status === "RUNNING") anyRunning = true;
      if (stageType === "wait" && status === "RUNNING") anyWaiting = true;

      // Collect env badges for non-PENDING deploy stages
      if (stageType === "deploy" && status !== "PENDING" && env) {
        envBadges.push({ env, status });
      }
    });

    const chevron = summary.querySelector("svg:last-child");
    summary.innerHTML = "";

    // Pipeline gear icon
    const gear = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    gear.setAttribute("class", "w-3.5 h-3.5 text-purple-400 shrink-0");
    gear.setAttribute("fill", "none");
    gear.setAttribute("stroke", "currentColor");
    gear.setAttribute("viewBox", "0 0 24 24");
    gear.innerHTML =
      '<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.066 2.573c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.573 1.066c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.066-2.573c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z"/><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z"/>';
    summary.appendChild(gear);

    // Status icon + label
    let statusIcon, statusLabel, statusLabelColor;
    if (allDone) {
      statusIcon = makeStatusIcon("check-circle", "text-green-500");
      statusLabel = "Pipeline complete";
      statusLabelColor = "text-gray-600";
    } else if (anyFailed) {
      statusIcon = makeStatusIcon("x-circle", "text-red-500");
      statusLabel = "Pipeline failed";
      statusLabelColor = "text-red-600";
    } else if (anyWaiting) {
      statusIcon = makeStatusIcon("clock", "text-yellow-500");
      statusLabel = "Waiting for time window";
      statusLabelColor = "text-yellow-700";
    } else if (anyRunning) {
      statusIcon = makeStatusIcon("pulse", "text-yellow-500");
      statusLabel = "Deploying to";
      statusLabelColor = "text-yellow-700";
    } else {
      statusIcon = makeStatusIcon("clock", "text-gray-300");
      statusLabel = "Pipeline pending";
      statusLabelColor = "text-gray-400";
    }

    summary.appendChild(statusIcon);
    const labelSpan = document.createElement("span");
    labelSpan.className = `${statusLabelColor} text-sm`;
    labelSpan.textContent = statusLabel;
    summary.appendChild(labelSpan);

    // Environment badges
    for (const { env, status } of envBadges) {
      summary.appendChild(makeEnvBadge(env, status));
    }

    // Progress counter
    const progress = document.createElement("span");
    progress.className = "text-xs text-gray-400";
    progress.textContent = `${done}/${total}`;
    summary.appendChild(progress);

    if (chevron) summary.appendChild(chevron);
  }

  function updateDestinationSummary(summary, card) {
    // Collect current statuses from destination rows
    const rows = card.querySelectorAll("details .px-4.py-2");
    const envStatuses = new Map();
    rows.forEach((row) => {
      const envBadge = row.querySelector("[class*='rounded-full']");
      const envName =
        envBadge?.closest("span[class*='px-2']")?.textContent?.trim() || "";
      const labels = row.querySelectorAll("span[class*='text-xs text-']");
      let status = "";
      labels.forEach((l) => {
        const t = l.textContent.trim();
        if (t === "Deployed") status = "SUCCEEDED";
        else if (t === "Deploying" || t === "Assigned") status = "RUNNING";
        else if (t.startsWith("Queued")) status = "QUEUED";
        else if (t === "Failed") status = "FAILED";
        else if (t === "Timed out") status = "TIMED_OUT";
        else if (t === "Cancelled") status = "CANCELLED";
      });
      if (envName && status) envStatuses.set(envName, status);
    });

    if (envStatuses.size === 0) return;

    const groups = new Map();
    for (const [env, st] of envStatuses) {
      if (!groups.has(st)) groups.set(st, []);
      groups.get(st).push(env);
    }

    const chevron = summary.querySelector("svg:last-child");
    summary.innerHTML = "";

    for (const [status, envs] of groups) {
      const cfg = STATUS_CONFIG[status];
      if (!cfg) continue;

      summary.appendChild(makeStatusIcon(cfg.summaryIcon, cfg.summaryColor));

      const label = document.createElement("span");
      label.className = `${cfg.summaryLabelColor} text-sm`;
      label.textContent = cfg.summaryLabel;
      summary.appendChild(label);

      for (const env of envs) {
        summary.appendChild(makeEnvBadge(env, status));
      }
    }

    if (chevron) summary.appendChild(chevron);
  }

  function makeEnvBadge(env, status) {
    const badge = document.createElement("span");
    let bgClass = "bg-gray-100 text-gray-700";
    let dotClass = "bg-gray-400";
    if (env.includes("prod") && !env.includes("preprod")) {
      bgClass = "bg-pink-100 text-pink-800";
      dotClass = "bg-pink-500";
    } else if (env.includes("preprod") || env.includes("pre-prod")) {
      bgClass = "bg-orange-100 text-orange-800";
      dotClass = "bg-orange-500";
    } else if (env.includes("stag")) {
      bgClass = "bg-yellow-100 text-yellow-800";
      dotClass = "bg-yellow-500";
    } else if (env.includes("dev")) {
      bgClass = "bg-violet-100 text-violet-800";
      dotClass = "bg-violet-500";
    }
    // Override dot color based on stage status
    const statusDots = {
      SUCCEEDED: "bg-green-500",
      RUNNING: "bg-yellow-500",
      FAILED: "bg-red-500",
    };
    if (statusDots[status]) dotClass = statusDots[status];

    badge.className = `inline-flex items-center gap-1 text-xs font-medium px-2 py-0.5 rounded-full ${bgClass}`;
    badge.innerHTML = `${env} <span class="w-1.5 h-1.5 rounded-full ${dotClass}"></span>`;
    return badge;
  }

  // ── Release event handler ─────────────────────────────────────────

  function handleReleaseEvent(data) {
    // Release status_changed or updated: metadata may carry per-destination
    // updates, or a high-level status change. Treat it as a destination update
    // when we have environment + status metadata; otherwise reload for safety.
    const status = data.metadata?.status;
    const env = data.metadata?.environment;

    if (status && env) {
      // We have enough info to do an inline update
      const config = STATUS_CONFIG[status];
      if (config) {
        updatePipelineStages(env, status, config);
        updateReleaseSummary(data);
      }
    } else {
      // Generic release change — reload to pick up new state
      window.location.reload();
    }
  }

  // ── Pipeline event handler ──────────────────────────────────────────

  function handlePipelineEvent(data) {
    // Pipeline events carry stage-level status updates in metadata:
    //   stage_id, stage_type, environment, status, started_at, completed_at, error_message
    const stageStatus = data.metadata?.status;
    const stageEnv = data.metadata?.environment;
    const stageType = data.metadata?.stage_type;
    const stageId = data.metadata?.stage_id;

    if (!stageStatus) {
      // Can't do inline update without status — reload
      if (data.action === "created" || data.action === "updated") {
        window.location.reload();
      }
      return;
    }

    const config = STATUS_CONFIG[stageStatus];

    // Update pipeline stage rows by environment (deploy stages)
    if (stageEnv && config) {
      updatePipelineStages(stageEnv, stageStatus, config);
    }

    // Also update by stage_id for wait stages or when env isn't enough
    if (stageId) {
      document
        .querySelectorAll(`[data-pipeline-stage]`)
        .forEach((row) => {
          // Match on the stage id attribute if we had one, but we use
          // stage_type + env. For wait stages, update all wait stages
          // in the same card context.
          if (stageType === "wait" && row.dataset.stageType === "wait") {
            row.dataset.stageStatus = stageStatus;

            if (stageStatus === "RUNNING") {
              row.classList.remove("opacity-50");
              if (!row.dataset.startedAt) {
                row.dataset.startedAt =
                  data.metadata?.started_at || new Date().toISOString();
              }
            } else if (stageStatus === "SUCCEEDED") {
              row.classList.remove("opacity-50");
              if (!row.dataset.completedAt) {
                row.dataset.completedAt =
                  data.metadata?.completed_at || new Date().toISOString();
              }
            }

            // Update icon
            const iconCfg = STATUS_CONFIG[stageStatus];
            if (iconCfg) {
              const oldIcon = row.firstElementChild;
              if (oldIcon) {
                const newIcon = makeStatusIcon(
                  iconCfg.icon,
                  iconCfg.iconColor
                );
                row.replaceChild(newIcon, oldIcon);
              }
            }

            // Update text ("Waiting" -> "Waited")
            const textSpan = row.querySelector("span.text-sm");
            if (textSpan) {
              const dur = textSpan.textContent.match(/\d+s/)?.[0] || "";
              if (stageStatus === "SUCCEEDED") {
                textSpan.textContent = `Waited ${dur}`;
                textSpan.className = "text-sm text-gray-700";
              } else if (stageStatus === "RUNNING") {
                textSpan.textContent = `Waiting ${dur}`;
                textSpan.className = "text-sm text-yellow-700";
              } else if (stageStatus === "FAILED") {
                textSpan.textContent = `Wait failed ${dur}`;
                textSpan.className = "text-sm text-red-700";
              } else if (stageStatus === "CANCELLED") {
                textSpan.textContent = `Wait cancelled ${dur}`;
                textSpan.className = "text-sm text-gray-500";
              }
            }

            // Remove wait_until span on completion
            if (["SUCCEEDED", "FAILED", "CANCELLED"].includes(stageStatus)) {
              const waitUntil = row.querySelector("[data-wait-until]");
              if (waitUntil) waitUntil.remove();
            }

            // Ensure elapsed span exists
            if (
              (stageStatus === "RUNNING" || stageStatus === "QUEUED") &&
              !row.querySelector("[data-elapsed]")
            ) {
              const pipelineLabel = row.querySelector("span.ml-auto");
              if (pipelineLabel) {
                const el = document.createElement("span");
                el.className = "text-xs text-gray-400 tabular-nums";
                el.dataset.elapsed = "";
                pipelineLabel.before(el);
              }
            }
          }
        });
    }

    // Re-compute summary for affected cards
    updateReleaseSummary(data);
  }

  // ── Elapsed time tickers ──────────────────────────────────────────

  function formatElapsed(seconds) {
    if (seconds < 0) seconds = 0;
    if (seconds < 60) return `${seconds}s`;
    const m = Math.floor(seconds / 60);
    const s = seconds % 60;
    if (m < 60) return `${m}m ${s}s`;
    const h = Math.floor(m / 60);
    return `${h}h ${m % 60}m`;
  }

  function updateElapsedTimers() {
    document.querySelectorAll("[data-pipeline-stage]").forEach((row) => {
      const elapsed = row.querySelector("[data-elapsed]");
      if (!elapsed) return;

      const startedAt = row.dataset.startedAt;
      if (!startedAt) return;

      const start = new Date(startedAt).getTime();
      if (isNaN(start)) return;

      const completedAt = row.dataset.completedAt;
      const status = row.dataset.stageStatus;

      if (completedAt && status !== "RUNNING" && status !== "QUEUED") {
        // Completed stage — show fixed duration
        const end = new Date(completedAt).getTime();
        if (!isNaN(end)) {
          elapsed.textContent = formatElapsed(Math.floor((end - start) / 1000));
        }
      } else {
        // Active stage — live counter
        const now = Date.now();
        elapsed.textContent = formatElapsed(Math.floor((now - start) / 1000));
      }
    });
  }

  // Run immediately, then tick every second
  updateElapsedTimers();
  setInterval(updateElapsedTimers, 1000);

  // Connect on page load
  connect();
})();
