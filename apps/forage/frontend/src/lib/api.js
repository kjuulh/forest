/**
 * Fetch timeline data from the JSON API.
 * @param {string} org
 * @param {string} project
 * @returns {Promise<{timeline: Array, lanes: Array}>}
 */
export async function fetchTimeline(org, project) {
  const url = timelineUrl(org, project);

  // The page may have started this request during HTML parse, so that it runs
  // alongside the bundle download instead of after it — see
  // templates/components/timeline_prefetch.html.jinja.
  //
  // Consumed once and then dropped: every later call is a refresh driven by a
  // live event, and handing those a promise resolved before the page was even
  // interactive would pin the timeline to its first state forever.
  const primed = globalThis.__forestTimeline?.[url];
  if (primed) {
    delete globalThis.__forestTimeline[url];
    const data = await primed;
    // `null` means the prefetch failed. Fall through rather than reporting it:
    // the request below produces the real error, so there is one place that
    // decides what a failed timeline looks like.
    if (data) return data;
  }

  const res = await fetch(url, {
    credentials: "same-origin",
  });
  if (!res.ok) throw new Error(`Timeline fetch failed: ${res.status}`);
  return res.json();
}

/**
 * Where a timeline lives. Shared with the prefetch in the page, which has to
 * produce a byte-identical URL for the handoff to be found.
 */
export function timelineUrl(org, project) {
  return project
    ? `/api/orgs/${org}/projects/${project}/timeline`
    : `/api/orgs/${org}/timeline`;
}

/**
 * Connect to SSE endpoint for live updates.
 * Returns a disconnect function.
 * @param {string} org
 * @param {string} project
 * @param {(type: string, data: object) => void} onEvent
 * @returns {() => void} disconnect
 */
export function connectSSE(org, project, onEvent) {
  const url = project
    ? `/orgs/${org}/projects/${project}/events`
    : `/orgs/${org}/events`;
  let retryDelay = 1000;
  let es = null;
  let stopped = false;

  function connect() {
    if (stopped) return;
    es = new EventSource(url);

    es.addEventListener("open", () => {
      retryDelay = 1000;
    });

    for (const type of ["destination", "release", "artifact", "pipeline"]) {
      es.addEventListener(type, (e) => {
        try {
          const data = JSON.parse(e.data);
          onEvent(type, data);
        } catch (err) {
          console.warn(`[release-timeline] bad ${type} event:`, err);
        }
      });
    }

    es.addEventListener("error", () => {
      es.close();
      if (!stopped) {
        setTimeout(connect, retryDelay);
        retryDelay = Math.min(retryDelay * 2, 30000);
      }
    });
  }

  connect();

  return () => {
    stopped = true;
    if (es) es.close();
  };
}

/**
 * Format elapsed time from seconds.
 */
export function formatElapsed(seconds) {
  if (seconds < 0) seconds = 0;
  if (seconds < 60) return `${seconds}s`;
  const m = Math.floor(seconds / 60);
  const s = seconds % 60;
  if (m < 60) return `${m}m ${s}s`;
  const h = Math.floor(m / 60);
  return `${h}h ${m % 60}m`;
}

/**
 * Format a relative timestamp.
 */
export function timeAgo(dateStr) {
  if (!dateStr) return "";
  const date = new Date(dateStr);
  const now = Date.now();
  const diff = Math.floor((now - date.getTime()) / 1000);
  if (diff < 10) return "just now";
  if (diff < 60) return `${diff}s ago`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  return `${Math.floor(diff / 86400)}d ago`;
}
