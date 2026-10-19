<svelte:options customElement={{ tag: "release-timeline", shadow: "none" }} />

<script>
  import { onMount, onDestroy, tick } from "svelte";
  import { fetchTimeline, connectSSE, formatElapsed, timeAgo } from "./lib/api.js";
  import { envColorPair, envChipStyle, orderLanes, envRank } from "./lib/colors.js";
  import {
    IN_FLIGHT, DEPLOYED, STOPPED,
    effectiveStatus, isPlanAwaiting, isGateAwaiting, releaseEnvStates, laneStatesAttr,
    timelineLaneStatesAttrs, isUnfinished, timelineDestinations, timelineDestinationStates,
    destinationsByEnv, releaseStoppedEnvs, releaseDestinationStates, timelineRollbacks,
  } from "./lib/lane-states.js";
  import { laneGeometry } from "./lib/lane-geometry.js";
  import { pipelineSummary, deployStageLabel, waitStageLabel, planStageLabel, gateStageLabel, STATUS_CONFIG } from "./lib/status.js";

  export let org = "";
  export let project = "";
  export let csrf = "";
  export let username = "";
  export let role = "";
  export let limit = "";

  let timeline = [];
  let lanes = [];
  let initialLoading = true;
  let error = null;
  let disconnectSSE = null;
  let now = Date.now();
  let timerInterval = null;

  let timelineEl = null;
  let laneBarData = {};
  let laneBarRaf = null;
  let laneBarScheduled = false;
  let laneBarRetryCount = 0;

  const LANE_W = 14;
  const LANE_GAP = 6;
  const STRAND_W = 12;
  const STRAND_GAP = 12;
  const GUTTER_INSET = 4;
  const HIT_OVERHANG = 3;

  const HEAD_KINDS = new Set(["live", "stopped", "awaiting"]);

  function dotSize(kind, strandWidth) {
    const head = HEAD_KINDS.has(kind);
    const size = Math.max(Math.round(strandWidth * (head ? 0.58 : 0.43)), head ? 5 : 4);
    return (strandWidth - size) % 2 === 0 ? size : size + 1;
  }

  function dotInset(size, strandWidth) {
    return (strandWidth - size) / 2;
  }

  function metricsFor(strandWidth) {
    return { cap: strandWidth / 2, dot: dotSize("live", strandWidth) };
  }
  const MAX_LANE_BAR_RETRIES = 8;

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
    approving = approving;
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
      if (res.ok || res.status === 303 || res.status === 302 || res.status === 0) {
        await refreshData();
      } else {
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

  let planOutputs = {};
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

  let refetchTimer = null;

  function scheduleRefetch() {
    if (refetchTimer) return;
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

  async function refreshData() {
    try {
      const data = await fetchTimeline(org, project);
      applyTimelineData(data.timeline, data.lanes);
      scheduleComputeLaneBars();
    } catch (e) {
      console.warn("[release-timeline] refresh failed:", e);
    }
  }

  function applyTimelineData(newTimeline, newLanes) {
    const existingBySlug = new Map();
    for (const item of timeline) {
      if (item.kind === "release" && item.release) {
        existingBySlug.set(item.release.slug, item);
      }
    }

    const merged = newTimeline.map(newItem => {
      if (newItem.kind !== "release" || !newItem.release) return newItem;
      const existing = existingBySlug.get(newItem.release.slug);
      if (!existing) return newItem;
      const oldR = existing.release;
      const newR = newItem.release;
      if (oldR.dest_envs === newR.dest_envs &&
          oldR.has_pipeline === newR.has_pipeline &&
          pipelineStagesEqual(oldR.pipeline_stages, newR.pipeline_stages) &&
          destinationsEqual(oldR.destinations, newR.destinations)) {
        return existing;
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

      const destIdx = r.destinations.findIndex(d => d.name === destName);
      if (destIdx === -1) return item;

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
      if (!stageChanged) return item;
      changed = true;
      return { ...item, release: { ...r, pipeline_stages: newStages } };
    });
    if (changed) scheduleComputeLaneBars();
  }

  function scheduleComputeLaneBars() {
    if (laneBarScheduled) return;
    laneBarScheduled = true;
    tick().then(() => {
      laneBarRaf = requestAnimationFrame(() => {
        laneBarRaf = null;
        laneBarScheduled = false;
        computeLaneBars();
      });
    });
  }

  function retryComputeLaneBars() {
    if (laneBarRetryCount >= MAX_LANE_BAR_RETRIES) return;
    laneBarRetryCount += 1;
    scheduleComputeLaneBars();
  }

  function measureRows() {
    const timelineRect = timelineEl.getBoundingClientRect();
    const cards = Array.from(timelineEl.querySelectorAll("[data-release]"));
    if (timelineRect.height === 0 || cards.length === 0) return null;

    const ys = new Map();
    for (const card of cards) {
      const slug = card.dataset.releaseSlug;
      if (!slug) continue;
      const anchor = card.querySelector("[data-avatar]") || card;
      const r = anchor.getBoundingClientRect();
      ys.set(slug, r.top + r.height / 2 - timelineRect.top);
    }
    return { height: timelineRect.height, ys };
  }

  function computeLaneBars() {
    if (!displayedLanes.length || !timelineEl) {
      if (displayedLanes.length) retryComputeLaneBars();
      else { laneBarData = {}; laneBarRetryCount = 0; }
      return;
    }

    const measured = measureRows();
    if (!measured) { retryComputeLaneBars(); return; }
    const { height, ys } = measured;

    const next = {};
    for (const lane of displayedLanes) {
      const env = lane.name;
      const destNames = destinationsByLane.get(env) || [];

      const rows = [];
      for (const release of visibleReleaseList) {
        const y = ys.get(release.slug);
        if (y === undefined) continue;
        const kind = laneStateFor(release.slug, env);
        if (!kind) continue;
        rows.push({ y, kind, slug: release.slug, release });
      }
      markStopped(rows, env);

      const geometry = laneGeometry(rows, height, metricsFor(LANE_W));

      const strands = destNames.map((name) => {
        const drows = [];
        for (const release of visibleReleaseList) {
          const y = ys.get(release.slug);
          if (y === undefined) continue;
          const kind = destStatesBySlug.get(release.slug)?.get(name);
          if (!kind) continue;
          drows.push({ y, kind, slug: release.slug, release });
        }
        const strandW = expandedLanes.has(env) && destNames.length > 1 ? STRAND_W : LANE_W;
        return { name, geometry: laneGeometry(drows, height, metricsFor(strandW)), rows: drows };
      });

      const headRow = rows.find((r) => r.kind === "live") || null;
      const movingRow = rows.find((r) => isUnfinished(r.kind)) || null;

      const marks = [...rows, ...strands.flatMap((st) => st.rows)].map((r) => r.y);
      const trackTop = marks.length ? Math.max(Math.min(...marks) - metricsFor(LANE_W).cap, 0) : null;

      next[env] = {
        geometry,
        rows,
        strands,
        trackTop,
        headRow,
        movingRow,
        color: envColorPair(env),
      };
    }

    laneBarRetryCount = 0;
    laneBarData = next;
  }

  function markStopped(rows, env) {
    const headIdx = rows.findIndex((r) => r.kind === "live");
    for (let i = 0; i < rows.length; i++) {
      if (headIdx !== -1 && i > headIdx) break;
      if (rows[i].kind !== "past") continue;
      if (!releaseStoppedEnvs(rows[i].release).has(env)) continue;
      rows[i] = { ...rows[i], kind: "stopped" };
      break;
    }
  }

  let expandedLanes = new Set();

  function toggleLane(env) {
    const next = new Set(expandedLanes);
    if (next.has(env)) next.delete(env);
    else next.add(env);
    expandedLanes = next;
    scheduleComputeLaneBars();
  }

  $: laneLayout = (() => {
    const map = new Map();
    for (const lane of displayedLanes) {
      const dests = destinationsByLane.get(lane.name) || [];
      const fans = dests.length > 1;
      const open = fans && expandedLanes.has(lane.name);
      map.set(lane.name, {
        dests,
        fans,
        open,
        width: open ? dests.length * STRAND_W + (dests.length - 1) * STRAND_GAP : LANE_W,
        offsets: dests.map((_, i) =>
          open
            ? { left: i * (STRAND_W + STRAND_GAP), width: STRAND_W }
            : { left: 0, width: LANE_W },
        ),
      });
    }
    return map;
  })();

  let hovered = null;

  const HOVER_REACH = 22;

  function showLaneCard(env, event) {
    const bar = laneBarData[env];
    if (!bar) return;
    const rect = event.currentTarget.getBoundingClientRect();
    const gutter = event.currentTarget.closest(".rt-gutter")?.getBoundingClientRect();
    const pointed = event.type !== "focus" && typeof event.clientY === "number";

    const layout = laneLayout.get(env);
    let dest = null;
    let rows = bar.rows;
    if (pointed && layout?.open) {
      const localX = event.clientX - rect.left - HIT_OVERHANG;
      const i = layout.offsets.findIndex(
        (o) => localX >= o.left - STRAND_GAP / 2 && localX <= o.left + o.width + STRAND_GAP / 2,
      );
      if (i !== -1) {
        dest = layout.dests[i];
        rows = bar.strands.find((st) => st.name === dest)?.rows ?? rows;
      }
    }

    let row = null;
    if (pointed) {
      const localY = event.clientY - rect.top;
      let best = null;
      for (const r of rows) {
        const d = Math.abs(r.y - localY);
        if (d <= HOVER_REACH && (!best || d < best.d)) best = { d, r };
      }
      row = best?.r ?? null;
    }

    const top = gutter
      ? pointed
        ? event.clientY - gutter.top - 18
        : (bar.headRow?.y ?? 0)
      : 0;

    const next = { env, dest, row, top: Math.max(top, 0) };
    if (
      hovered &&
      hovered.env === next.env &&
      hovered.dest === next.dest &&
      hovered.row === next.row &&
      Math.abs(hovered.top - next.top) < 2
    ) {
      return;
    }
    hovered = next;
  }

  function hideLaneCard() {
    hovered = null;
  }

  function laneCardFacts(env, dest, row) {
    const bar = laneBarData[env];
    if (!bar) return null;
    const rows = dest ? bar.strands.find((s) => s.name === dest)?.rows || [] : bar.rows;
    const head = rows.find((r) => r.kind === "live");
    const subject = row || rows.find((r) => isUnfinished(r.kind) || r.kind === "stopped") || head;
    if (!subject) return { env, dest, empty: true };
    return {
      env,
      dest,
      empty: false,
      pinned: Boolean(row),
      kind: subject.kind,
      release: subject.release,
      head: head?.release || null,
      count: dest || row ? null : (destinationsByLane.get(env) || []).length,
    };
  }

  function shortDestination(env, name) {
    for (const sep of ["-", "_", "/", "."]) {
      const prefix = `${env}${sep}`;
      if (name.startsWith(prefix) && name.length > prefix.length) return name.slice(prefix.length);
    }
    return name;
  }

  const EMPTY_SET = new Set();

  const KIND_WORDS = {
    live: "Live here",
    flight: "Deploying",
    awaiting: "Awaiting approval",
    pending: "Queued",
    stopped: "Failed",
    past: "Previously released",
  };

  const DEST_WORDS = {
    live: "Deployed",
    flight: "Deploying",
    awaiting: "Waiting for approval",
    pending: "Not started",
    stopped: "Failed",
    past: "Deployed, since replaced",
  };

  function showsDestinations(dests, release) {
    if (dests.length > 1) return true;
    return dests.some((d) => {
      const row = (release.destinations || []).find((x) => x.name === d.name);
      return d.kind === "stopped" || row?.error_message || row?.queue_position;
    });
  }


  function glyphSignal(icon) {
    switch (icon) {
      case "check-circle": return "ok";
      case "x-circle": return "fail";
      case "pulse": return "running";
      case "shield": return "attention";
      case "clock": return "waiting";
      default: return "queued";
    }
  }

  function stageSignal(status) {
    switch (status) {
      case "SUCCEEDED": return "ok";
      case "RUNNING": return "running";
      case "QUEUED": return "waiting";
      case "FAILED":
      case "TIMED_OUT": return "fail";
      case "AWAITING_APPROVAL": return "attention";
      case "AWAITING_SIGNAL": return "waiting";
      case "CANCELLED": return "cancelled";
      default: return "queued";
    }
  }

  function releaseDestinationRows(release) {
    return releaseDestinationStates(release);
  }

  onMount(() => {
    loadData();
    timerInterval = setInterval(() => { now = Date.now(); }, 10000);
  });

  onDestroy(() => {
    if (disconnectSSE) disconnectSSE();
    if (timerInterval) clearInterval(timerInterval);
    if (refetchTimer) clearTimeout(refetchTimer);
    if (laneBarRaf) cancelAnimationFrame(laneBarRaf);
    laneBarScheduled = false;
  });

  $: if (!initialLoading && !error && org && !disconnectSSE) {
    disconnectSSE = connectSSE(org, project, handleEvent);
  }

  $: if (!initialLoading && renderedTimeline.length && laneCount > 0) scheduleComputeLaneBars();
  function handleResize() { scheduleComputeLaneBars(); }

  let avatarFailed = new Set();

  function avatarSrc(user) {
    return `/avatars/${encodeURIComponent(user)}`;
  }

  function avatarMissing(user) {
    if (!user || avatarFailed.has(user)) return;
    avatarFailed = new Set(avatarFailed).add(user);
  }

  function initial(user) {
    return user ? user.slice(0, 1).toUpperCase() : "";
  }

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

  function itemKey(item) {
    if (item.kind === "release" && item.release) return `r:${item.release.slug}`;
    if (item.kind === "hidden") return `h:${item.count}:${(item.releases || [])[0]?.slug || ""}`;
    return `u:${Math.random()}`;
  }

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

  function laneNamesInTimeline(items) {
    const names = new Set();
    for (const item of items) {
      if (item.kind !== "release" || !item.release) continue;
      for (const { env } of releaseEnvStates(item.release)) {
        names.add(env);
      }
    }
    return names;
  }

  function resolveLaneStates(items) {
    const releases = items.filter(i => i.kind === "release" && i.release).map(i => i.release);
    const attrs = timelineLaneStatesAttrs(releases);
    const bySlug = new Map();
    releases.forEach((r, i) => bySlug.set(r.slug, attrs[i]));
    return bySlug;
  }

  function parseEnvs(raw) {
    if (!raw) return [];
    return raw.split(",").map(s => s.trim()).filter(Boolean).map(entry => {
      const colon = entry.indexOf(":");
      if (colon === -1) return { env: entry, status: "SUCCEEDED" };
      return { env: entry.slice(0, colon), status: entry.slice(colon + 1) };
    });
  }

  function laneStateFor(slug, env) {
    return parseEnvs(laneStatesBySlug.get(slug)).find(e => e.env === env)?.status || null;
  }

  function cardAccent(release) {
    const live = releaseEnvStates(release)
      .filter((s) => s.kind === "live")
      .sort((a, b) => envRank(a.env) - envRank(b.env));
    return live[0]?.env || null;
  }

  const PAGE_SIZE = 20;
  let visibleReleases = PAGE_SIZE;

  function itemReleaseCount(item) {
    if (item.kind !== "hidden") return 1;
    return item.count ?? (item.releases || []).length;
  }

  function takeReleases(items, target) {
    let count = 0;
    for (let i = 0; i < items.length; i++) {
      count += itemReleaseCount(items[i]);
      if (count >= target) return items.slice(0, i + 1);
    }
    return items;
  }

  function showMore() {
    visibleReleases = renderedReleaseCount + PAGE_SIZE;
    scheduleComputeLaneBars();
  }

  $: hardLimit = limit && Number(limit) > 0 ? Number(limit) : 0;
  $: renderedTimeline = hardLimit
    ? timeline.slice(0, hardLimit)
    : takeReleases(timeline, visibleReleases);
  $: renderedReleaseCount = renderedTimeline.reduce((n, item) => n + itemReleaseCount(item), 0);
  $: hasMore = !hardLimit && renderedTimeline.length < timeline.length;
  $: renderedLaneNames = laneNamesInTimeline(renderedTimeline);
  $: laneStatesBySlug = resolveLaneStates(renderedTimeline);

  $: visibleReleaseList = renderedTimeline
    .filter(i => i.kind === "release" && i.release)
    .map(i => i.release);
  $: destinationsByLane = timelineDestinations(visibleReleaseList);
  $: rollbacksBySlug = (() => {
    const sets = timelineRollbacks(visibleReleaseList);
    const bySlug = new Map();
    visibleReleaseList.forEach((r, i) => bySlug.set(r.slug, sets[i]));
    return bySlug;
  })();
  $: destStatesBySlug = (() => {
    const states = timelineDestinationStates(visibleReleaseList);
    const bySlug = new Map();
    visibleReleaseList.forEach((r, i) => bySlug.set(r.slug, states[i]));
    return bySlug;
  })();

  $: displayedLanes = orderLanes(lanes.filter(lane => renderedLaneNames.has(lane.name)));
  $: laneCount = displayedLanes.length;
  $: gutterWidth = laneCount > 0
    ? [...laneLayout.values()].reduce((w, l) => w + l.width + LANE_GAP, 0) + GUTTER_INSET + 4
    : 0;
</script>

<svelte:window on:resize={handleResize} />

{#if approvalError}
  <div class="rt-alert" role="alert">
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" aria-hidden="true"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8v4m0 4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>
    {approvalError}
    <button class="rt-alert-close" aria-label="Dismiss approval error" on:click={() => approvalError = null}>
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" aria-hidden="true"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"/></svg>
    </button>
  </div>
{/if}

{#if initialLoading}
  <div class="rt-placeholder">
    <span class="rt-spinner" aria-hidden="true"></span>
    <p>Loading releases…</p>
  </div>
{:else if error}
  <div class="rt-placeholder rt-placeholder-error">
    <p>{error}</p>
    <button class="rt-link-button" on:click={loadData}>Try again</button>
  </div>
{:else if timeline.length === 0}
  <div class="rt-placeholder">
    <p class="rt-empty-title">No releases yet</p>
    <p>Ship one with <code>forest release create</code>.</p>
  </div>
{:else}
  <div
    class="rt"
    style="grid-template-columns: {gutterWidth}px minmax(0, 1fr);"
    class:rt-limited={hardLimit > 0}
  >
    <div class="rt-gutter" style="grid-row: 1; grid-column: 1;">
      {#each displayedLanes as lane (lane.name)}
        {@const bar = laneBarData[lane.name]}
        {@const [light, dark] = bar?.color || envColorPair(lane.name)}
        {@const L = laneLayout.get(lane.name) || { open: false, fans: false, width: LANE_W, offsets: [], dests: [] }}
        <div
          class="rt-lane env-scope"
          class:rt-lane-open={L.open}
          data-env={lane.name}
          style="--env: {light}; --env-dark: {dark}; width: {L.width}px; margin-right: {LANE_GAP}px;"
        >
          {#if bar?.trackTop !== null && bar?.trackTop !== undefined}
            <div class="rt-track" style="top: {bar.trackTop}px;" aria-hidden="true"></div>
          {/if}

          {#if bar}
            {#each (L.open ? bar.strands : [{ name: null, geometry: bar.geometry, rows: bar.rows }]) as strand, si (strand.name ?? "env")}
              {@const off = L.offsets[si] || { left: 0, width: LANE_W }}
              {@const g = strand.geometry}
              <div class="rt-strand" style="left: {off.left}px; width: {off.width}px;">
                {#each g.runs as run, ri (`${run.layer}:${run.kind}:${ri}`)}
                  <div
                    class="lane-run"
                    data-run={run.kind}
                    data-layer={run.layer}
                    data-direction={run.direction}
                    data-motion={run.motion}
                    style="top: {run.top}px; height: {run.height}px;"
                  ></div>
                {/each}
                {#each g.dots as row (row.slug)}
                  {@const size = dotSize(row.kind, off.width)}
                  <span
                    class="lane-dot"
                    class:lane-pulse={isUnfinished(row.kind)}
                    data-kind={row.kind}
                    data-tone={row.tone}
                    style="top: {row.y - size / 2}px; left: {dotInset(size, off.width)}px; width: {size}px; height: {size}px;"
                    title={`${KIND_WORDS[row.kind] || row.kind} — ${strand.name || lane.name}`}
                  ></span>
                {/each}
              </div>
            {/each}
          {/if}

          <button
            type="button"
            class="rt-lane-hit"
            aria-expanded={L.fans ? L.open : undefined}
            aria-label={L.fans
              ? `${lane.name}: ${L.dests.length} destinations, ${L.open ? "collapse" : "expand"}`
              : lane.name}
            on:click={() => L.fans && toggleLane(lane.name)}
            on:mouseenter={(e) => showLaneCard(lane.name, e)}
            on:mousemove={(e) => showLaneCard(lane.name, e)}
            on:focus={(e) => showLaneCard(lane.name, e)}
            on:mouseleave={hideLaneCard}
            on:blur={hideLaneCard}
          ></button>
        </div>
      {/each}

      {#if hovered}
        {@const facts = laneCardFacts(hovered.env, hovered.dest, hovered.row)}
        {#if facts}
          {@const [light, dark] = envColorPair(hovered.env)}
          <div class="rt-hovercard env-scope" style="--env: {light}; --env-dark: {dark}; top: {hovered.top}px;">
            <p class="rt-hovercard-title">
              <span class="rt-hovercard-swatch" aria-hidden="true"></span>
              {hovered.dest || hovered.env}
              <span class="rt-hovercard-scope">{facts.pinned ? "this release" : "now"}</span>
            </p>
            {#if facts.empty}
              <p class="rt-hovercard-empty">Nothing has reached this environment yet.</p>
            {:else}
              <dl class="rt-hovercard-facts">
                <dt>Status</dt>
                <dd data-kind={facts.kind}>{KIND_WORDS[facts.kind] || facts.kind}</dd>
                <dt>Commit</dt>
                <dd class="rt-mono">{facts.release.commit_sha ? facts.release.commit_sha.slice(0, 7) : facts.release.slug}</dd>
                <dt>Release</dt>
                <dd class="rt-hovercard-release">{facts.release.title}</dd>
                {#if facts.release.source_user}
                  <dt>By</dt>
                  <dd>{facts.release.source_user}</dd>
                {/if}
                <dt>Started</dt>
                <dd>{timeAgo(facts.release.created_at)}</dd>
                {#if facts.count && facts.count > 1}
                  <dt>Placements</dt>
                  <dd>{facts.count} destinations — click to fan out</dd>
                {/if}
              </dl>
            {/if}
          </div>
        {/if}
      {/if}
    </div>

    <div bind:this={timelineEl} class="rt-cards" style="grid-row: 1; grid-column: 2;">
      {#each renderedTimeline as item (itemKey(item))}
        {#if item.kind === "release" && item.release}
          {@const release = item.release}
          {@const accent = cardAccent(release)}
          {@const accentPair = accent ? envColorPair(accent) : null}
          {@const summary = release.has_pipeline ? pipelineSummary(release.pipeline_stages) : null}
          {@const backwards = rollbacksBySlug.get(release.slug) ?? EMPTY_SET}
          {@const destEnvs = destinationsByEnv(release)}
          <article
            data-release
            data-release-slug={release.slug}
            data-envs={release.dest_envs}
            data-lane-states={laneStatesBySlug.get(release.slug) ?? laneStatesAttr(release)}
            class="rt-card env-scope"
            class:rt-card-accented={!!accent}
            style={accentPair ? `--env: ${accentPair[0]}; --env-dark: ${accentPair[1]};` : ""}
          >
            <header class="rt-card-head">
              {#if release.source_user && !avatarFailed.has(release.source_user)}
                <img
                  data-avatar
                  src={avatarSrc(release.source_user)}
                  alt=""
                  title="Released by {release.source_user}"
                  class="rt-avatar"
                  on:error={() => avatarMissing(release.source_user)}
                />
              {:else}
                <span
                  data-avatar
                  title={release.source_user ? `Released by ${release.source_user}` : undefined}
                  class="rt-avatar rt-avatar-initial"
                >{initial(release.source_user)}</span>
              {/if}

              <a
                href="/orgs/{org}/projects/{release.project_name || project}/releases/{release.slug}"
                class="rt-card-title"
                title={release.title}
              >{release.title}</a>

              <div class="rt-meta">
                {#if release.branch}
                  <span class="rt-meta-item" title="Branch">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" aria-hidden="true"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 3v12m0 0a3 3 0 103 3m-3-3a3 3 0 113-3m9-6a3 3 0 11-3 3m3-3v6a6 6 0 01-6 6"/></svg>
                    {release.branch}
                  </span>
                {/if}
                {#if release.commit_sha}
                  <span class="rt-meta-item rt-mono" title={release.commit_sha}>{release.commit_sha.slice(0, 7)}</span>
                {/if}
                <time class="rt-meta-item" datetime={release.created_at}>{timeAgo(release.created_at)}</time>
                {#if release.source_user}
                  <a class="rt-meta-item" href="/users/{release.source_user}">{release.source_user}</a>
                {/if}
                {#if release.project_name && release.project_name !== project}
                  <a class="rt-meta-item" href="/orgs/{org}/projects/{release.project_name}">{release.project_name}</a>
                {/if}
              </div>
            </header>

            <details class="rt-details" on:toggle={scheduleComputeLaneBars}>
              <summary class="rt-summary">
                {#if release.has_pipeline && !summary}
                  {@const envAllDone = release.env_groups && release.env_groups.length > 0 && release.env_groups.every(g => g.status === "SUCCEEDED")}
                  <span class="rt-glyph" data-signal={envAllDone ? "ok" : "queued"} aria-hidden="true"></span>
                  <span class="rt-summary-label">{envAllDone ? "Released" : "Queued"}</span>
                {:else if summary}
                  <span class="rt-glyph" data-signal={backwards.size ? "attention" : glyphSignal(summary.icon)} aria-hidden="true"></span>
                  <span
                    class="rt-summary-label"
                    data-signal={backwards.size ? "attention" : glyphSignal(summary.icon)}
                  >{backwards.size && summary.label === "Deploying to" ? "Rolling back to" : summary.label}</span>

                  {#each release.pipeline_stages as stage, i (stage.id || `${stage.stage_type}-${stage.environment}-${i}`)}
                    {#if stage.stage_type === "deploy" && summaryShowsStage(summary, stage.status)}
                      {@const dests = destEnvs.get(stage.environment) || []}
                      <span class="rt-chip env-scope" class:rt-chip-back={backwards.has(stage.environment)} style={envChipStyle(stage.environment)}>
                        {stage.environment}
                        <span
                          class="rt-chip-mark"
                          data-status={stage.status}
                          data-direction={backwards.has(stage.environment) ? "reverse" : "forward"}
                          aria-hidden="true"
                        ></span>
                        {#if dests.length > 1}
                          <span class="rt-chip-count rt-mono" title="reached {dests.filter(d => d.kind === 'live' || d.kind === 'past').length} of {dests.length} destinations">{dests.filter(d => d.kind === "live" || d.kind === "past").length}/{dests.length}</span>
                        {/if}
                      </span>
                    {/if}
                    {#if stage.stage_type === "plan" && isPlanAwaiting(stage) && release.release_intent_id && csrf}
                      <span class="rt-chip rt-chip-attention">{stage.environment} plan</span>
                      <button
                        class="rt-button rt-button-go"
                        disabled={approving.has(`plan:${release.release_intent_id}:${stage.id}`)}
                        on:click|preventDefault|stopPropagation={() => approvePlanStage(release, stage)}
                      >Approve plan</button>
                    {/if}
                    {#if stage.blocked_by && release.release_intent_id && csrf}
                      {#if isAuthor(release) && isAdmin()}
                        <button
                          class="rt-button rt-button-warn"
                          disabled={approving.has(`${release.release_intent_id}:${stage.environment}`)}
                          on:click|preventDefault|stopPropagation={() => { if (confirm('You are the release author. Bypass approval?')) approveRelease(release, stage, true); }}
                        >Bypass</button>
                      {:else if !isAuthor(release)}
                        <button
                          class="rt-button rt-button-go"
                          disabled={approving.has(`${release.release_intent_id}:${stage.environment}`)}
                          on:click|preventDefault|stopPropagation={() => approveRelease(release, stage)}
                        >Approve</button>
                      {/if}
                    {/if}
                  {/each}

                  <span class="rt-progress rt-mono" title="{summary.done} of {summary.total} stages finished">
                    {summary.done}/{summary.total}
                  </span>
                {:else if release.env_groups && release.env_groups.length > 0}
                  {@const allSucceeded = release.env_groups.every(g => g.status === "SUCCEEDED")}
                  {#if allSucceeded}
                    <span class="rt-glyph" data-signal="ok" aria-hidden="true"></span>
                    <span class="rt-summary-label">Released</span>
                    {#each release.env_groups as group, gi (gi)}
                      {#each group.envs as env (env)}
                        <span class="rt-chip env-scope" style={envChipStyle(env)}>
                          {env}<span class="rt-chip-mark" data-status="SUCCEEDED" aria-hidden="true"></span>
                        </span>
                      {/each}
                    {/each}
                  {:else}
                    {#each release.env_groups as group, gi (gi)}
                      {#if group.status !== "SUCCEEDED"}
                        {@const cfg = STATUS_CONFIG[group.status] || STATUS_CONFIG.SUCCEEDED}
                        <span class="rt-glyph" data-signal={glyphSignal(cfg.icon)} aria-hidden="true"></span>
                        <span class="rt-summary-label">{cfg.label}</span>
                        {#each group.envs as env (env)}
                          <span class="rt-chip env-scope" style={envChipStyle(env)}>
                            {env}<span class="rt-chip-mark" data-status={group.status} aria-hidden="true"></span>
                          </span>
                        {/each}
                      {/if}
                    {/each}
                  {/if}
                {:else}
                  <span class="rt-glyph" data-signal="queued" aria-hidden="true"></span>
                  <span class="rt-summary-label rt-muted">Not released yet</span>
                {/if}

                <span class="rt-disclosure" aria-hidden="true">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7"/></svg>
                </span>
              </summary>

              <div class="rt-body">
                {#if release.description}
                  <p class="rt-description" title={release.description}>{release.description}</p>
                {/if}

                {#if release.has_pipeline}
                  <ol class="rt-stages">
                    {#each release.pipeline_stages as stage, i (stage.id || `${stage.stage_type}-${stage.environment}-${i}`)}
                      {@const stageStatus = effectiveStatus(stage)}
                      {@const future = stageStatus === "PENDING"}
                      <li class="rt-stage" class:rt-stage-future={future}>
                        <span class="rt-glyph" data-signal={stageSignal(stageStatus)} aria-hidden="true"></span>

                        {#if stage.stage_type === "deploy"}
                          <span class="rt-stage-label">{deployStageLabel(stage.status)}</span>
                          <span class="rt-chip env-scope" style={envChipStyle(stage.environment || "")}>
                            {stage.environment}<span class="rt-chip-mark" data-status={stage.status} aria-hidden="true"></span>
                          </span>
                        {:else if stage.stage_type === "wait"}
                          <span class="rt-stage-label">{waitStageLabel(stage.status)} {stage.duration_seconds}s</span>
                        {:else if stage.stage_type === "plan"}
                          <span class="rt-stage-label">{planStageLabel(stageStatus)}</span>
                          <span class="rt-chip env-scope" style={envChipStyle(stage.environment || "")}>
                            {stage.environment}<span class="rt-chip-mark" data-status={stage.status} aria-hidden="true"></span>
                          </span>
                          {#if stageStatus === "AWAITING_APPROVAL" && release.release_intent_id && csrf}
                            <button
                              class="rt-button rt-button-go"
                              disabled={approving.has(`plan:${release.release_intent_id}:${stage.id}`)}
                              on:click|stopPropagation={() => approvePlanStage(release, stage)}
                            >Approve plan</button>
                            <button
                              class="rt-button rt-button-warn"
                              disabled={approving.has(`plan:${release.release_intent_id}:${stage.id}`)}
                              on:click|stopPropagation={() => { if (confirm('Reject this plan?')) approvePlanStage(release, stage, true); }}
                            >Reject</button>
                          {/if}
                        {:else if stage.stage_type === "gate"}
                          <span class="rt-stage-label">{gateStageLabel(stageStatus)}</span>
                          {#if isGateAwaiting(stage)}
                            {#each stage.gate_waiting_on as waiting}
                              <span class="rt-chip rt-chip-attention">{waiting}</span>
                            {/each}
                          {:else if stageStatus === "FAILED" && stage.error_message}
                            <a
                              class="rt-why"
                              href="/orgs/{org}/projects/{release.project_name || project}/releases/{release.slug}"
                            >Why it failed</a>
                          {/if}
                        {/if}

                        {#if stage.stage_type === "plan" && (stageStatus === "AWAITING_APPROVAL" || stageStatus === "SUCCEEDED" || stageStatus === "FAILED") && release.release_intent_id}
                          <button
                            class="rt-button"
                            disabled={planOutputLoading.has(`${release.release_intent_id}:${stage.id}`)}
                            on:click|stopPropagation={() => viewPlanOutput(release, stage)}
                          >{planOutputs[`${release.release_intent_id}:${stage.id}`] ? "Hide plan" : "View plan"}</button>
                        {/if}

                        {#if stage.started_at && (stageStatus === "RUNNING" || stageStatus === "QUEUED" || stageStatus === "AWAITING_APPROVAL" || stage.completed_at)}
                          <span class="rt-stage-elapsed rt-mono">{elapsedStr(stage.started_at, stage.completed_at, stage.status)}</span>
                        {/if}

                        {#if stage.stage_type === "deploy" && showsDestinations(destEnvs.get(stage.environment) || [], release)}
                          <ul class="rt-destinations">
                            {#each destEnvs.get(stage.environment) as dest (dest.name)}
                              {@const row = (release.destinations || []).find(d => d.name === dest.name)}
                              <li class="rt-destination" data-kind={dest.kind}>
                                <span class="rt-dest-pip" aria-hidden="true"></span>
                                <span class="rt-mono rt-dest-name">{dest.name}</span>
                                <span class="rt-dest-state">{DEST_WORDS[dest.kind] || dest.kind}</span>
                                {#if row?.queue_position}
                                  <span class="rt-mono rt-muted">#{row.queue_position}</span>
                                {/if}
                                {#if row?.error_message}
                                  <a
                                    class="rt-why"
                                    href="/orgs/{org}/projects/{release.project_name || project}/releases/{release.slug}"
                                  >Why it failed</a>
                                {/if}
                                {#if row?.completed_at}
                                  <time class="rt-dest-time">{timeAgo(row.completed_at)}</time>
                                {/if}
                              </li>
                            {/each}
                          </ul>
                        {/if}
                      </li>

                      {#if stage.stage_type === "plan" && planOutputs[`${release.release_intent_id}:${stage.id}`]}
                        {@const planData = planOutputs[`${release.release_intent_id}:${stage.id}`]}
                        <li class="rt-plan-output">
                          <div class="rt-plan-head">
                            <span>Plan output</span>
                            <span class="rt-chip rt-chip-attention">{planData.status}</span>
                          </div>
                          {#if planData.outputs && planData.outputs.length > 0}
                            {#each planData.outputs as destOutput (destOutput.destination_id)}
                              <div class="rt-plan-block">
                                <div class="rt-plan-block-head">
                                  <span class="rt-mono">{destOutput.destination_name}</span>
                                  <span class="rt-muted">{destOutput.status}</span>
                                </div>
                                <pre>{destOutput.plan_output || "(no output)"}</pre>
                              </div>
                            {/each}
                          {:else}
                            <pre>{planData.plan_output || "(no output)"}</pre>
                          {/if}
                        </li>
                      {/if}
                    {/each}
                  </ol>
                {:else if release.destinations.length > 0}
                  <ul class="rt-destinations rt-destinations-flat">
                    {#each releaseDestinationRows(release) as dest (dest.name)}
                      <li class="rt-destination" data-kind={dest.kind}>
                        <span class="rt-dest-pip" aria-hidden="true"></span>
                        <span class="rt-chip env-scope" style={envChipStyle(dest.env)}>
                          {dest.env}<span class="rt-chip-mark" data-status={dest.kind === "live" ? "SUCCEEDED" : ""} aria-hidden="true"></span>
                        </span>
                        <span class="rt-mono rt-dest-name">{dest.name}</span>
                        <span class="rt-dest-state">{DEST_WORDS[dest.kind] || dest.kind}</span>
                      </li>
                    {/each}
                  </ul>
                {/if}

                <p class="rt-footnote">
                  <span class="rt-mono">{release.slug}</span>
                  {#if release.version}<span class="rt-mono rt-version">{release.version}</span>{/if}
                </p>
              </div>
            </details>
          </article>

        {:else if item.kind === "hidden"}
          <details class="rt-hidden" on:toggle={scheduleComputeLaneBars}>
            <summary>
              <span class="rt-hidden-rule" aria-hidden="true"></span>
              <span class="rt-hidden-count">{item.count} hidden commit{item.count !== 1 ? "s" : ""}</span>
              <span class="rt-hidden-action">Show</span>
              <span class="rt-hidden-rule" aria-hidden="true"></span>
            </summary>
            <div class="rt-hidden-list">
              {#each item.releases || [] as release (release.slug)}
                <article data-release data-release-slug={release.slug} data-envs="" data-lane-states="" class="rt-card rt-card-quiet">
                  <header class="rt-card-head">
                    {#if release.source_user && !avatarFailed.has(release.source_user)}
                      <img
                        data-avatar
                        src={avatarSrc(release.source_user)}
                        alt=""
                        title="Committed by {release.source_user}"
                        class="rt-avatar"
                        on:error={() => avatarMissing(release.source_user)}
                      />
                    {:else}
                      <span data-avatar class="rt-avatar rt-avatar-initial">{initial(release.source_user)}</span>
                    {/if}
                    <a href="/orgs/{org}/projects/{release.project_name || project}/releases/{release.slug}" class="rt-card-title" title={release.title}>{release.title}</a>
                    <div class="rt-meta">
                      {#if release.commit_sha}<span class="rt-meta-item rt-mono">{release.commit_sha.slice(0, 7)}</span>{/if}
                      <time class="rt-meta-item">{timeAgo(release.created_at)}</time>
                    </div>
                  </header>
                </article>
              {/each}
            </div>
          </details>
        {/if}
      {/each}
    </div>

    {#if hasMore}
      <div class="rt-more" style="grid-row: 2; grid-column: 2;">
        <button type="button" on:click={showMore}>Show more releases</button>
      </div>
    {/if}

    <div class="rt-labels" style="grid-row: 2; grid-column: 1;">
      {#each displayedLanes as lane (lane.name)}
        {@const [light, dark] = envColorPair(lane.name)}
        {@const L = laneLayout.get(lane.name) || { open: false, width: LANE_W, offsets: [], dests: [] }}
        <div class="rt-label-slot env-scope" style="--env: {light}; --env-dark: {dark}; width: {L.width}px; margin-right: {LANE_GAP}px;">
          {#if L.open}
            {#each L.dests as dest, di (dest)}
              <span class="rt-lane-label rt-lane-label-dest" style="left: {L.offsets[di]?.left ?? 0}px;" title={dest}>{shortDestination(lane.name, dest)}</span>
            {/each}
          {:else}
            <span class="rt-lane-label">{lane.name}{L.fans ? ` ${L.dests.length}` : ""}</span>
          {/if}
        </div>
      {/each}
    </div>
  </div>
{/if}

<style>
  .rt {
    --gutter-inset: 4px;

    --surface: var(--color-white, #fff);
    --surface-sunken: var(--color-gray-50, #f9fafb);
    --line: var(--color-gray-200, #e5e7eb);
    --line-soft: var(--color-gray-100, #f3f4f6);
    --ink: var(--color-gray-900, #111827);
    --ink-soft: var(--color-gray-600, #4b5563);
    --ink-faint: var(--color-gray-400, #9ca3af);

    --sig-ok: #059669;
    --sig-fail: #dc2626;
    --sig-attn: #d97706;
    --sig-attn-bg: #fde68a;
    --sig-run: #0284c7;
    --sig-idle: #9ca3af;
  }

  @media (prefers-color-scheme: dark) {
    .rt {
      --sig-ok: #34d399;
      --sig-fail: #f87171;
      --sig-attn: #fbbf24;
      --sig-attn-bg: #a16207;
      --sig-run: #38bdf8;
      --sig-idle: #6b7280;
    }
  }

  :global(:root[data-theme="dark"]) .rt {
    --sig-ok: #34d399;
    --sig-fail: #f87171;
    --sig-attn: #fbbf24;
    --sig-attn-bg: #a16207;
    --sig-run: #38bdf8;
    --sig-idle: #6b7280;
  }

  :global(:root[data-theme="light"]) .rt {
    --sig-ok: #059669;
    --sig-fail: #dc2626;
    --sig-attn: #d97706;
    --sig-attn-bg: #fde68a;
    --sig-run: #0284c7;
    --sig-idle: #9ca3af;
  }

  .env-scope { --env-c: var(--env); }
  @media (prefers-color-scheme: dark) {
    .env-scope { --env-c: var(--env-dark); }
  }
  :global(:root[data-theme="dark"]) .env-scope { --env-c: var(--env-dark); }
  :global(:root[data-theme="light"]) .env-scope { --env-c: var(--env); }

  .rt {
    display: grid;
    grid-template-rows: 1fr auto;
    max-width: 64rem;
    margin: 0 auto;
    color: var(--ink);
    font-variant-numeric: tabular-nums;
  }

  .rt-mono {
    font-family: ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, monospace;
    font-size: 0.92em;
  }

  .rt-muted { color: var(--ink-faint); }

  .rt-gutter {
    position: relative;
    display: flex;
    align-self: stretch;
    padding-left: var(--gutter-inset);
  }

  .rt-lane {
    position: relative;
    min-height: 100%;
    transition: width 260ms cubic-bezier(0.2, 0.8, 0.2, 1);
  }

  .rt-track {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    border-radius: 999px;
    background: color-mix(in oklab, var(--env-c) 12%, transparent);
    transition: top 620ms cubic-bezier(0.2, 0.8, 0.2, 1);
  }

  .rt-strand {
    position: absolute;
    top: 0;
    bottom: 0;
    transition:
      left 260ms cubic-bezier(0.2, 0.8, 0.2, 1),
      width 260ms cubic-bezier(0.2, 0.8, 0.2, 1);
  }

  .lane-run {
    position: absolute;
    left: 0;
    width: 100%;
    border-radius: 999px;
    overflow: hidden;
    transition:
      top 620ms cubic-bezier(0.2, 0.8, 0.2, 1),
      height 620ms cubic-bezier(0.2, 0.8, 0.2, 1);
  }

  .lane-run[data-layer="approach"] { z-index: 1; }
  .lane-run[data-layer="hold"] { z-index: 2; }
  .lane-run[data-layer="override"] { z-index: 3; }

  .lane-run[data-run="solid"] {
    background: var(--env-c);
  }

  .lane-run[data-run="travel"] {
    background: color-mix(in oklab, var(--env-c) 20%, var(--surface));
  }

  .lane-run[data-run="wait"],
  .lane-run[data-run="reverse"] {
    background: var(--sig-attn-bg);
  }

  .lane-run[data-run="fault"] {
    background: var(--sig-fail);
  }

  .lane-run[data-direction]::after {
    content: "";
    position: absolute;
    inset: -14px 0;
    -webkit-mask-image: var(--chevron);
    mask-image: var(--chevron);
    -webkit-mask-size: 100% 14px;
    mask-size: 100% 14px;
    -webkit-mask-repeat: repeat-y;
    mask-repeat: repeat-y;
    --chevron: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 14 14' preserveAspectRatio='none'%3E%3Cpath d='M1.5 9.5 L7 4 L12.5 9.5' fill='none' stroke='%23fff' stroke-width='2.6' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E");
  }

  .lane-run[data-direction="up"]::after {
    background: var(--env-c);
    opacity: 0.75;
  }

  .lane-run[data-direction="down"]::after {
    background: var(--sig-attn);
    transform: scaleY(-1);
  }

  .lane-run[data-run="wait"]::after {
    background: var(--sig-attn);
  }

  .lane-run[data-motion="march"]::after {
    animation: lane-march 1.15s linear infinite;
  }

  .lane-run[data-motion="breathe"]::after {
    animation: lane-breathe 2.1s ease-in-out infinite;
  }

  @keyframes lane-march {
    to {
      -webkit-mask-position: 0 -14px;
      mask-position: 0 -14px;
    }
  }

  .lane-dot {
    position: absolute;
    box-sizing: border-box;
    border-radius: 50%;
    z-index: 4;
    transition: top 620ms cubic-bezier(0.2, 0.8, 0.2, 1);
  }

  .lane-dot[data-kind="live"] {
    background: var(--env-c);
    border: 2px solid var(--surface);
    z-index: 6;
  }

  .lane-dot[data-kind="flight"] {
    background: var(--surface);
    border: 2px solid var(--env-c);
    z-index: 5;
  }

  .lane-dot[data-kind="awaiting"] {
    background: var(--sig-attn);
    border: 2px solid var(--surface);
    z-index: 6;
  }

  .lane-dot[data-kind="stopped"] {
    background: var(--sig-fail);
    border: 2px solid var(--surface);
    z-index: 6;
  }

  .lane-dot[data-tone="attention"] {
    background: var(--sig-attn);
    border-color: var(--surface);
  }

  .lane-dot[data-kind="pending"] {
    background: var(--surface);
    border: 1.5px dashed color-mix(in oklab, var(--env-c) 60%, transparent);
    opacity: 0.75;
  }

  .lane-dot[data-kind="past"] {
    background: var(--surface);
    border: 1.5px solid color-mix(in oklab, var(--env-c) 55%, transparent);
    opacity: 0.85;
  }

  .lane-pulse {
    animation: lane-breathe 2.1s ease-in-out infinite;
  }

  @keyframes lane-breathe {
    0%, 100% { opacity: 0.55; }
    50% { opacity: 1; }
  }

  .rt-lane-hit {
    position: absolute;
    inset: 0 -3px;
    z-index: 7;
    padding: 0;
    border: 0;
    background: transparent;
    cursor: pointer;
    border-radius: 999px;
  }

  .rt-lane-hit:focus-visible {
    outline: 2px solid var(--env-c);
    outline-offset: 2px;
  }

  .rt-lane:hover .rt-track {
    background: color-mix(in oklab, var(--env-c) 20%, transparent);
  }

  .rt-hovercard {
    position: absolute;
    left: calc(100% + 10px);
    z-index: 30;
    width: 17rem;
    padding: 10px 12px 11px;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: var(--surface);
    box-shadow: 0 8px 28px -8px rgb(0 0 0 / 0.28);
    pointer-events: none;
    font-size: 12px;
    line-height: 1.5;
  }

  .rt-hovercard-title {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0 0 7px;
    font-size: 13px;
    font-weight: 600;
    color: var(--ink);
  }

  .rt-hovercard-scope {
    margin-left: auto;
    font-size: 10.5px;
    font-weight: 500;
    color: var(--ink-faint);
  }

  .rt-hovercard-swatch {
    width: 9px;
    height: 9px;
    border-radius: 999px;
    background: var(--env-c);
    flex: none;
  }

  .rt-hovercard-facts {
    display: grid;
    grid-template-columns: 4.6rem minmax(0, 1fr);
    gap: 2px 8px;
    margin: 0;
  }

  .rt-hovercard-facts dt {
    color: var(--ink-faint);
  }

  .rt-hovercard-facts dd {
    margin: 0;
    color: var(--ink-soft);
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .rt-hovercard-facts dd[data-kind="live"] { color: var(--env-c); font-weight: 600; }
  .rt-hovercard-facts dd[data-kind="flight"] { color: var(--sig-run); font-weight: 600; }
  .rt-hovercard-facts dd[data-kind="awaiting"] { color: var(--sig-attn); font-weight: 600; }
  .rt-hovercard-facts dd[data-kind="stopped"] { color: var(--sig-fail); font-weight: 600; }

  .rt-hovercard-release {
    display: -webkit-box;
    -webkit-line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  .rt-hovercard-empty {
    margin: 0;
    color: var(--ink-faint);
  }

  .rt-labels {
    display: flex;
    padding-top: 8px;
    padding-left: var(--gutter-inset);
    min-height: 82px;
  }

  .rt-label-slot {
    position: relative;
    transition: width 260ms cubic-bezier(0.2, 0.8, 0.2, 1);
  }

  .rt-lane-label {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    writing-mode: vertical-rl;
    transform: rotate(180deg);
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.01em;
    line-height: 1;
    color: var(--env-c);
    white-space: nowrap;
    text-align: right;
  }

  .rt-lane-label-dest {
    right: auto;
    width: 8px;
    font-weight: 500;
    font-size: 9px;
    letter-spacing: 0;
    opacity: 0.85;
  }

  .rt-cards {
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-width: 0;
  }

  .rt-card {
    position: relative;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: var(--surface);
    overflow: hidden;
  }

  .rt-card-accented::before {
    content: "";
    position: absolute;
    top: 0;
    bottom: 0;
    left: 0;
    width: 2px;
    background: var(--env-c);
  }

  .rt-card-quiet {
    opacity: 0.72;
    background: var(--surface-sunken);
  }

  .rt-card-head {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
    padding: 11px 14px;
  }

  .rt-avatar {
    width: 24px;
    height: 24px;
    border-radius: 999px;
    object-fit: cover;
    background: var(--line-soft);
    flex: none;
  }

  .rt-avatar-initial {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-size: 10px;
    font-weight: 700;
    color: var(--ink-faint);
  }

  .rt-card-title {
    flex: 1 1 14rem;
    min-width: 0;
    font-size: 14px;
    font-weight: 550;
    color: var(--ink);
    text-decoration: none;
    letter-spacing: -0.006em;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .rt-card-title:hover { text-decoration: underline; }

  .rt-meta {
    display: flex;
    align-items: center;
    gap: 12px;
    flex: none;
    font-size: 11.5px;
    color: var(--ink-faint);
  }

  .rt-meta-item {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: inherit;
    text-decoration: none;
    white-space: nowrap;
  }

  a.rt-meta-item:hover { color: var(--ink-soft); text-decoration: underline; }

  .rt-meta-item svg {
    width: 12px;
    height: 12px;
    flex: none;
  }

  .rt-details { border-top: 1px solid var(--line-soft); }

  .rt-summary {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    padding: 8px 14px;
    font-size: 13px;
    cursor: pointer;
    list-style: none;
  }

  .rt-summary::-webkit-details-marker { display: none; }
  .rt-summary:hover { background: var(--surface-sunken); }

  .rt-summary-label { color: var(--ink-soft); }
  .rt-summary-label[data-signal="fail"] { color: var(--sig-fail); }
  .rt-summary-label[data-signal="attention"] { color: var(--sig-attn); }

  .rt-glyph {
    width: 13px;
    height: 13px;
    border-radius: 999px;
    flex: none;
    box-shadow: inset 0 0 0 2px var(--sig-idle);
  }

  .rt-glyph[data-signal="ok"] {
    background: var(--sig-ok);
    box-shadow: inset 0 0 0 2px var(--sig-ok);
  }
  .rt-glyph[data-signal="fail"] {
    background: var(--sig-fail);
    box-shadow: inset 0 0 0 2px var(--sig-fail);
  }
  .rt-glyph[data-signal="attention"] {
    box-shadow: inset 0 0 0 3px var(--sig-attn);
  }
  .rt-glyph[data-signal="running"] {
    box-shadow: inset 0 0 0 3px var(--sig-run);
    animation: lane-breathe 1.7s ease-in-out infinite;
  }
  .rt-glyph[data-signal="waiting"] {
    box-shadow: inset 0 0 0 2px var(--sig-run);
    opacity: 0.6;
  }
  .rt-glyph[data-signal="cancelled"] {
    box-shadow: inset 0 0 0 2px var(--sig-idle);
    opacity: 0.5;
  }

  .rt-progress {
    margin-left: auto;
    font-size: 11px;
    color: var(--ink-faint);
  }

  .rt-disclosure {
    margin-left: auto;
    display: inline-flex;
    padding-left: 10px;
    color: var(--ink-faint);
    transition: transform 180ms ease;
  }

  .rt-disclosure svg { width: 14px; height: 14px; }

  .rt-details[open] .rt-disclosure { transform: rotate(180deg); }

  .rt-chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 1px 8px;
    border-radius: 999px;
    font-size: 11.5px;
    font-weight: 550;
    line-height: 1.6;
    white-space: nowrap;
    color: var(--env-c);
    background: color-mix(in oklab, var(--env-c) 13%, transparent);
    box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--env-c) 25%, transparent);
  }

  .rt-chip-back {
    color: var(--sig-attn);
    background: color-mix(in oklab, var(--sig-attn) 14%, transparent);
    box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--sig-attn) 30%, transparent);
  }

  .rt-chip-attention {
    color: var(--sig-attn);
    background: color-mix(in oklab, var(--sig-attn) 14%, transparent);
    box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--sig-attn) 30%, transparent);
  }

  .rt-chip-mark {
    width: 5px;
    height: 5px;
    border-radius: 999px;
    background: currentColor;
    flex: none;
  }

  .rt-chip-mark[data-status="RUNNING"],
  .rt-chip-mark[data-status="ASSIGNED"] {
    width: 7px;
    height: 6px;
    border-radius: 1px;
    clip-path: polygon(50% 0, 100% 100%, 0 100%);
  }

  .rt-chip-mark[data-direction="reverse"] {
    clip-path: polygon(50% 100%, 100% 0, 0 0);
  }

  .rt-chip-mark[data-status="RUNNING"],
  .rt-chip-mark[data-status="ASSIGNED"] {
    animation: lane-breathe 1.6s ease-in-out infinite;
  }

  .rt-chip-mark[data-status="FAILED"],
  .rt-chip-mark[data-status="TIMED_OUT"] { background: var(--sig-fail); }

  .rt-chip-mark[data-status="PENDING"],
  .rt-chip-mark[data-status=""] {
    background: transparent;
    box-shadow: inset 0 0 0 1.5px currentColor;
  }

  .rt-chip-count {
    font-size: 10px;
    opacity: 0.75;
  }

  .rt-button {
    font: inherit;
    font-size: 11.5px;
    font-weight: 550;
    padding: 2px 9px;
    border-radius: 6px;
    border: 1px solid var(--line);
    background: var(--surface);
    color: var(--ink-soft);
    cursor: pointer;
  }

  .rt-button:hover { background: var(--surface-sunken); color: var(--ink); }
  .rt-button:disabled { opacity: 0.5; cursor: default; }

  .rt-button-go {
    border-color: transparent;
    background: var(--sig-ok);
    color: #fff;
  }
  .rt-button-go:hover { filter: brightness(0.94); background: var(--sig-ok); color: #fff; }

  .rt-button-warn {
    border-color: transparent;
    background: var(--sig-fail);
    color: #fff;
  }
  .rt-button-warn:hover { filter: brightness(0.94); background: var(--sig-fail); color: #fff; }

  .rt-body {
    border-top: 1px solid var(--line-soft);
  }

  .rt-description {
    margin: 0;
    padding: 11px 14px;
    font-size: 12.5px;
    line-height: 1.6;
    color: var(--ink-soft);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    max-height: 11rem;
    overflow: auto;
  }

  .rt-stages {
    margin: 0;
    padding: 0;
    list-style: none;
    border-top: 1px solid var(--line-soft);
  }

  .rt-stage {
    display: flex;
    align-items: center;
    gap: 9px;
    flex-wrap: wrap;
    padding: 6px 14px;
    font-size: 12.5px;
    color: var(--ink-soft);
  }

  .rt-stage + .rt-stage { border-top: 1px solid var(--line-soft); }

  .rt-stage-future { opacity: 0.45; }

  .rt-stage-label { color: inherit; }

  .rt-stage-elapsed {
    margin-left: auto;
    font-size: 11px;
    color: var(--ink-faint);
  }

  .rt-why {
    font-size: 11.5px;
    color: var(--sig-fail);
    text-decoration: none;
    border-bottom: 1px solid transparent;
    white-space: nowrap;
  }
  .rt-why:hover,
  .rt-why:focus-visible {
    border-bottom-color: currentColor;
  }

  .rt-destinations {
    flex-basis: 100%;
    margin: 3px 0 1px 6px;
    padding: 0 0 0 18px;
    list-style: none;
    border-left: 1px solid var(--line);
  }

  .rt-destinations-flat {
    flex-basis: auto;
    margin: 0;
    padding: 6px 14px;
    border-left: 0;
  }

  .rt-destination {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 1px 0;
    font-size: 11.5px;
    line-height: 1.7;
    color: var(--ink-faint);
  }

  .rt-dest-pip {
    width: 6px;
    height: 6px;
    border-radius: 999px;
    flex: none;
    box-shadow: inset 0 0 0 1.5px var(--ink-faint);
  }

  .rt-destination[data-kind="live"] .rt-dest-pip { background: var(--sig-ok); box-shadow: none; }
  .rt-destination[data-kind="flight"] .rt-dest-pip { background: var(--sig-run); box-shadow: none; animation: lane-breathe 1.6s ease-in-out infinite; }
  .rt-destination[data-kind="awaiting"] .rt-dest-pip { box-shadow: inset 0 0 0 2px var(--sig-attn); }
  .rt-destination[data-kind="stopped"] .rt-dest-pip { background: var(--sig-fail); box-shadow: none; }

  .rt-destination[data-kind="live"] .rt-dest-state { color: var(--sig-ok); }
  .rt-destination[data-kind="stopped"] .rt-dest-state { color: var(--sig-fail); }
  .rt-destination[data-kind="awaiting"] .rt-dest-state { color: var(--sig-attn); }

  .rt-dest-name { color: var(--ink-soft); }

  .rt-dest-time { margin-left: auto; }

  .rt-plan-output {
    padding: 10px 14px;
    background: var(--surface-sunken);
    border-top: 1px solid var(--line-soft);
    font-size: 12px;
  }

  .rt-plan-head {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 7px;
    color: var(--ink-faint);
  }

  .rt-plan-block-head {
    display: flex;
    gap: 8px;
    margin: 6px 0 4px;
    font-size: 11.5px;
    color: var(--ink-soft);
  }

  .rt-plan-output pre {
    margin: 0;
    padding: 9px 11px;
    max-height: 16rem;
    overflow: auto;
    border: 1px solid var(--line);
    border-radius: 6px;
    background: var(--surface);
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 11.5px;
    line-height: 1.55;
    color: var(--ink-soft);
    white-space: pre-wrap;
  }

  .rt-footnote {
    display: flex;
    gap: 10px;
    align-items: center;
    margin: 0;
    padding: 8px 14px;
    border-top: 1px solid var(--line-soft);
    font-size: 11px;
    color: var(--ink-faint);
  }

  .rt-version {
    padding: 1px 6px;
    border-radius: 999px;
    background: color-mix(in oklab, var(--sig-ok) 14%, transparent);
    color: var(--sig-ok);
  }

  .rt-hidden summary {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 2px 4px;
    font-size: 11.5px;
    color: var(--ink-faint);
    cursor: pointer;
    list-style: none;
  }

  .rt-hidden summary::-webkit-details-marker { display: none; }
  .rt-hidden summary:hover { color: var(--ink-soft); }

  .rt-hidden-rule {
    flex: 1;
    height: 1px;
    background: var(--line);
  }

  .rt-hidden-count { flex: none; }

  .rt-hidden-action {
    flex: none;
    text-decoration: underline;
    text-underline-offset: 2px;
  }

  .rt-hidden[open] .rt-hidden-action::after { content: " less"; }
  .rt-hidden:not([open]) .rt-hidden-action::after { content: " commits"; }

  .rt-hidden-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding-top: 8px;
  }

  .rt-more {
    padding-top: 10px;
  }

  .rt-more button {
    width: 100%;
    font: inherit;
    font-size: 12.5px;
    padding: 8px;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: transparent;
    color: var(--ink-faint);
    cursor: pointer;
  }

  .rt-more button:hover { color: var(--ink); border-color: var(--ink-faint); }

  .rt-alert {
    display: flex;
    align-items: center;
    gap: 8px;
    max-width: 64rem;
    margin: 0 auto 14px;
    padding: 10px 14px;
    border-radius: 8px;
    border: 1px solid color-mix(in oklab, #dc2626 35%, transparent);
    background: color-mix(in oklab, #dc2626 8%, transparent);
    color: #b91c1c;
    font-size: 13px;
  }

  .rt-alert svg { width: 16px; height: 16px; flex: none; }

  .rt-alert-close {
    margin-left: auto;
    border: 0;
    background: none;
    color: inherit;
    cursor: pointer;
    opacity: 0.6;
  }

  .rt-alert-close:hover { opacity: 1; }
  .rt-alert-close svg { width: 15px; height: 15px; }

  .rt-placeholder {
    max-width: 64rem;
    margin: 0 auto;
    padding: 44px 20px;
    text-align: center;
    color: var(--ink-faint);
    font-size: 13px;
    border: 1px solid var(--line);
    border-radius: 8px;
  }

  .rt-placeholder p { margin: 4px 0 0; }

  .rt-empty-title {
    color: var(--ink);
    font-size: 14px;
    font-weight: 600;
  }

  .rt-placeholder code {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    padding: 1px 5px;
    border-radius: 4px;
    background: var(--surface-sunken);
  }

  .rt-placeholder-error { border-color: color-mix(in oklab, #dc2626 35%, transparent); }
  .rt-placeholder-error p { color: var(--sig-fail); }

  .rt-link-button {
    margin-top: 8px;
    font: inherit;
    font-size: 12.5px;
    border: 0;
    background: none;
    color: var(--ink-soft);
    text-decoration: underline;
    cursor: pointer;
  }

  .rt-spinner {
    display: inline-block;
    width: 18px;
    height: 18px;
    border-radius: 999px;
    border: 2px solid var(--line);
    border-top-color: var(--ink-faint);
    animation: rt-spin 0.8s linear infinite;
  }

  @keyframes rt-spin { to { transform: rotate(360deg); } }

  @media (max-width: 40rem) {
    .rt-meta { flex-basis: 100%; gap: 10px; }
    .rt-card-title {
      flex: 1 1 0;
      white-space: normal;
      overflow: visible;
      text-overflow: clip;
    }
    .rt-hovercard { display: none; }
  }

  @media (prefers-reduced-motion: reduce) {
    .lane-run::after,
    .lane-pulse,
    .rt-glyph,
    .rt-chip-mark,
    .rt-dest-pip,
    .rt-spinner {
      animation: none !important;
    }
    .lane-run,
    .lane-dot,
    .rt-lane,
    .rt-strand,
    .rt-label-slot,
    .rt-disclosure {
      transition: none !important;
    }
  }
</style>
