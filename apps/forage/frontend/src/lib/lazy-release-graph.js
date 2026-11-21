import { effectiveStatus, IN_FLIGHT, STOPPED } from "./lane-states.js";
import { createTimelineScheduler } from "./timeline-scheduler.js";

export function releaseGraphActive(release) {
  if (!release.has_pipeline || !release.release_intent_id) return false;
  const stages = release.pipeline_stages || [];
  if (stages.some(stage => IN_FLIGHT.has(effectiveStatus(stage)) ||
      ["AWAITING_APPROVAL", "AWAITING_SIGNAL"].includes(effectiveStatus(stage)))) return true;
  if (IN_FLIGHT.has(release.summary_status)) return true;
  return !STOPPED.has(release.summary_status) &&
    !stages.some(stage => STOPPED.has(effectiveStatus(stage))) &&
    stages.some(stage => effectiveStatus(stage) === "PENDING");
}

export function releaseGraphUrl(org, project, release) {
  return `/orgs/${encodeURIComponent(org)}/projects/${encodeURIComponent(release.project_name || project)}/releases/${encodeURIComponent(release.slug)}/graph`;
}

export function createLazyReleaseGraph(url, onChange, { fetchGraph = fetch } = {}) {
  let html = "";
  let expanded = false;
  let active = false;
  let revision;
  let session = 0;
  let scheduler = null;

  function reset() {
    session += 1;
    scheduler?.destroy();
    scheduler = null;
    html = "";
    onChange({ html, error: null });
  }

  return {
    update(next) {
      if (!next.expanded) {
        if (expanded) reset();
        expanded = false;
        active = next.active;
        revision = next.revision;
        return;
      }
      const opened = !expanded;
      const completed = active && !next.active;
      const changed = revision !== next.revision;
      expanded = true;
      active = next.active;
      revision = next.revision;
      let created = false;
      if (!scheduler && active) {
        const currentSession = session;
        created = true;
        scheduler = createTimelineScheduler(async signal => {
          const response = await fetchGraph(url, {
            credentials: "same-origin", headers: { Accept: "text/html" }, signal, cache: "no-store", redirect: "error",
          });
          if (!response.ok) throw new Error(`Pipeline graph fetch failed: ${response.status}`);
          const nextHTML = await response.text();
          if (signal.aborted || currentSession !== session) return;
          html = nextHTML;
          onChange({ html, error: null });
        }, {
          visible: next.visible,
          onError: error => onChange({ html, error: error.message }),
        });
      }
      scheduler?.setVisible(next.visible);
      if (scheduler && (created || (active && (opened || changed)) || completed)) scheduler.request();
    },
    destroy: reset,
  };
}
