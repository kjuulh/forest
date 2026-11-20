const UNFINISHED = new Set(["flight", "awaiting"]);

const MIN_RUN = 6;

export function laneGeometry(rows, height, metrics = {}) {
  const cap = metrics.cap ?? 0;

  const marked = (rows || []).filter((r) => r && r.kind);

  const headIdx = marked.findIndex((r) => r.kind === "live");
  const head = headIdx === -1 ? null : marked[headIdx].y;

  const holdIdx = headIdx !== -1 ? headIdx : marked.findIndex((r) => r.kind === "past");
  const holdY = holdIdx === -1 ? null : marked[holdIdx].y;

  const movingIdx = marked.findIndex((r) => UNFINISHED.has(r.kind));
  const moving = movingIdx === -1 ? null : marked[movingIdx];
  const forward = moving ? head === null || movingIdx < headIdx : false;
  const waiting = moving ? moving.kind === "awaiting" : false;

  const faultIdx = marked.findIndex((r) => r.kind === "stopped");
  const fault =
    faultIdx !== -1 && (holdIdx === -1 || faultIdx < holdIdx) ? marked[faultIdx] : null;

  const runs = [];
  const add = (run) => {
    runs.push({
      kind: run.kind,
      layer: run.layer,
      top: Math.max(run.top, 0),
      height: Math.max(run.bottom - Math.max(run.top, 0), MIN_RUN),
      direction: run.direction ?? null,
      motion: run.motion ?? null,
    });
  };

  const approachBottom = holdY !== null ? holdY + cap : null;

  if (fault) {
    add({
      kind: "fault",
      layer: "approach",
      top: fault.y - cap,
      bottom: approachBottom ?? fault.y + MIN_RUN,
    });
  }

  if (moving && forward) {
    add({
      kind: waiting ? "wait" : "travel",
      layer: "approach",
      top: moving.y - cap,
      bottom: approachBottom ?? height,
      direction: "up",
      motion: waiting ? "breathe" : "march",
    });
  }

  if (holdY !== null) {
    add({ kind: "solid", layer: "hold", top: holdY - cap, bottom: height });
  }

  if (moving && !forward && holdY !== null) {
    add({
      kind: waiting ? "wait" : "reverse",
      layer: "override",
      top: holdY - cap,
      bottom: Math.min(moving.y + cap, height),
      direction: "down",
      motion: waiting ? "breathe" : "march",
    });
  }

  const rollbackTarget = moving && !forward && holdY !== null ? moving : null;
  const dots = marked.map((r) => ({ ...r, tone: r === rollbackTarget ? "attention" : null }));

  return { runs, head, dots };
}
