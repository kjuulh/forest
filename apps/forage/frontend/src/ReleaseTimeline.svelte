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

  // Props from attributes
  export let org = "";
  export let project = "";
  export let csrf = "";
  export let username = "";
  export let role = "";
  // Hard cap on how many timeline items to render, with no way to see
  // more. Used by the project Overview for its top-3 summary linked to
  // the full Releases tab. Empty string / "0" / missing → the full
  // views (Releases tab, org-wide releases), which instead reveal
  // PAGE_SIZE releases at a time behind a "Show more" control.
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
  let laneBarRaf = null;
  let laneBarScheduled = false;
  let laneBarRetryCount = 0;

  // Gutter metrics. A lane is a pill in a slot; fanned out, the slot holds one
  // thin strand per destination instead of one fat one.
  const LANE_W = 14;
  const LANE_GAP = 6;
  // Wide enough that a fanned strand's dots keep the same proportion of
  // clearance a collapsed lane gives them — see `dotSize`.
  const STRAND_W = 12;
  // Wide enough that a fanned lane's vertical labels sit beside each other
  // rather than on top of each other — the labels, not the strands, set this.
  const STRAND_GAP = 12;
  const GUTTER_INSET = 4;
  /** How far the lane's hit area reaches past the lane on each side. */
  const HIT_OVERHANG = 3;

  /**
   * How big a dot is, given the strand it sits on.
   *
   * Every dot has to fit *inside* its strand. A fixed size does not: the head
   * dot used to be as wide as a collapsed lane and nearly twice as wide as a
   * fanned strand, so it bulged out of the capsule and the lane read as a
   * lollipop rather than a rail with a marker on it.
   *
   * The states that mark where an environment *is* get the larger size; the
   * ones that are history or not-yet get the smaller one.
   */
  const HEAD_KINDS = new Set(["live", "stopped", "awaiting"]);

  function dotSize(kind, strandWidth) {
    const head = HEAD_KINDS.has(kind);
    // A proportion of the strand rather than a fixed inset, so the clearance
    // around a dot scales with the lane it sits in. Subtracting a constant left
    // only 2px of lane around a head dot: the dot all but filled the bar, the
    // rounded end became a thin arc hugging it, and the track showed through at
    // the shoulders where the arc had not yet reached full width.
    const size = Math.max(Math.round(strandWidth * (head ? 0.58 : 0.43)), head ? 5 : 4);
    // Same parity as the strand it sits in, so the margin either side is a
    // whole pixel. See `dotInset`.
    return (strandWidth - size) % 2 === 0 ? size : size + 1;
  }

  /**
   * Where a dot sits across its strand, in whole pixels.
   *
   * Not `left: 50%` with a `translateX(-50%)`, which is the obvious way and was
   * wrong: the runs are laid out as `left: 0; width: 100%`, and Firefox rounds
   * a percentage-positioned box differently from a full-width one, so the dot
   * landed half a pixel off the bar it is supposed to be centred on. Chrome
   * rounds both the same way and showed nothing — the bug was only ever visible
   * in one browser. An integer offset cannot disagree with anything.
   */
  function dotInset(size, strandWidth) {
    return (strandWidth - size) / 2;
  }

  /**
   * What the geometry needs to know about the size of things.
   *
   * `cap` is exactly the strand's radius, which puts the marker dot at the
   * cap's own centre of curvature — the dot and the rounded end become
   * concentric, so the clearance around the dot is the same at the sides as it
   * is above: `(strand - dot) / 2` in every direction.
   *
   * Anything larger pushes the dot below the centre of the arc and leaves a
   * visible gap above it that is not there at the sides. Anything smaller — or
   * a dot as wide as its strand, which is what it used to be — wraps the cap
   * tight around the dot and the lane ends in a map pin.
   */
  function metricsFor(strandWidth) {
    return { cap: strandWidth / 2, dot: dotSize("live", strandWidth) };
  }
  const MAX_LANE_BAR_RETRIES = 8;

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

  // ── The gutter ───────────────────────────────────────────────────
  //
  // Measuring is all this does. Where each release sits is a DOM question, and
  // what each lane should draw given those positions is not — that half lives
  // in lib/lane-geometry.js, where it can be tested. See
  // design/RELEASE-SWIMLANE.md for what the segments mean.

  // Debounce lane bar computation to one per frame. Waiting for Svelte's
  // flush before rAF keeps measurements out of first-paint zero-size races.
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

  /**
   * Where each release's row sits, in pixels from the top of the card column.
   * Anchored to the avatar so a dot lines up with the face that deployed it.
   */
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

      // The environment strand.
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

      // One strand per destination, resolved the same way a lane is. Computed
      // whether or not the lane is open: the collapsed lane needs to know
      // whether its destinations disagree in order to say so.
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

      // The road stops at the topmost marker on the lane, consuming it like any
      // other run. It used to span the whole list, so a lane with nothing left
      // to do still trailed a pale stub above its head — background where there
      // is no road. When the head *is* the topmost marker the track and the
      // hold now coincide, and there is nothing to see.
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

  /**
   * Promote the newest row of a broken environment from `past` to `stopped`.
   *
   * `releaseEnvStates` deliberately resolves a terminal failure to `past`, and
   * supersession relies on that meaning exactly one thing. But an environment
   * nothing has replaced since it broke has to look different from one that
   * simply moved on — see `releaseStoppedEnvs`.
   */
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

  // ── Fanning a lane out ───────────────────────────────────────────

  let expandedLanes = new Set();

  function toggleLane(env) {
    const next = new Set(expandedLanes);
    if (next.has(env)) next.delete(env);
    else next.add(env);
    expandedLanes = next;
    scheduleComputeLaneBars();
  }

  /**
   * Every lane's geometry, as one derived value.
   *
   * Deliberately a map rather than helper functions the template calls. Svelte
   * decides whether an attribute needs an update effect from what its
   * expression *mentions*, and a width helper taking only the lane name
   * mentions neither `expandedLanes` nor `destinationsByLane` — so the width
   * was computed once and a fanned-out lane kept its collapsed 14px forever.
   * A derived map is read directly, so it cannot go stale.
   */
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

  // ── Hover card ───────────────────────────────────────────────────

  let hovered = null; // { env, dest, row, top }

  /**
   * How close the pointer has to be to a dot to be asking about it.
   *
   * A lane is a column of bubbles, and "what is this environment doing" is the
   * wrong answer when the pointer is plainly on one of them — that reported the
   * lane's head no matter which bubble you were pointing at. Past this reach
   * there is no bubble in question and the lane summary is the right answer
   * again.
   */
  const HOVER_REACH = 22;

  function showLaneCard(env, event) {
    const bar = laneBarData[env];
    if (!bar) return;
    const rect = event.currentTarget.getBoundingClientRect();
    const gutter = event.currentTarget.closest(".rt-gutter")?.getBoundingClientRect();
    // Keyboard focus has no coordinates; it gets the lane summary. Tested on
    // the event type rather than on the coordinate being positive — a lane
    // scrolled to the top of the viewport has a perfectly good `clientY` of
    // zero or less, and treating that as "no pointer" silently fell back to
    // the lane summary for the bubbles nearest the top of the page.
    const pointed = event.type !== "focus" && typeof event.clientY === "number";

    // Which strand, when the lane is fanned out. The hit area spans the whole
    // bundle, so the pointer's x is what says which placement is being asked
    // about.
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

    // Which bubble.
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

    // `mousemove` fires continuously; only re-render when the answer changes.
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

  /**
   * What the hover card says: about one bubble if the pointer is on one,
   * otherwise about the lane as a whole.
   */
  function laneCardFacts(env, dest, row) {
    const bar = laneBarData[env];
    if (!bar) return null;
    const rows = dest ? bar.strands.find((s) => s.name === dest)?.rows || [] : bar.rows;
    const head = rows.find((r) => r.kind === "live");
    // A bubble answers for itself. Without this the card reported whatever the
    // lane was doing — usually the newest release — whichever bubble you were
    // actually pointing at.
    const subject = row || rows.find((r) => isUnfinished(r.kind) || r.kind === "stopped") || head;
    if (!subject) return { env, dest, empty: true };
    return {
      env,
      dest,
      empty: false,
      // Whether this is about a bubble or the lane changes what the reader
      // should take the status to mean.
      pinned: Boolean(row),
      kind: subject.kind,
      release: subject.release,
      head: head?.release || null,
      count: dest || row ? null : (destinationsByLane.get(env) || []).length,
    };
  }

  /**
   * A destination label with the environment it is in stripped off.
   *
   * Every placement in `prod` is called `prod-something`, and stacking three
   * labels that all start with the same six characters wastes the only axis a
   * vertical label has. The lane above them already says `prod`.
   */
  function shortDestination(env, name) {
    for (const sep of ["-", "_", "/", "."]) {
      const prefix = `${env}${sep}`;
      if (name.startsWith(prefix) && name.length > prefix.length) return name.slice(prefix.length);
    }
    return name;
  }

  /** Shared empty set, so a card without a rollback allocates nothing. */
  const EMPTY_SET = new Set();

  // The gutter answers "what is this environment doing", so its words are about
  // the environment.
  const KIND_WORDS = {
    live: "Live here",
    flight: "Deploying",
    awaiting: "Awaiting approval",
    pending: "Queued",
    stopped: "Failed",
    past: "Previously released",
  };

  // A destination row inside a card answers a different question — what did
  // *this release* do here — so it needs its own words. "Live here" under a
  // release from last week is true of the environment and false of the release.
  const DEST_WORDS = {
    live: "Deployed",
    flight: "Deploying",
    awaiting: "Waiting for approval",
    pending: "Not started",
    stopped: "Failed",
    past: "Deployed, since replaced",
  };

  /**
   * Is the destination breakdown worth showing under this environment's stage?
   *
   * One placement that went fine is already described by the stage row above
   * it, and repeating it is the kind of noise that stops people expanding
   * cards at all. More than one, or anything with something to say — an error,
   * a queue position — earns the space.
   */
  function showsDestinations(dests, release) {
    if (dests.length > 1) return true;
    return dests.some((d) => {
      const row = (release.destinations || []).find((x) => x.name === d.name);
      return d.kind === "stopped" || row?.error_message || row?.queue_position;
    });
  }


  /** Map the status module's icon names onto the three signal colours. */
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

  /** Destination rows for a release with no pipeline to hang them under. */
  function releaseDestinationRows(release) {
    return releaseDestinationStates(release);
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
    laneBarScheduled = false;
  });

  // Connect SSE after first data load
  $: if (!initialLoading && !error && org && !disconnectSSE) {
    disconnectSSE = connectSSE(org, project, handleEvent);
  }

  // Recompute lane bars on window resize (debounced via rAF)
  $: if (!initialLoading && renderedTimeline.length && laneCount > 0) scheduleComputeLaneBars();
  function handleResize() { scheduleComputeLaneBars(); }

  // ── Helpers for template ─────────────────────────────────────────

  // ── Deployer avatar ──────────────────────────────────────────────
  //
  // A release names whoever deployed it by username (`source_user`), and
  // `/avatars/` resolves that to the picture on their account — an upload, or
  // the one Google/GitHub handed us at sign-in. Anyone without a resolvable
  // picture falls back to their initial, so the slot is never a broken image.
  //
  // Whatever renders here must keep the `data-avatar` attribute:
  // `measureRows()` anchors each swim-lane dot to it.
  let avatarFailed = new Set();

  function avatarSrc(user) {
    return `/avatars/${encodeURIComponent(user)}`;
  }

  // A user with no picture 404s, and the browser would draw a broken image.
  // Remember the miss instead and render the initial from then on — the whole
  // list shares one entry per user, so one 404 settles every card they deployed.
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

  // Lanes to show. Driven by the same resolver as the dots, so a lane exists
  // exactly when something in it does — previously this read `dest_envs` and
  // dropped PENDING, which hid a pending deploy precisely when it mattered
  // most: an environment awaiting its first release had no lane to show it on.
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

  // Lane states for every rendered release, resolved *across* the list rather
  // than one release at a time — see `timelineEnvStates`. A release whose prod
  // leg is parked on approval, or failed, is superseded once a newer release is
  // live on prod, and the gutter has to say so; the card keeps its own history.
  //
  // Keyed by slug because the card loop walks timeline *items*, and a "hidden"
  // group is one item standing for many releases. Hidden releases are excluded
  // for the same reason they render `data-lane-states=""` today: they are
  // undeployed commits with no environments to supersede anything on.
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

  /**
   * The environment this release most recently reached, for the accent on the
   * left edge of its card. Ranked, so a release live on prod is marked by prod
   * even when it is also live on dev — the card should say the furthest it got.
   */
  function cardAccent(release) {
    const live = releaseEnvStates(release)
      .filter((s) => s.kind === "live")
      .sort((a, b) => envRank(a.env) - envRank(b.env));
    return live[0]?.env || null;
  }

  // ── Progressive reveal ───────────────────────────────────────────
  //
  // The full views start at the most recent PAGE_SIZE releases and grow
  // by PAGE_SIZE on each "Show more". The timeline is served whole (the
  // platform's artifact listing has no paging), so this is a pure
  // client-side reveal — nothing to fetch, nothing to wait for.

  const PAGE_SIZE = 20;
  let visibleReleases = PAGE_SIZE;

  // How many releases an item stands for. A "hidden" item collapses a
  // run of undeployed commits, so the window counts releases rather
  // than cards — 20 means 20 releases, not 20 disclosure widgets.
  function itemReleaseCount(item) {
    if (item.kind !== "hidden") return 1;
    return item.count ?? (item.releases || []).length;
  }

  // Take whole items until `target` releases are covered. A hidden
  // group straddling the boundary is taken whole rather than split —
  // splitting one would misreport its "N hidden commits" count.
  function takeReleases(items, target) {
    let count = 0;
    for (let i = 0; i < items.length; i++) {
      count += itemReleaseCount(items[i]);
      if (count >= target) return items.slice(0, i + 1);
    }
    return items;
  }

  // Grow from the releases actually on screen, not from the previous
  // target. Taking hidden groups whole means the window can overshoot
  // its target, and counting from the target would leave clicks that
  // reveal nothing — a single group of 50 would swallow two rounds.
  function showMore() {
    visibleReleases = renderedReleaseCount + PAGE_SIZE;
    // The card list just got taller; re-measure the swim lane bars
    // against the new height.
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

  // The releases the gutter draws against, newest first — the same list, in the
  // same order, that `timelineLaneStatesAttrs` resolved.
  $: visibleReleaseList = renderedTimeline
    .filter(i => i.kind === "release" && i.release)
    .map(i => i.release);
  $: destinationsByLane = timelineDestinations(visibleReleaseList);
  // Which environments each release is taking *backwards*. A shape in the
  // timeline rather than a flag on a release — see `timelineRollbacks`.
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

  // Production first, then back down the pipeline — see `orderLanes`.
  $: displayedLanes = orderLanes(lanes.filter(lane => renderedLaneNames.has(lane.name)));
  $: laneCount = displayedLanes.length;
  // +GUTTER_INSET matches the CSS custom property of the same name; the grid
  // column has to allow for the padding the gutter draws inside it.
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
    <!-- ── The gutter ───────────────────────────────────────────────
         One pill per environment, drawn from measured card positions.
         Click one to fan it into its destinations. -->
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
          <!-- The road. Runs from the topmost marker on this lane down to the
               bottom of the list — no further up, or a finished lane shows
               background above its head with nothing travelling on it. -->
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

          <!-- The hit area sits above the paint so hover and click work
               anywhere along the lane, including its empty upper stretch. -->
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
              <!-- Which question is being answered. Without it a card about one
                   bubble and a card about the whole lane look identical, and
                   the status line means different things in each. -->
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

    <!-- ── The cards ────────────────────────────────────────────────
         This element is what the lanes measure against, so nothing but
         release rows belongs inside it. -->
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
                <!-- What this release is doing right now, in one line. -->
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
                          <!-- How many placements this release actually reached.
                               Counting only the ones it still holds would read
                               0/2 on every superseded release, which says
                               nothing about how that release went. -->
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

                <!-- The stage ledger. Stages that have not happened yet are
                     listed in their place, dimmed: what is *going* to happen to
                     a release is as much a part of reading it as what already
                     did. -->
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
                          <!-- What it is parked on, in the server's own words. A
                               gate whose state you cannot see is worse than the
                               sleep it replaced, so this is the point of the
                               stage rendering rather than a nicety. -->
                          {#if isGateAwaiting(stage)}
                            {#each stage.gate_waiting_on as waiting}
                              <span class="rt-chip rt-chip-attention">{waiting}</span>
                            {/each}
                          {:else if stageStatus === "FAILED" && stage.error_message}
                            <span class="rt-stage-error">{stage.error_message}</span>
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

                        <!-- Destinations, nested under the environment stage
                             that carries them. An environment is a set of
                             placements, and a deploy that reached two of three
                             must not read the same as one that reached all
                             three — see design/RELEASE-SWIMLANE.md. -->
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
                                  <span class="rt-stage-error">{row.error_message}</span>
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
                  <!-- No pipeline: the destinations are the whole story. -->
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
          <!-- Commits that touched nothing this project deploys. Folded away by
               default: the point of the timeline is what moved. -->
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

    <!-- Show more (row 2, column 2). Deliberately outside the measured
         card list so revealing more doesn't skew the lane geometry. -->
    {#if hasMore}
      <div class="rt-more" style="grid-row: 2; grid-column: 2;">
        <button type="button" on:click={showMore}>Show more releases</button>
      </div>
    {/if}

    <!-- Lane labels (row 2, column 1) -->
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
            <!-- The count is how a collapsed lane says it has something to
                 open. It used to be a hairline down the middle of the bar,
                 which read as a rendering seam rather than a signal. -->
            <span class="rt-lane-label">{lane.name}{L.fans ? ` ${L.dests.length}` : ""}</span>
          {/if}
        </div>
      {/each}
    </div>
  </div>
{/if}

<style>
  /* ── Tokens ─────────────────────────────────────────────────────────────
     Neutrals come from the app's own palette. Tailwind emits `--color-*` and
     input.css remaps them under `prefers-color-scheme: dark`, so borrowing
     them means the timeline follows the app into dark mode for free — and,
     more to the point, cannot drift out of step with it. Restating the greys
     here is how a component ends up a shade darker than the page it sits on.

     Signal colours are the timeline's own, because the app has no opinion
     about them. Warm is reserved: amber means a person is needed, red means it
     broke, and no environment is ever allowed either. See src/lib/colors.js. */
  .rt {
    /* Room for the vertical lane labels, whose glyphs sit a little to the left
       of the strand they name. Without it the first label is clipped. */
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

  /* The design gallery sets `data-theme` so dark mode can be switched without
     touching an OS setting; these come after the media query so the explicit
     choice wins in both directions. */
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

  /* An environment's colour arrives as a light/dark pair on the element that
     needs it; this picks one. Declared on the elements that *set* the pair,
     because a custom property that references another is resolved where it is
     declared, not where it is used. */
  .env-scope { --env-c: var(--env); }
  @media (prefers-color-scheme: dark) {
    .env-scope { --env-c: var(--env-dark); }
  }
  :global(:root[data-theme="dark"]) .env-scope { --env-c: var(--env-dark); }
  :global(:root[data-theme="light"]) .env-scope { --env-c: var(--env); }

  /* ── Frame ─────────────────────────────────────────────────────────────── */

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

  /* ── Gutter ────────────────────────────────────────────────────────────── */

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

  /* The road the lane travels, drawn whether or not anything is on it. */
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

  /* ── Runs ───────────────────────────────────────────────────────────────
     One element per run, and the layer it belongs to decides what paints over
     what. Every run is a pill: the layer beneath always covers its ends, so a
     rounded end can only ever reveal the track or the hold, never the page.
     See lib/lane-geometry.js. */
  .lane-run {
    position: absolute;
    left: 0;
    width: 100%;
    border-radius: 999px;
    overflow: hidden;
    /* The one orchestrated moment: when a deploy lands, the run grows into its
       new extent rather than blinking there. */
    transition:
      top 620ms cubic-bezier(0.2, 0.8, 0.2, 1),
      height 620ms cubic-bezier(0.2, 0.8, 0.2, 1);
  }

  .lane-run[data-layer="approach"] { z-index: 1; }
  .lane-run[data-layer="hold"] { z-index: 2; }
  .lane-run[data-layer="override"] { z-index: 3; }

  /* What the environment is running, and its history below. */
  .lane-run[data-run="solid"] {
    background: var(--env-c);
  }

  /* On its way up. Tinted rather than solid — it is not here yet. */
  .lane-run[data-run="travel"] {
    background: color-mix(in oklab, var(--env-c) 20%, var(--surface));
  }

  /* Parked on a person, and a rollback: the same yellow, because both are
     states somebody has to know about rather than states the pipeline is
     quietly working through. */
  .lane-run[data-run="wait"],
  .lane-run[data-run="reverse"] {
    background: var(--sig-attn-bg);
  }

  /* A deploy failed here and nothing has replaced it since. */
  .lane-run[data-run="fault"] {
    background: var(--sig-fail);
  }

  /* ── Chevrons ───────────────────────────────────────────────────────────
     A run that is going somewhere says which way, in the run's own colour. */
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

  /* Down is always the attention colour, never an environment's: a rollback
     must not be mistakable for a deploy at a glance. */
  .lane-run[data-direction="down"]::after {
    background: var(--sig-attn);
    transform: scaleY(-1);
  }

  .lane-run[data-run="wait"]::after {
    background: var(--sig-attn);
  }

  /* Moving, versus parked and waiting on somebody. Both animate: a pipeline
     that needs a person looking exactly like a finished one is the bug this
     component was written to prevent. */
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

  /* Dots are rings, not shadows, so `pending` can be dashed — the one state
     that has to look provisional. `border-box` keeps every ring the same
     outside diameter whatever its width, and nothing is allowed to paint
     outside that diameter: a dot wider than its strand turns the lane into a
     lollipop. Size comes from `dotSize`. */
  .lane-dot {
    position: absolute;
    /* `left` is set per dot, in whole pixels — see `dotInset`. */
    box-sizing: border-box;
    border-radius: 50%;
    z-index: 4;
    transition: top 620ms cubic-bezier(0.2, 0.8, 0.2, 1);
  }

  /* Where the environment is right now. A bullseye punched out of the bar:
     the ring is the page colour, so it reads as a hole in the trail rather
     than as something sitting on top of it. */
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

  /* Where a rollback is heading. It sits inside the yellow run, so it takes
     the yellow with it: the destination is part of the rollback, not an
     ordinary deploy that happens to be underneath one. */
  .lane-dot[data-tone="attention"] {
    background: var(--sig-attn);
    border-color: var(--surface);
  }

  /* Headed here, nothing started. Dashed: provisional, and unmistakable for
     `past` — which is the same shape but did actually happen. */
  .lane-dot[data-kind="pending"] {
    background: var(--surface);
    border: 1.5px dashed color-mix(in oklab, var(--env-c) 60%, transparent);
    opacity: 0.75;
  }

  /* The environment held this release once. A solid ring, because it happened;
     quiet, because there are a lot of them and they are history. */
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
    /* Matches HIT_OVERHANG, which showLaneCard subtracts to map the pointer
       back onto the lane's own coordinates. */
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

  /* ── Hover card ────────────────────────────────────────────────────────── */

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

  /* ── Lane labels ───────────────────────────────────────────────────────── */

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

  /* Sized to its strand, not to its text: a negative margin to centre the
     glyphs pushed the leftmost label off the edge of the page. */
  .rt-lane-label-dest {
    right: auto;
    width: 8px;
    font-weight: 500;
    font-size: 9px;
    letter-spacing: 0;
    opacity: 0.85;
  }

  /* ── Cards ─────────────────────────────────────────────────────────────── */

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

  /* The card carries the colour of the furthest environment it reached, as a
     seam down its left edge. It is the one thing tying a row in the list to a
     lane in the gutter without drawing a line across the page. */
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

  /* ── Summary line ──────────────────────────────────────────────────────── */

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

  /* One glyph vocabulary for every status in the component: a filled ring for
     done, a hollow one for not started, an amber ring for you. */
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

  /* Pushed to the far end, away from the chip's destination count: two
     fractions sitting next to each other read as one muddled number. */
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

  /* ── Chips ─────────────────────────────────────────────────────────────── */

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

  /* A chip for an environment being taken backwards wears the attention colour,
     not its environment's. Where it is going matters less than which way. */
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

  /* A moving deploy points where it is going. Same convention as the lane it
     belongs to, so the chip and the gutter never disagree about direction. */
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

  /* ── Buttons ───────────────────────────────────────────────────────────── */

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

  /* ── Expanded body ─────────────────────────────────────────────────────── */

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

  /* What has not happened yet, in its place in the order. */
  .rt-stage-future { opacity: 0.45; }

  .rt-stage-label { color: inherit; }

  .rt-stage-elapsed {
    margin-left: auto;
    font-size: 11px;
    color: var(--ink-faint);
  }

  .rt-stage-error {
    font-size: 11.5px;
    color: var(--sig-fail);
  }

  /* ── Destinations ──────────────────────────────────────────────────────── */

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

  /* ── Plan output ───────────────────────────────────────────────────────── */

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

  /* ── Hidden commits ────────────────────────────────────────────────────── */

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

  /* ── Chrome ────────────────────────────────────────────────────────────── */

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

  /* ── Narrow ────────────────────────────────────────────────────────────── */

  @media (max-width: 40rem) {
    /* The meta row drops below; the title stays beside the avatar and wraps
       inside its own box, rather than being pushed onto a second line and
       leaving the avatar sitting alone above it. */
    .rt-meta { flex-basis: 100%; gap: 10px; }
    .rt-card-title {
      flex: 1 1 0;
      white-space: normal;
      overflow: visible;
      text-overflow: clip;
    }
    /* A hover card needs a pointer and somewhere to sit. There is neither
       here; the dots keep their titles, and the card itself says the rest. */
    .rt-hovercard { display: none; }
  }

  /* ── Reduced motion ────────────────────────────────────────────────────── */

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
