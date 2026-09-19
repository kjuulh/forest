/**
 * The shape of one lane.
 *
 * A lane is **a road with things painted on it**, in four layers. Everything
 * else falls out of that, which is the point: the rules below are general, and
 * no state needs a special case.
 *
 *   0. **track**    the road itself — the full height of the list, faint,
 *                   always there. Nothing else ever has to reach the page.
 *   1. **approach** what is on its way in, above where the lane sits now:
 *                   a deploy travelling up, a stage parked on a person, a
 *                   failure nothing has replaced.
 *   2. **hold**     what the environment is running, and the history below it.
 *   3. **override** what is leaving: a rollback travelling back down through
 *                   ground the lane still holds.
 *
 * Two rules govern every extent, and between them they replace the seam-
 * patching this file used to do:
 *
 *   **A run goes from one dot to another and consumes both.** It reaches `cap`
 *   past the marker at each end, never stopping on one — a boundary that lands
 *   on a dot cuts it in half and the dot reads as sitting crooked on the lane.
 *   `cap` is the strand's radius, which puts the dot at the centre of curvature
 *   of the rounded end: the two are concentric, so the clearance around a dot
 *   is the same above as it is at the sides.
 *
 *   Which colour a viewer sees at each end is then a question of layer, not of
 *   extent. An approach run is painted *under* the hold, so at the end it comes
 *   from the environment's own colour wins and the head dot sits on solid. A
 *   rollback is painted *over* it, so it wins at both ends — the stretch it is
 *   travelling, and the release it is leaving.
 *
 *   **A run never has to butt against another.** Every run is a pill, and the
 *   layer beneath it always covers its ends — the track is under everything,
 *   and an approach run continues past where the hold begins. That is why there
 *   are no square joins here and no overlap fudge: a rounded end can only ever
 *   reveal the layer below, never the page.
 *
 * Coordinates are pixels from the top of the timeline. Rows arrive newest
 * first, which is the order the cards are painted in.
 */

/** Lane states that mean the release has not finished arriving. */
const UNFINISHED = new Set(["flight", "awaiting"]);

/** A run shorter than this is invisible; floor it so it still reads. */
const MIN_RUN = 6;

/**
 * @param {Array<{y: number, kind: string}>} rows newest first; `y` is the row's
 *   anchor, `kind` one of live / flight / awaiting / pending / past / stopped
 * @param {number} height full height of the timeline
 * @param {{cap?: number, dot?: number}} [metrics] `cap` is how far a run
 *   reaches above the dot it marks; `dot` is the marker diameter
 * @returns {{
 *   runs: Array<{kind: string, layer: string, top: number, height: number,
 *                direction: "up"|"down"|null, motion: "march"|"breathe"|null}>,
 *   head: number | null,
 *   dots: Array<{y: number, kind: string, tone: string|null}>,
 * }}
 */
export function laneGeometry(rows, height, metrics = {}) {
  const cap = metrics.cap ?? 0;

  const marked = (rows || []).filter((r) => r && r.kind);

  const headIdx = marked.findIndex((r) => r.kind === "live");
  const head = headIdx === -1 ? null : marked[headIdx].y;

  // What the lane holds, and therefore where its solid run starts. `live` now,
  // or `past` — it held this release once and something newer has taken over.
  // A `stopped` row must not anchor it: a placement whose only row was a failed
  // deploy would draw a full-height solid bar, i.e. the healthiest thing on the
  // page.
  const holdIdx = headIdx !== -1 ? headIdx : marked.findIndex((r) => r.kind === "past");
  const holdY = holdIdx === -1 ? null : marked[holdIdx].y;

  const movingIdx = marked.findIndex((r) => UNFINISHED.has(r.kind));
  const moving = movingIdx === -1 ? null : marked[movingIdx];
  // A pipeline moving *up* the list is heading for a newer commit — a deploy.
  // Moving down is a rollback, and the two must never look alike.
  const forward = moving ? head === null || movingIdx < headIdx : false;
  // Parked on a person: the same stretch of road, but nothing is happening.
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

  // ── 1. approach ─────────────────────────────────────────────────────────
  //
  // Each of these consumes the hold's dot as well as its own, and is painted
  // under the hold — so the hold's colour is what shows there, and its rounded
  // cap lands on something rather than on the page. With no hold below, a
  // travelling deploy is filling the whole lane, while a failure is a mark at a
  // point rather than a stretch of road.
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

  // ── 2. hold ─────────────────────────────────────────────────────────────
  if (holdY !== null) {
    add({ kind: "solid", layer: "hold", top: holdY - cap, bottom: height });
  }

  // ── 3. override ─────────────────────────────────────────────────────────
  //
  // A rollback consumes both of its dots — the one marking where the lane is
  // now, and the release it is heading back to — and is painted over the hold,
  // so the whole stretch between them reads as the rollback rather than as the
  // environment with a band laid across it.
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

  // The dot a rollback is heading for wears the colour of the run it sits in,
  // so the destination reads as part of the rollback rather than as an ordinary
  // deploy that happens to be below it.
  const rollbackTarget = moving && !forward && holdY !== null ? moving : null;
  const dots = marked.map((r) => ({ ...r, tone: r === rollbackTarget ? "attention" : null }));

  return { runs, head, dots };
}
