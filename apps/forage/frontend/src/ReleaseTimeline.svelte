<svelte:options customElement={{ tag: "release-timeline", shadow: "none" }} />

<script>
  import { onMount, onDestroy, tick } from "svelte";
  import { fetchTimeline, connectSSE, formatElapsed, timeAgo } from "./lib/api.js";
  import { envColors, envLaneColor, envBadgeClasses, statusDotColor } from "./lib/colors.js";
  import { pipelineSummary, deployStageLabel, waitStageLabel, planStageLabel, STATUS_CONFIG } from "./lib/status.js";

  // Props from attributes
  export let org = "";
  export let project = "";
  export let csrf = "";
  export let username = "";
  export let role = "";
  // Cap how many releases the timeline renders. Empty string / "0" /
  // missing → render all. Used by the project Overview to show a top-3
  // summary linked to the full Releases tab.
  export let limit = "";

  // Reactive state
  let timeline = [];
  let lanes = [];
  let initialLoading = true;  // only true until first successful load
  let error = null;
  let disconnectSSE = null;
  let now = Date.now();
  let timerInterval = null;

  // DOM refs for swim lane positioning
  let timelineEl = null;
  let laneBarData = {};

  const BAR_WIDTH = 20;
  const BAR_GAP = 4;
  const DOT_SIZE = 12;
  const IN_FLIGHT = new Set(["QUEUED", "RUNNING", "ASSIGNED"]);
  const DEPLOYED = new Set(["SUCCEEDED"]);

  // ── Approval action ──────────────────────────────────────────────

  let approving = new Set();
  let approvalError = null;

  function isAdmin() {
    return role === "owner" || role === "admin";
  }

  function isAuthor(release) {
    return username && release.source_user === username;
  }

  async function approveRelease(release, stage, bypass = false) {
    const key = `${release.release_intent_id}:${stage.environment}`;
    if (approving.has(key)) return;
    approving.add(key);
    approving = approving; // trigger reactivity
    approvalError = null;

    try {
      const formData = new URLSearchParams();
      formData.set("csrf_token", csrf);
      formData.set("release_intent_id", release.release_intent_id);
      formData.set("target_environment", stage.environment);
      if (bypass) formData.set("force_bypass", "true");

      const res = await fetch(
        `/orgs/${org}/projects/${release.project_name}/releases/${release.slug}/approve`,
        {
          method: "POST",
          body: formData,
          credentials: "same-origin",
          headers: {
            "Content-Type": "application/x-www-form-urlencoded",
            "Accept": "application/json",
          },
          redirect: "manual",
        }
      );
      // 303/302 redirect = success (form handler redirects after approval)
      if (res.ok || res.status === 303 || res.status === 302 || res.status === 0) {
        await refreshData();
      } else {
        // Try JSON error first, then extract from HTML
        const text = await res.text().catch(() => "");
        let msg;
        try { msg = JSON.parse(text).error; } catch {}
        if (!msg) {
          const match = text.match(/<p[^>]*>\s*(.*?)\s*<\/p>/);
          msg = match?.[1];
        }
        approvalError = msg || `Approval failed (${res.status})`;
        setTimeout(() => { approvalError = null; }, 8000);
      }
    } catch (err) {
      approvalError = err.message || "Approval request failed";
      setTimeout(() => { approvalError = null; }, 8000);
    } finally {
      approving.delete(key);
      approving = approving;
    }
  }

  // ── Plan stage actions ──────────────────────────────────────────

  let planOutputs = {};  // keyed by "intentId:stageId"
  let planOutputLoading = new Set();

  async function approvePlanStage(release, stage, reject = false) {
    const key = `plan:${release.release_intent_id}:${stage.id}`;
    if (approving.has(key)) return;
    approving.add(key);
    approving = approving;
    approvalError = null;

    try {
      const action = reject ? "reject" : "approve";
      const formData = new URLSearchParams();
      formData.set("csrf_token", csrf);
      formData.set("release_intent_id", release.release_intent_id);

      const res = await fetch(
        `/api/orgs/${org}/projects/${release.project_name || project}/plan-stages/${stage.id}/${action}`,
        {
          method: "POST",
          body: formData,
          credentials: "same-origin",
          headers: {
            "Content-Type": "application/x-www-form-urlencoded",
            "Accept": "application/json",
          },
        }
      );
      if (res.ok) {
        await refreshData();
      } else {
        const text = await res.text().catch(() => "");
        let msg;
        try { msg = JSON.parse(text).error; } catch {}
        approvalError = msg || `Plan ${action} failed (${res.status})`;
        setTimeout(() => { approvalError = null; }, 8000);
      }
    } catch (err) {
      approvalError = err.message || "Plan action failed";
      setTimeout(() => { approvalError = null; }, 8000);
    } finally {
      approving.delete(key);
      approving = approving;
    }
  }

  async function viewPlanOutput(release, stage) {
    const key = `${release.release_intent_id}:${stage.id}`;
    if (planOutputLoading.has(key)) return;
    if (planOutputs[key]) {
      // Toggle off
      delete planOutputs[key];
      planOutputs = planOutputs;
      return;
    }
    planOutputLoading.add(key);
    planOutputLoading = planOutputLoading;

    try {
      const res = await fetch(
        `/api/orgs/${org}/projects/${release.project_name || project}/plan-stages/${stage.id}/output?release_intent_id=${encodeURIComponent(release.release_intent_id)}`,
        { credentials: "same-origin", headers: { "Accept": "application/json" } }
      );
      if (res.ok) {
        const data = await res.json();
        planOutputs[key] = data;
        planOutputs = planOutputs;
      } else {
        approvalError = `Failed to load plan output (${res.status})`;
        setTimeout(() => { approvalError = null; }, 8000);
      }
    } catch (err) {
      approvalError = err.message || "Failed to load plan output";
      setTimeout(() => { approvalError = null; }, 8000);
    } finally {
      planOutputLoading.delete(key);
      planOutputLoading = planOutputLoading;
    }
  }

  // ── Data fetching ────────────────────────────────────────────────

  // Debounce re-fetches: multiple SSE events within 300ms only trigger one fetch
  let refetchTimer = null;

  function scheduleRefetch() {
    if (refetchTimer) return; // already scheduled
    refetchTimer = setTimeout(() => {
      refetchTimer = null;
      refreshData();
    }, 300);
  }

  async function loadData() {
    try {
      error = null;
      const data = await fetchTimeline(org, project);
      applyTimelineData(data.timeline, data.lanes);
      initialLoading = false;
      scheduleComputeLaneBars();
    } catch (e) {
      error = e.message;
      initialLoading = false;
    }
  }

  // Background refresh: merge new data without loading state
  async function refreshData() {
    try {
      const data = await fetchTimeline(org, project);
      applyTimelineData(data.timeline, data.lanes);
      scheduleComputeLaneBars();
    } catch (e) {
      // Silently ignore refresh failures — we still have the old data
      console.warn("[release-timeline] refresh failed:", e);
    }
  }

  // Merge new timeline data, preserving object identity where possible
  // to minimize DOM thrash. Uses slug as the stable key.
  function applyTimelineData(newTimeline, newLanes) {
    // Build a map of existing releases by slug for fast lookup
    const existingBySlug = new Map();
    for (const item of timeline) {
      if (item.kind === "release" && item.release) {
        existingBySlug.set(item.release.slug, item);
      }
    }

    // Merge: reuse existing objects when data hasn't changed
    const merged = newTimeline.map(newItem => {
      if (newItem.kind !== "release" || !newItem.release) return newItem;
      const existing = existingBySlug.get(newItem.release.slug);
      if (!existing) return newItem;
      // Shallow-compare key fields; if same, keep the old reference
      const oldR = existing.release;
      const newR = newItem.release;
      if (oldR.dest_envs === newR.dest_envs &&
          oldR.has_pipeline === newR.has_pipeline &&
          pipelineStagesEqual(oldR.pipeline_stages, newR.pipeline_stages) &&
          destinationsEqual(oldR.destinations, newR.destinations)) {
        return existing; // same reference = no DOM update
      }
      return newItem;
    });

    timeline = merged;
    lanes = newLanes;
  }

  function pipelineStagesEqual(a, b) {
    if (a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) {
      if (a[i].status !== b[i].status || a[i].started_at !== b[i].started_at || a[i].completed_at !== b[i].completed_at) return false;
    }
    return true;
  }

  function destinationsEqual(a, b) {
    if (a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) {
      if (a[i].status !== b[i].status || a[i].completed_at !== b[i].completed_at) return false;
    }
    return true;
  }

  // ── SSE event handling ───────────────────────────────────────────

  function handleEvent(type, data) {
    if (type === "destination" && data.action === "status_changed") {
      handleDestinationUpdate(data);
    } else if (type === "release") {
      if (data.action === "created") {
        scheduleRefetch();
      } else if (data.action === "status_changed" || data.action === "updated") {
        handleReleaseUpdate(data);
      }
    } else if (type === "artifact" && (data.action === "created" || data.action === "updated")) {
      scheduleRefetch();
    } else if (type === "pipeline") {
      handlePipelineUpdate(data);
    }
  }

  function handleDestinationUpdate(data) {
    const status = data.metadata?.status;
    const destName = data.metadata?.destination_name || data.resource_id;
    const env = data.metadata?.environment;
    if (!status || !destName) return;

    let changed = false;
    timeline = timeline.map(item => {
      if (item.kind !== "release" || !item.release) return item;
      const r = item.release;

      // Check if this release has a matching destination
      const destIdx = r.destinations.findIndex(d => d.name === destName);
      if (destIdx === -1) return item; // no match, keep same reference

      changed = true;
      const newDests = r.destinations.map(d =>
        d.name === destName ? { ...d, status, ...(["SUCCEEDED","FAILED","TIMED_OUT","CANCELLED"].includes(status) ? { completed_at: new Date().toISOString() } : {}) } : d
      );
      const newEnvStatuses = newDests.map(d => `${d.environment}:${d.status || "PENDING"}`).join(",");

      const newStages = env ? r.pipeline_stages.map(s =>
        s.stage_type === "deploy" && s.environment === env ? { ...s, status: status === "ASSIGNED" ? "RUNNING" : status } : s
      ) : r.pipeline_stages;

      return {
        ...item,
        release: { ...r, destinations: newDests, dest_envs: newEnvStatuses, pipeline_stages: newStages }
      };
    });
    if (changed) scheduleComputeLaneBars();
  }

  function handleReleaseUpdate(data) {
    const status = data.metadata?.status;
    const env = data.metadata?.environment;
    if (status && env) {
      handleDestinationUpdate(data);
    } else {
      scheduleRefetch();
    }
  }

  function handlePipelineUpdate(data) {
    const stageStatus = data.metadata?.status;
    const stageEnv = data.metadata?.environment;
    const stageType = data.metadata?.stage_type;
    if (!stageStatus) {
      if (data.action === "created" || data.action === "updated") scheduleRefetch();
      return;
    }

    let changed = false;
    timeline = timeline.map(item => {
      if (item.kind !== "release" || !item.release) return item;
      const r = item.release;
      let stageChanged = false;
      const newStages = r.pipeline_stages.map(s => {
        if (stageEnv && s.stage_type === "deploy" && s.environment === stageEnv) {
          stageChanged = true;
          return { ...s, status: stageStatus, ...(s.started_at ? {} : { started_at: new Date().toISOString() }) };
        }
        if (stageType === "wait" && s.stage_type === "wait") {
          stageChanged = true;
          return { ...s, status: stageStatus };
        }
        return s;
      });
      if (!stageChanged) return item; // keep same reference
      changed = true;
      return { ...item, release: { ...r, pipeline_stages: newStages } };
    });
    if (changed) scheduleComputeLaneBars();
  }

  // ── Swim lane bar computation ────────────────────────────────────

  function parseEnvs(raw) {
    if (!raw) return [];
    return raw.split(",").map(s => s.trim()).filter(Boolean).map(entry => {
      const colon = entry.indexOf(":");
      if (colon === -1) return { env: entry, status: "SUCCEEDED" };
      return { env: entry.slice(0, colon), status: entry.slice(colon + 1) };
    });
  }

  // Debounce lane bar computation to one per frame
  let laneBarRaf = null;
  function scheduleComputeLaneBars() {
    if (laneBarRaf) return;
    laneBarRaf = requestAnimationFrame(() => {
      laneBarRaf = null;
      tick().then(computeLaneBars);
    });
  }

  function computeLaneBars() {
    if (!timelineEl) return;
    const timelineRect = timelineEl.getBoundingClientRect();
    if (timelineRect.height === 0) return;
    const timelineH = timelineRect.height;

    const cards = Array.from(timelineEl.querySelectorAll("[data-release]"));
    const newBarData = {};

    for (const lane of lanes) {
      const env = lane.name;
      let deployedCard = null, flightCard = null;
      let deployedIdx = -1, flightIdx = -1;

      for (let i = 0; i < cards.length; i++) {
        const entries = parseEnvs(cards[i].dataset.envs);
        for (const entry of entries) {
          if (entry.env !== env) continue;
          if (DEPLOYED.has(entry.status) && !deployedCard) { deployedCard = cards[i]; deployedIdx = i; }
          if (IN_FLIGHT.has(entry.status) && !flightCard) { flightCard = cards[i]; flightIdx = i; }
        }
      }

      const deployedTop = deployedCard ? deployedCard.getBoundingClientRect().top - timelineRect.top : null;
      const flightTop = flightCard ? flightCard.getBoundingClientRect().top - timelineRect.top : null;

      let solidH = 0;
      if (deployedTop !== null && flightTop !== null) {
        solidH = timelineH - Math.max(deployedTop, flightTop);
      } else if (deployedTop !== null) {
        solidH = timelineH - deployedTop;
      }

      const hasHatch = !!flightCard;
      let hatchTop = 0, hatchH = 0, isForward = false;
      if (flightCard) {
        isForward = deployedIdx === -1 || flightIdx < deployedIdx;
        const anchorY = deployedTop !== null ? deployedTop : timelineH;
        const topY = Math.min(anchorY, flightTop);
        const bottomY = Math.max(anchorY, flightTop);
        hatchTop = topY;
        hatchH = Math.max(bottomY - topY, 4);
      }

      const dots = [];
      for (const card of cards) {
        const entries = parseEnvs(card.dataset.envs);
        if (!entries.find(e => e.env === env)) continue;
        const avatar = card.querySelector("[data-avatar]");
        const anchor = avatar || card;
        const r = anchor.getBoundingClientRect();
        dots.push(r.top + r.height / 2 - timelineRect.top);
      }

      newBarData[env] = { solidH, hasHatch, hatchTop, hatchH, isForward, dots, color: envColors(env) };
    }

    laneBarData = newBarData;
  }

  // ── Hatch pattern SVG ────────────────────────────────────────────

  // Cache hatch pattern data URIs to avoid re-encoding on every render
  const hatchCache = new Map();
  function hatchPattern(color, bgColor) {
    const key = `${color}|${bgColor}`;
    let cached = hatchCache.get(key);
    if (cached) return cached;
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="${bgColor}"/><path d="M-2,2 l4,-4 M0,8 l8,-8 M6,10 l4,-4" stroke="${color}" stroke-width="1.5" opacity="0.6"/></svg>`;
    cached = `url("data:image/svg+xml,${encodeURIComponent(svg)}")`;
    hatchCache.set(key, cached);
    return cached;
  }

  // ── Lifecycle ────────────────────────────────────────────────────

  onMount(() => {
    loadData();
    // Update "time ago" labels every 10 seconds instead of every 1 second
    // — 1s resolution adds no value for "3m ago" style labels
    timerInterval = setInterval(() => { now = Date.now(); }, 10000);
  });

  onDestroy(() => {
    if (disconnectSSE) disconnectSSE();
    if (timerInterval) clearInterval(timerInterval);
    if (refetchTimer) clearTimeout(refetchTimer);
    if (laneBarRaf) cancelAnimationFrame(laneBarRaf);
  });

  // Connect SSE after first data load
  $: if (!initialLoading && !error && org && !disconnectSSE) {
    disconnectSSE = connectSSE(org, project, handleEvent);
  }

  // Recompute lane bars on window resize (debounced via rAF)
  function handleResize() { scheduleComputeLaneBars(); }

  // ── Helpers for template ─────────────────────────────────────────

  function elapsedStr(startedAt, completedAt, status) {
    if (!startedAt) return "";
    const start = new Date(startedAt).getTime();
    if (isNaN(start)) return "";
    if (completedAt && status !== "RUNNING" && status !== "QUEUED") {
      const end = new Date(completedAt).getTime();
      if (!isNaN(end)) return formatElapsed(Math.floor((end - start) / 1000));
    }
    return formatElapsed(Math.floor((now - start) / 1000));
  }

  // Unique key for each timeline item (used in keyed {#each})
  function itemKey(item) {
    if (item.kind === "release" && item.release) return `r:${item.release.slug}`;
    if (item.kind === "hidden") return `h:${item.count}:${(item.releases || [])[0]?.slug || ""}`;
    return `u:${Math.random()}`;
  }

  // Which deploy stages to show as badges on the summary line,
  // filtered to match the current pipeline state.
  function summaryShowsStage(summary, stageStatus) {
    if (!summary) return false;
    switch (summary.label) {
      case "Pipeline complete":  return stageStatus === "SUCCEEDED";
      case "Pipeline failed":    return stageStatus === "FAILED" || stageStatus === "RUNNING" || stageStatus === "ASSIGNED";
      case "Deploying to":       return stageStatus === "RUNNING" || stageStatus === "ASSIGNED";
      case "Queued":             return stageStatus === "QUEUED";
      case "Waiting for time window": return stageStatus === "RUNNING" || stageStatus === "ASSIGNED";
      default:                   return stageStatus !== "PENDING" && stageStatus !== "SUCCEEDED";
    }
  }

  // Normalize plan stage status: the API returns status="RUNNING" with
  // approval_status="AWAITINGAPPROVAL" (no underscore, Debug format from Rust).
  // Map this to a single effective status for template rendering.
  function effectiveStatus(stage) {
    if (stage.stage_type === "plan" && stage.approval_status &&
        (stage.approval_status === "AWAITINGAPPROVAL" || stage.approval_status === "AWAITING_APPROVAL")) {
      return "AWAITING_APPROVAL";
    }
    return stage.status;
  }

  function isPlanAwaiting(stage) {
    return stage.stage_type === "plan" && effectiveStatus(stage) === "AWAITING_APPROVAL";
  }

  $: laneCount = lanes.length;
  $: gutterWidth = laneCount * (BAR_WIDTH + BAR_GAP) + 8;
</script>

<svelte:window on:resize={handleResize} />

{#if approvalError}
  <div class="max-w-5xl mx-auto mb-4 px-4 py-3 border border-red-200 bg-red-50 rounded-lg flex items-center gap-2 text-sm text-red-700">
    <svg class="w-4 h-4 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8v4m0 4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>
    {approvalError}
    <button class="ml-auto text-red-400 hover:text-red-600" on:click={() => approvalError = null}>
      <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"/></svg>
    </button>
  </div>
{/if}

{#if initialLoading}
  <div class="max-w-5xl mx-auto p-12 text-center text-gray-400">
    <span class="w-5 h-5 inline-block border-2 border-gray-300 border-t-gray-600 rounded-full animate-spin"></span>
    <p class="mt-2 text-sm">Loading releases...</p>
  </div>
{:else if error}
  <div class="max-w-5xl mx-auto p-6 border border-red-200 rounded-lg text-center">
    <p class="text-red-600">{error}</p>
    <button class="mt-2 text-sm text-gray-500 hover:text-gray-900 underline" on:click={loadData}>Retry</button>
  </div>
{:else if timeline.length === 0}
  <div class="max-w-5xl mx-auto p-6 border border-gray-200 rounded-lg text-center">
    <p class="text-gray-600">No releases yet.</p>
    <p class="text-sm text-gray-400 mt-2">Create a release with <code class="bg-gray-100 px-1 rounded">forest release create</code></p>
  </div>
{:else}
  <div class="max-w-5xl mx-auto grid" style="grid-template-columns: {gutterWidth}px 1fr; grid-template-rows: 1fr auto;">
    <!-- Swim lane gutter -->
    <div class="flex" style="grid-row: 1;">
      {#each lanes as lane (lane.name)}
        {@const bar = laneBarData[lane.name]}
        {@const [barColor, lightColor] = bar?.color || [lane.color, "#e5e7eb"]}
        <div style="width: {BAR_WIDTH}px; margin-right: {BAR_GAP}px; position: relative;">
          {#if bar}
            {#if bar.hasHatch}
              <div class="lane-bar lane-pulse" style="position: absolute; left: 0; width: 100%; top: {bar.hatchTop}px; height: {bar.hatchH + (bar.solidH > 0 ? BAR_WIDTH / 2 : 0)}px; background-image: {bar.isForward ? hatchPattern(barColor, lightColor) : hatchPattern('#f59e0b', '#fef3c7')}; background-size: 8px 8px; background-repeat: repeat; border-radius: 9999px; z-index: 0;"></div>
            {/if}
            {#if bar.solidH > 0}
              <div class="lane-bar" style="position: absolute; bottom: 0; left: 0; width: 100%; height: {bar.solidH + (bar.hasHatch ? BAR_WIDTH / 2 : 0)}px; background: {barColor}; border-radius: 9999px; z-index: 1;"></div>
            {/if}
            {#each bar.dots as dotY, di (di)}
              <div class="lane-dot" style="position: absolute; left: 50%; transform: translateX(-50%); top: {dotY - DOT_SIZE/2}px; width: {DOT_SIZE}px; height: {DOT_SIZE}px; border-radius: 50%; background: #fff; border: 2px solid {barColor}; z-index: 2;"></div>
            {/each}
          {/if}
        </div>
      {/each}
    </div>

    <!-- Timeline cards. When `limit` is set (Overview summary use case),
         render only the first N items; the rest are accessible via the
         full Releases tab. -->
    <div bind:this={timelineEl} class="space-y-3 min-w-0" style="grid-row: 1;">
      {#each (limit && Number(limit) > 0 ? timeline.slice(0, Number(limit)) : timeline) as item (itemKey(item))}
        {#if item.kind === "release" && item.release}
          {@const release = item.release}
          <div data-release data-envs={release.dest_envs} class="border border-gray-200 rounded-lg overflow-hidden">
            <div class="px-4 py-3 flex items-center gap-3 flex-wrap">
              <div class="flex items-center gap-2 min-w-0 flex-1">
                <span class="inline-block w-6 h-6 rounded-full bg-gray-200 shrink-0" data-avatar></span>
                <a href="/orgs/{org}/projects/{release.project_name || project}/releases/{release.slug}" class="font-medium text-gray-900 hover:text-black truncate" title={release.title}>
                  {release.title?.length > 80 ? release.title.slice(0, 80) + "…" : release.title}
                </a>
              </div>
              <div class="flex items-center gap-4 text-xs text-gray-500 shrink-0 flex-wrap">
                {#if release.branch}
                  <span class="flex items-center gap-1">
                    <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M7 7h.01M7 3h5c.512 0 1.024.195 1.414.586l7 7a2 2 0 010 2.828l-7 7a2 2 0 01-2.828 0l-7-7A2 2 0 013 12V7a4 4 0 014-4z"/></svg>
                    {release.branch}
                  </span>
                {/if}
                {#if release.commit_sha}
                  <span class="font-mono">{release.commit_sha.slice(0, 7)}</span>
                {/if}
                <time>{timeAgo(release.created_at)}</time>
                {#if release.source_user}
                  <span class="flex items-center gap-1">
                    <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M16 7a4 4 0 11-8 0 4 4 0 018 0zM12 14a7 7 0 00-7 7h14a7 7 0 00-7-7z"/></svg>
                    <a href="/users/{release.source_user}" class="hover:underline">{release.source_user}</a>
                  </span>
                {/if}
                {#if release.project_name && release.project_name !== project}
                  <a href="/orgs/{org}/projects/{release.project_name}" class="hover:underline">{release.project_name}</a>
                {/if}
              </div>
            </div>

            <!-- Summary + details -->
            <details class="border-t border-gray-100 group" on:toggle={scheduleComputeLaneBars}>
              <summary class="px-4 py-2 flex items-center gap-2 text-sm cursor-pointer list-none hover:bg-gray-50 flex-wrap">
                {#if release.has_pipeline && !pipelineSummary(release.pipeline_stages)}
                  <!-- Pipeline exists but not triggered yet -->
                  {@const envAllDone = release.env_groups && release.env_groups.length > 0 && release.env_groups.every(g => g.status === "SUCCEEDED")}
                  <svg class="w-3.5 h-3.5 text-purple-400 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.066 2.573c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.573 1.066c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.066-2.573c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z"/><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z"/></svg>
                  {#if envAllDone}
                    <svg class="w-4 h-4 text-green-500 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m6 2a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>
                    <span class="text-gray-500 text-sm">Deployed</span>
                  {:else}
                    <svg class="w-4 h-4 text-blue-400 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>
                    <span class="text-blue-600 text-sm">Queued</span>
                  {/if}
                {:else if release.has_pipeline && pipelineSummary(release.pipeline_stages)}
                  {@const summary = pipelineSummary(release.pipeline_stages)}
                  <svg class="w-3.5 h-3.5 text-purple-400 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.066 2.573c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.573 1.066c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.066-2.573c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z"/><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z"/></svg>
                  {#if summary.icon === "pulse"}
                    <span class="w-4 h-4 shrink-0 flex items-center justify-center"><span class="w-2.5 h-2.5 rounded-full bg-yellow-500 animate-pulse"></span></span>
                  {:else if summary.icon === "check-circle"}
                    <svg class="w-4 h-4 {summary.iconColor} shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m6 2a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>
                  {:else if summary.icon === "x-circle"}
                    <svg class="w-4 h-4 {summary.iconColor} shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10 14l2-2m0 0l2-2m-2 2l-2-2m2 2l2 2m7-2a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>
                  {:else if summary.icon === "clock"}
                    <svg class="w-4 h-4 {summary.iconColor} shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>
                  {:else if summary.icon === "shield"}
                    <svg class="w-4 h-4 {summary.iconColor} shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m5.618-4.016A11.955 11.955 0 0112 2.944a11.955 11.955 0 01-8.618 3.04A12.02 12.02 0 003 9c0 5.591 3.824 10.29 9 11.622 5.176-1.332 9-6.03 9-11.622 0-1.042-.133-2.052-.382-3.016z"/></svg>
                  {:else}
                    <svg class="w-4 h-4 text-gray-300 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><circle cx="12" cy="12" r="9" stroke-width="2"/></svg>
                  {/if}
                  <span class="{summary.color} text-sm">{summary.label}</span>

                  {#each release.pipeline_stages as stage (stage.id || stage.environment || stage.stage_type)}
                    {#if stage.stage_type === "deploy" && summaryShowsStage(summary, stage.status)}
                      {@const badge = envBadgeClasses(stage.environment || "")}
                      {@const dot = statusDotColor(stage.status) || badge.dot}
                      <span class="inline-flex items-center gap-1 text-xs font-medium px-2 py-0.5 rounded-full {badge.bg}">
                        {stage.environment}
                        <span class="w-1.5 h-1.5 rounded-full {dot}"></span>
                      </span>
                    {/if}
                    {#if stage.stage_type === "plan" && isPlanAwaiting(stage) && release.release_intent_id && csrf}
                      {@const planBadge = envBadgeClasses(stage.environment || "")}
                      <span class="inline-flex items-center gap-1 text-xs font-medium px-2 py-0.5 rounded-full bg-purple-100">
                        {stage.environment} plan
                        <span class="w-1.5 h-1.5 rounded-full bg-purple-400"></span>
                      </span>
                      <button
                        class="text-xs px-2 py-0.5 rounded-md bg-green-600 text-white hover:bg-green-700 transition-colors disabled:opacity-50"
                        disabled={approving.has(`plan:${release.release_intent_id}:${stage.id}`)}
                        on:click|stopPropagation={() => approvePlanStage(release, stage)}
                      >Approve plan</button>
                    {/if}
                    {#if stage.blocked_by && release.release_intent_id && csrf}
                      {#if isAuthor(release) && isAdmin()}
                        <button
                          class="text-xs px-2 py-0.5 rounded-md bg-red-600 text-white hover:bg-red-700 transition-colors disabled:opacity-50"
                          disabled={approving.has(`${release.release_intent_id}:${stage.environment}`)}
                          on:click|stopPropagation={() => { if (confirm('You are the release author. Bypass approval?')) approveRelease(release, stage, true); }}
                        >Bypass</button>
                      {:else if !isAuthor(release)}
                        <button
                          class="text-xs px-2 py-0.5 rounded-md bg-green-600 text-white hover:bg-green-700 transition-colors disabled:opacity-50"
                          disabled={approving.has(`${release.release_intent_id}:${stage.environment}`)}
                          on:click|stopPropagation={() => approveRelease(release, stage)}
                        >Approve</button>
                      {/if}
                    {/if}
                  {/each}

                  <span class="text-xs text-gray-400">{summary.done}/{summary.total}</span>

                {:else if release.env_groups && release.env_groups.length > 0}
                  {@const allSucceeded = release.env_groups.every(g => g.status === "SUCCEEDED")}
                  {#if allSucceeded}
                    <svg class="w-4 h-4 text-green-500 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m6 2a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>
                    <span class="text-gray-500 text-sm">Deployed</span>
                  {:else}
                    {#each release.env_groups as group, gi (gi)}
                      {#if group.status !== "SUCCEEDED"}
                        {@const cfg = STATUS_CONFIG[group.status] || STATUS_CONFIG.SUCCEEDED}
                        {#if cfg.icon === "pulse"}
                          <span class="w-4 h-4 shrink-0 flex items-center justify-center"><span class="w-2.5 h-2.5 rounded-full bg-yellow-500 animate-pulse"></span></span>
                        {:else if cfg.icon === "check-circle"}
                          <svg class="w-4 h-4 {cfg.iconColor} shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m6 2a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>
                        {:else}
                          <svg class="w-4 h-4 {cfg.iconColor} shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>
                        {/if}
                        <span class="{cfg.color} text-sm">{cfg.label}</span>
                        {#each group.envs as env (env)}
                          {@const badge = envBadgeClasses(env)}
                          <span class="inline-flex items-center gap-1 text-xs font-medium px-2 py-0.5 rounded-full {badge.bg}">
                            {env}
                            <span class="w-1.5 h-1.5 rounded-full {badge.dot}"></span>
                          </span>
                        {/each}
                      {/if}
                    {/each}
                  {/if}
                {:else}
                  <svg class="w-4 h-4 text-gray-300 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>
                  <span class="text-gray-400 text-sm">Pending</span>
                {/if}

                <svg class="w-3 h-3 text-gray-400 shrink-0 ml-auto transition-transform group-open:rotate-90" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7"/></svg>
              </summary>

              <!-- Release details — clamp the body to ~400 chars so
                   very long commit messages don't dominate the page.
                   Full text in a `title=` tooltip for hover. -->
              <div class="px-4 py-3 border-t border-gray-100 space-y-3">
                {#if release.description}
                  {@const desc = release.description}
                  <p class="text-sm text-gray-700 whitespace-pre-wrap break-words" title={desc}>
                    {desc.length > 400 ? desc.slice(0, 400) + "…" : desc}
                  </p>
                {/if}
                <div class="flex flex-wrap gap-x-6 gap-y-2 text-xs text-gray-500">
                  <span class="font-mono text-gray-400">{release.slug}</span>
                  {#if release.version}
                    <span class="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-green-100 text-green-800">{release.version}</span>
                  {/if}
                </div>
              </div>

              <!-- Pipeline stages -->
              {#if release.has_pipeline}
                <div class="border-t border-gray-100">
                  {#each release.pipeline_stages as stage, i (stage.id || `${stage.stage_type}-${stage.environment}-${i}`)}
                    {@const stageStatus = effectiveStatus(stage)}
                    <div class="px-4 py-2.5 flex items-center gap-3 text-sm {i < release.pipeline_stages.length - 1 ? 'border-b border-gray-50' : ''} {stageStatus === 'PENDING' ? 'opacity-50' : ''}">
                      {#if stageStatus === "SUCCEEDED"}
                        <svg class="w-4 h-4 text-green-500 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m6 2a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>
                      {:else if stageStatus === "RUNNING"}
                        <span class="w-4 h-4 shrink-0 flex items-center justify-center"><span class="w-2.5 h-2.5 rounded-full bg-yellow-500 animate-pulse"></span></span>
                      {:else if stageStatus === "QUEUED"}
                        <svg class="w-4 h-4 text-blue-400 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>
                      {:else if stageStatus === "FAILED"}
                        <svg class="w-4 h-4 text-red-500 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10 14l2-2m0 0l2-2m-2 2l-2-2m2 2l2 2m7-2a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>
                      {:else if stageStatus === "AWAITING_APPROVAL"}
                        <svg class="w-4 h-4 text-purple-500 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m5.618-4.016A11.955 11.955 0 0112 2.944a11.955 11.955 0 01-8.618 3.04A12.02 12.02 0 003 9c0 5.591 3.824 10.29 9 11.622 5.176-1.332 9-6.03 9-11.622 0-1.042-.133-2.052-.382-3.016z"/></svg>
                      {:else}
                        <svg class="w-4 h-4 text-gray-300 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><circle cx="12" cy="12" r="9" stroke-width="2"/></svg>
                      {/if}

                      {#if stage.stage_type === "deploy"}
                        <span class="text-sm {stage.status === 'SUCCEEDED' ? 'text-gray-700' : stage.status === 'RUNNING' ? 'text-yellow-700' : stage.status === 'FAILED' ? 'text-red-700' : 'text-gray-400'}">
                          {deployStageLabel(stage.status)}
                        </span>
                        {@const badge = envBadgeClasses(stage.environment || "")}
                        <span class="inline-flex items-center gap-1 text-xs font-medium px-2 py-0.5 rounded-full {badge.bg}">
                          {stage.environment}
                          <span class="w-1.5 h-1.5 rounded-full {badge.dot}"></span>
                        </span>
                      {:else if stage.stage_type === "wait"}
                        <span class="text-sm {stage.status === 'SUCCEEDED' ? 'text-gray-700' : stage.status === 'RUNNING' ? 'text-yellow-700' : 'text-gray-400'}">
                          {waitStageLabel(stage.status)} {stage.duration_seconds}s
                        </span>
                      {:else if stage.stage_type === "plan"}
                        <span class="text-sm {stageStatus === 'AWAITING_APPROVAL' ? 'text-purple-700' : stageStatus === 'SUCCEEDED' ? 'text-gray-700' : stageStatus === 'RUNNING' ? 'text-yellow-700' : stageStatus === 'FAILED' ? 'text-red-700' : 'text-gray-400'}">
                          {planStageLabel(stageStatus)}
                        </span>
                        {@const planBadge = envBadgeClasses(stage.environment || "")}
                        <span class="inline-flex items-center gap-1 text-xs font-medium px-2 py-0.5 rounded-full {planBadge.bg}">
                          {stage.environment}
                          <span class="w-1.5 h-1.5 rounded-full {planBadge.dot}"></span>
                        </span>
                        {#if stageStatus === "AWAITING_APPROVAL" && release.release_intent_id && csrf}
                          <button
                            class="text-xs px-2 py-0.5 rounded-md bg-green-600 text-white hover:bg-green-700 transition-colors disabled:opacity-50"
                            disabled={approving.has(`plan:${release.release_intent_id}:${stage.id}`)}
                            on:click|stopPropagation={() => approvePlanStage(release, stage)}
                          >Approve plan</button>
                          <button
                            class="text-xs px-2 py-0.5 rounded-md bg-red-600 text-white hover:bg-red-700 transition-colors disabled:opacity-50"
                            disabled={approving.has(`plan:${release.release_intent_id}:${stage.id}`)}
                            on:click|stopPropagation={() => { if (confirm('Reject this plan?')) approvePlanStage(release, stage, true); }}
                          >Reject</button>
                        {/if}
                        {#if (stageStatus === "AWAITING_APPROVAL" || stageStatus === "SUCCEEDED" || stageStatus === "FAILED") && release.release_intent_id}
                          <button
                            class="text-xs px-2 py-0.5 rounded-md border border-gray-300 text-gray-600 hover:bg-gray-50 transition-colors disabled:opacity-50"
                            disabled={planOutputLoading.has(`${release.release_intent_id}:${stage.id}`)}
                            on:click|stopPropagation={() => viewPlanOutput(release, stage)}
                          >{planOutputs[`${release.release_intent_id}:${stage.id}`] ? "Hide plan" : "View plan"}</button>
                        {/if}
                      {/if}

                      {#if stage.started_at && (stageStatus === "RUNNING" || stageStatus === "QUEUED" || stageStatus === "AWAITING_APPROVAL" || stage.completed_at)}
                        <span class="text-xs text-gray-400 tabular-nums">{elapsedStr(stage.started_at, stage.completed_at, stage.status)}</span>
                      {/if}

                      <span class="ml-auto flex items-center gap-1 text-xs text-gray-400 shrink-0">
                        <svg class="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.066 2.573c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.573 1.066c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.066-2.573c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z"/><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z"/></svg>
                        pipeline
                      </span>
                    </div>
                    {#if stage.stage_type === "plan" && planOutputs[`${release.release_intent_id}:${stage.id}`]}
                      {@const planData = planOutputs[`${release.release_intent_id}:${stage.id}`]}
                      <div class="px-4 py-3 bg-gray-50 border-t border-gray-100 space-y-3">
                        <div class="flex items-center gap-2">
                          <span class="text-xs font-medium text-gray-500">Plan output</span>
                          <span class="text-xs px-1.5 py-0.5 rounded bg-purple-100 text-purple-700">{planData.status}</span>
                        </div>
                        {#if planData.outputs && planData.outputs.length > 0}
                          {#each planData.outputs as destOutput (destOutput.destination_id)}
                            <div>
                              <div class="flex items-center gap-2 mb-1">
                                <span class="text-xs font-medium text-gray-600">{destOutput.destination_name}</span>
                                <span class="text-xs text-gray-400">{destOutput.status}</span>
                              </div>
                              <pre class="text-xs font-mono text-gray-700 whitespace-pre-wrap bg-white border border-gray-200 rounded p-3 max-h-48 overflow-auto">{destOutput.plan_output || "(no output)"}</pre>
                            </div>
                          {/each}
                        {:else}
                          <pre class="text-xs font-mono text-gray-700 whitespace-pre-wrap bg-white border border-gray-200 rounded p-3 max-h-64 overflow-auto">{planData.plan_output || "(no output)"}</pre>
                        {/if}
                      </div>
                    {/if}
                  {/each}
                </div>
              {/if}

              <!-- Destinations -->
              {#each release.destinations as dest, i (dest.name)}
                {@const destBadge = envBadgeClasses(dest.environment || "")}
                <div class="px-4 py-2 flex items-center gap-3 text-sm {i < release.destinations.length - 1 ? 'border-b border-gray-50' : ''} border-t border-gray-100">
                  {#if dest.status === "SUCCEEDED"}
                    <svg class="w-4 h-4 text-green-500 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m6 2a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>
                  {:else if dest.status === "RUNNING" || dest.status === "ASSIGNED"}
                    <span class="w-4 h-4 shrink-0 flex items-center justify-center"><span class="w-2.5 h-2.5 rounded-full bg-yellow-500 animate-pulse"></span></span>
                  {:else if dest.status === "QUEUED"}
                    <svg class="w-4 h-4 text-blue-400 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>
                  {:else if dest.status === "FAILED"}
                    <svg class="w-4 h-4 text-red-500 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10 14l2-2m0 0l2-2m-2 2l-2-2m2 2l2 2m7-2a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>
                  {:else}
                    <svg class="w-4 h-4 text-gray-300 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m6 2a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>
                  {/if}
                  <span class="inline-flex items-center gap-1 text-xs font-medium px-2 py-0.5 rounded-full {destBadge.bg}">
                    {dest.environment}
                    <span class="w-1.5 h-1.5 rounded-full {destBadge.dot}"></span>
                  </span>
                  <span class="text-gray-400 text-xs">{dest.name}</span>
                  {#if dest.status === "SUCCEEDED"}
                    <span class="text-xs text-green-600">Deployed</span>
                  {:else if dest.status === "RUNNING"}
                    <span class="text-xs text-yellow-600">Deploying</span>
                  {:else if dest.status === "QUEUED"}
                    <span class="text-xs text-blue-600">Queued{dest.queue_position ? ` #${dest.queue_position}` : ""}</span>
                  {:else if dest.status === "FAILED"}
                    <span class="text-xs text-red-600">Failed</span>
                  {/if}
                  {#if dest.completed_at}
                    <time class="text-xs text-gray-400 ml-auto">{timeAgo(dest.completed_at)}</time>
                  {/if}
                </div>
              {/each}
            </details>
          </div>

        {:else if item.kind === "hidden"}
          <details class="group" on:toggle={scheduleComputeLaneBars}>
            <summary class="flex items-center gap-2 py-2 px-1 text-sm text-gray-400 cursor-pointer hover:text-gray-600 list-none">
              <svg class="w-3 h-3 transition-transform group-open:rotate-90" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7"/></svg>
              {item.count} hidden commit{item.count !== 1 ? "s" : ""}
              <span class="text-gray-300">&middot;</span>
              <span class="group-open:hidden">Show commit{item.count !== 1 ? "s" : ""}</span>
              <span class="hidden group-open:inline">Hide commit{item.count !== 1 ? "s" : ""}</span>
            </summary>
            <div class="space-y-3 mt-1">
              {#each item.releases || [] as release (release.slug)}
                <div data-release data-envs="" class="border border-gray-200 rounded-lg overflow-hidden opacity-75">
                  <div class="px-4 py-3 flex items-center gap-3 flex-wrap">
                    <div class="flex items-center gap-2 min-w-0 flex-1">
                      <span class="inline-block w-6 h-6 rounded-full bg-gray-200 shrink-0" data-avatar></span>
                      <a href="/orgs/{org}/projects/{release.project_name || project}/releases/{release.slug}" class="font-medium text-gray-900 hover:text-black truncate" title={release.title}>
                        {release.title?.length > 80 ? release.title.slice(0, 80) + "…" : release.title}
                      </a>
                    </div>
                    <div class="flex items-center gap-4 text-xs text-gray-500 shrink-0">
                      {#if release.commit_sha}
                        <span class="font-mono">{release.commit_sha.slice(0, 7)}</span>
                      {/if}
                      <time>{timeAgo(release.created_at)}</time>
                    </div>
                  </div>
                </div>
              {/each}
            </div>
          </details>
        {/if}
      {/each}
    </div>

    <!-- Lane labels (row 2, column 1) -->
    <div class="flex pt-1" style="grid-row: 2; grid-column: 1; height: 56px;">
      {#each lanes as lane (lane.name)}
        <div style="width: {BAR_WIDTH}px; margin-right: {BAR_GAP}px; display: flex; justify-content: center;">
          <span style="writing-mode: vertical-rl; transform: rotate(180deg); font-size: 10px; font-weight: 500; color: {lane.color}; white-space: nowrap;">{lane.name}</span>
        </div>
      {/each}
    </div>
  </div>
{/if}

<style>
  @keyframes lane-pulse {
    0%, 100% { opacity: 0.6; }
    50% { opacity: 1; }
  }
  :global(.lane-pulse) {
    animation: lane-pulse 2s ease-in-out infinite;
  }
</style>
