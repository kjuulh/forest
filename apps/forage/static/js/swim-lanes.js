/**
 * <swim-lanes> web component
 *
 * Renders colored vertical bars alongside a release timeline.
 * Bars grow from the BOTTOM of the timeline upward to the dot position
 * (avatar center) of the relevant release card.
 *
 * In-flight deployments (QUEUED/RUNNING/ASSIGNED) show a hatched segment
 * with direction arrows: ▲ for forward deploy, ▼ for rollback.
 *
 * data-envs format: "env:STATUS,env:STATUS" e.g. "staging:SUCCEEDED,prod:QUEUED"
 */

const ENV_COLORS = {
  prod: ["#ec4899", "#fce7f3"],
  production: ["#ec4899", "#fce7f3"],
  preprod: ["#f97316", "#ffedd5"],
  "pre-prod": ["#f97316", "#ffedd5"],
  staging: ["#eab308", "#fef9c3"],
  stage: ["#eab308", "#fef9c3"],
  dev: ["#8b5cf6", "#ede9fe"],
  development: ["#8b5cf6", "#ede9fe"],
  test: ["#06b6d4", "#cffafe"],
};

const DEFAULT_COLORS = ["#6b7280", "#e5e7eb"];
const IN_FLIGHT = new Set(["QUEUED", "RUNNING", "ASSIGNED"]);
const DEPLOYED = new Set(["SUCCEEDED"]);

function envColors(name) {
  const lower = name.toLowerCase();
  if (ENV_COLORS[lower]) return ENV_COLORS[lower];
  for (const [key, colors] of Object.entries(ENV_COLORS)) {
    if (lower.includes(key)) return colors;
  }
  return DEFAULT_COLORS;
}

function parseEnvs(raw) {
  if (!raw) return [];
  return raw
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean)
    .map((entry) => {
      const colon = entry.indexOf(":");
      if (colon === -1) return { env: entry, status: "SUCCEEDED" };
      return { env: entry.slice(0, colon), status: entry.slice(colon + 1) };
    });
}

function dotY(card, timelineTop) {
  const avatar = card.querySelector("[data-avatar]");
  const anchor = avatar || card;
  const r = anchor.getBoundingClientRect();
  return r.top + r.height / 2 - timelineTop;
}

