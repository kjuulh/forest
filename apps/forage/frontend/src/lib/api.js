export async function fetchTimeline(org, project, options = {}) {
  const url = timelineUrl(org, project, options);

  const primed = globalThis.__forestTimeline?.[url];
  if (primed) {
    delete globalThis.__forestTimeline[url];
    const data = await primed;
    if (data) return data;
  }

  const res = await fetch(url, {
    credentials: "same-origin",
    ...(options.signal ? { signal: options.signal } : {}),
  });
  if (!res.ok) throw new Error(`Timeline fetch failed: ${res.status}`);
  return res.json();
}

export function timelineUrl(org, project, { includeGraph = false } = {}) {
  const url = project
    ? `/api/orgs/${org}/projects/${project}/timeline`
    : `/api/orgs/${org}/timeline`;
  return includeGraph ? `${url}?include_graph=true` : url;
}

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

export function formatElapsed(seconds) {
  if (seconds < 0) seconds = 0;
  if (seconds < 60) return `${seconds}s`;
  const m = Math.floor(seconds / 60);
  const s = seconds % 60;
  if (m < 60) return `${m}m ${s}s`;
  const h = Math.floor(m / 60);
  return `${h}h ${m % 60}m`;
}

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