/** Create an inline SVG data URL for a diagonal hatch pattern */
function hatchPattern(color, bgColor) {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8">
    <rect width="8" height="8" fill="${bgColor}"/>
    <path d="M-2,2 l4,-4 M0,8 l8,-8 M6,10 l4,-4" stroke="${color}" stroke-width="1.5" opacity="0.6"/>
  </svg>`;
  return `url("data:image/svg+xml,${encodeURIComponent(svg)}")`;
}

// Inject CSS once
if (!document.getElementById("swim-lane-styles")) {
  const style = document.createElement("style");
  style.id = "swim-lane-styles";
  style.textContent = `
    @keyframes lane-pulse {
      0%, 100% { opacity: 0.6; }
      50% { opacity: 1; }
    }
    .lane-pulse {
      animation: lane-pulse 2s ease-in-out infinite;
    }
    .lane-arrow {
      font-size: 9px;
      line-height: 1;
      font-weight: 700;
      text-align: center;
      width: 100%;
      position: absolute;
      left: 0;
      z-index: 3;
      pointer-events: none;
    }
  `;
  document.head.appendChild(style);
}

const BAR_WIDTH = 20;
const BAR_GAP = 4;
const DOT_SIZE = 12;

class SwimLanes extends HTMLElement {
  connectedCallback() {
    // Lanes live in [data-swimlane-gutter], a CSS grid column to the
    // left of the timeline. The grid column width is pre-set in the
    // template (lane_count * 18 + 8 px) so there is no layout shift.
    requestAnimationFrame(() => {
      this._render();
      this._ro = new ResizeObserver(() => this._render());
      const timeline = this.querySelector("[data-swimlane-timeline]");
      if (timeline) {
        this._ro.observe(timeline);
        timeline.addEventListener("toggle", () => this._render(), true);
      }
    });
  }

  disconnectedCallback() {
    if (this._ro) this._ro.disconnect();
  }

  _render() {
    const timeline = this.querySelector("[data-swimlane-timeline]");
    if (!timeline) return;

    const cards = Array.from(timeline.querySelectorAll("[data-release]"));
    if (cards.length === 0) return;

    const timelineRect = timeline.getBoundingClientRect();
    if (timelineRect.height === 0) return;
    const gutter = this.querySelector("[data-swimlane-gutter]");
    const lanes = gutter
      ? Array.from(gutter.querySelectorAll("[data-lane]"))
      : Array.from(this.querySelectorAll("[data-lane]"));

    for (const lane of lanes) {
      const env = lane.dataset.lane;
      const [barColor, lightColor] = envColors(env);

      let deployedCard = null;
      let deployedIdx = -1;
      let flightCard = null;
      let flightIdx = -1;

      for (let i = 0; i < cards.length; i++) {
        const entries = parseEnvs(cards[i].dataset.envs);
        for (const entry of entries) {
          if (entry.env !== env) continue;
          if (DEPLOYED.has(entry.status) && !deployedCard) {
            deployedCard = cards[i];
            deployedIdx = i;
          }
          if (IN_FLIGHT.has(entry.status) && !flightCard) {
            flightCard = cards[i];
            flightIdx = i;
          }
        }
      }

      const timelineH = timelineRect.height;

      // Card top edge (Y relative to timeline) — bars extend to the card top
      const deployedTop = deployedCard
        ? deployedCard.getBoundingClientRect().top - timelineRect.top
        : null;
      const flightTop = flightCard
        ? flightCard.getBoundingClientRect().top - timelineRect.top
        : null;
      // Dot center Y — used for arrow placement
      const flightDot = flightCard
        ? dotY(flightCard, timelineRect.top)
        : null;

      // Solid bar: from bottom up to the card top of the LOWER card.
      // If both exist, only go to whichever is lower (further down) to avoid overlap.
      let solidBarFromBottom = 0;
      if (deployedTop !== null && flightTop !== null) {
        const lowerTop = Math.max(deployedTop, flightTop);
        solidBarFromBottom = timelineH - lowerTop;
      } else if (deployedTop !== null) {
        solidBarFromBottom = timelineH - deployedTop;
      }

      // Style lane container — width/gap only; height comes from the grid row
      lane.style.width = BAR_WIDTH + "px";
      lane.style.marginRight = BAR_GAP + "px";
      lane.style.position = "relative";

      const hasHatch = !!flightCard;
      const hasSolid = solidBarFromBottom > 0;
      const R = "9999px";

      // ── Solid bar ──
      let bar = lane.querySelector(".lane-bar");
      if (!bar) {
        bar = document.createElement("div");
        bar.className = "lane-bar";
        bar.style.position = "absolute";
        bar.style.bottom = "0";
        bar.style.left = "0";
        bar.style.width = "100%";
        lane.appendChild(bar);
      }
      bar.style.height = Math.max(solidBarFromBottom, 0) + "px";
      bar.style.backgroundColor = barColor;
      // Round bottom always; round top only if no hatch connects above
      bar.style.borderRadius = hasHatch
        ? `0 0 ${R} ${R}`
        : R;

      // ── Hatched segment for in-flight ──
      let hatch = lane.querySelector(".lane-hatch");
      let arrow = lane.querySelector(".lane-arrow");
      if (flightCard) {
        const isForward = deployedIdx === -1 || flightIdx < deployedIdx;

        // Hatched segment spans between the two card tops (or bottom of timeline)
        const anchorY = deployedTop !== null ? deployedTop : timelineH;
        const topY = Math.min(anchorY, flightTop);
        const bottomY = Math.max(anchorY, flightTop);
        const segHeight = bottomY - topY;

        if (!hatch) {
          hatch = document.createElement("div");
          hatch.className = "lane-hatch lane-pulse";
          hatch.style.position = "absolute";
          hatch.style.left = "0";
          hatch.style.width = "100%";
          hatch.style.backgroundSize = "8px 8px";
          hatch.style.backgroundRepeat = "repeat";
          lane.appendChild(hatch);
        }
        hatch.style.backgroundImage = isForward
          ? hatchPattern(barColor, lightColor)
          : hatchPattern("#f59e0b", "#fef3c7");
        hatch.style.top = topY + "px";
        hatch.style.height = Math.max(segHeight, 4) + "px";
        hatch.style.display = "";
        // Round top always; round bottom only if no solid bar connects below
        hatch.style.borderRadius = hasSolid
          ? `${R} ${R} 0 0`
          : R;

        // Direction arrow:
        // Forward (▲): shown at the in-flight card (destination)
        // Rollback (▼): shown at the deployed card (source we're rolling back from)
        const arrowDotY = isForward
          ? flightDot
          : dotY(deployedCard, timelineRect.top);
        if (!arrow) {
          arrow = document.createElement("div");
          arrow.className = "lane-arrow";
          lane.appendChild(arrow);
        }
        arrow.textContent = isForward ? "\u25B2" : "\u25BC";
        arrow.style.color = isForward ? barColor : "#f59e0b";
        arrow.style.top = arrowDotY - 5 + "px";
        arrow.style.display = "";
      } else {
        if (hatch) hatch.style.display = "none";
        if (arrow) arrow.style.display = "none";
      }

      // ── Dots ──
      // The arrow replaces the dot on one card:
      //   Forward: arrow on in-flight card (destination)
      //   Rollback: arrow on deployed card (source)
      const arrowCard = flightCard
        ? (deployedIdx === -1 || flightIdx < deployedIdx ? flightCard : deployedCard)
        : null;

      const existingDots = lane.querySelectorAll(".lane-dot");
      let dotIndex = 0;
      for (const card of cards) {
        const entries = parseEnvs(card.dataset.envs);
        const match = entries.find((e) => e.env === env);
        if (!match) continue;
        if (card === arrowCard) continue; // arrow shown instead of dot

        const cy = dotY(card, timelineRect.top);

        let dot = existingDots[dotIndex];
        if (!dot) {
          dot = document.createElement("div");
          dot.className = "lane-dot";
          dot.style.position = "absolute";
          dot.style.left = "50%";
          dot.style.transform = "translateX(-50%)";
          dot.style.width = DOT_SIZE + "px";
          dot.style.height = DOT_SIZE + "px";
          dot.style.borderRadius = "50%";
          dot.style.zIndex = "2";
          lane.appendChild(dot);
        }
        dot.style.top = cy - DOT_SIZE / 2 + "px";
        dot.style.backgroundColor = "#fff";
        dot.style.border = "2px solid " + barColor;
        dot.classList.remove("lane-pulse");
        dotIndex++;
      }
      for (let i = dotIndex; i < existingDots.length; i++) {
        existingDots[i].remove();
      }

      // Labels are rendered server-side above the gutter (no JS needed).
    }
  }
}

customElements.define("swim-lanes", SwimLanes);
