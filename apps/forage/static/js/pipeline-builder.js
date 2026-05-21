/**
 * <pipeline-builder> web component
 *
 * Interactive node-graph editor for release pipeline stages.
 * Syncs to a hidden textarea (data-target) as JSON.
 *
 * Stage format (matches Rust serde of PipelineStage):
 *   { "id": "stage-name", "depends_on": ["other"], "config": {"Deploy": {"environment": "prod"}} }
 *
 * Usage:
 *   <pipeline-builder data-target="pipeline-stages"></pipeline-builder>
 *   <textarea id="pipeline-stages" name="stages_json" hidden></textarea>
 *
 * Optional attributes:
 *   data-readonly="true"   render-only mode (no editing)
 */

const NODE_W = 200;
const NODE_H = 68;
const GRID = 20;
const SVG_HALF = 6000;          // SVG covers [-6000, +6000] in canvas coords
const TYPES = ["deploy", "wait", "plan"];

// Lucide-style icons (16x16, stroke="currentColor"). Inline SVG paths.
const ICONS = {
  deploy: '<path d="M4.5 16.5c-1.5 1.26-2 5-2 5s3.74-.5 5-2c.71-.84.7-2.13-.09-2.91a2.18 2.18 0 0 0-2.91-.09z"/><path d="M12 15l-3-3a22 22 0 0 1 2-3.95A12.88 12.88 0 0 1 22 2c0 2.72-.78 7.5-6 11a22.35 22.35 0 0 1-4 2z"/><path d="M9 12H4s.55-3.03 2-4c1.62-1.08 5 0 5 0"/><path d="M12 15v5s3.03-.55 4-2c1.08-1.62 0-5 0-5"/>',
  wait:   '<circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/>',
  plan:   '<polyline points="3 6 5 8 9 4"/><polyline points="3 12 5 14 9 10"/><polyline points="3 18 5 20 9 16"/><line x1="13" y1="6" x2="21" y2="6"/><line x1="13" y1="12" x2="21" y2="12"/><line x1="13" y1="18" x2="21" y2="18"/>',
  plus:   '<line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/>',
  minus:  '<line x1="5" y1="12" x2="19" y2="12"/>',
  fit:    '<polyline points="9 3 3 3 3 9"/><polyline points="15 3 21 3 21 9"/><polyline points="9 21 3 21 3 15"/><polyline points="15 21 21 21 21 15"/>',
  reset:  '<polyline points="1 4 1 10 7 10"/><path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10"/>',
  layout: '<path d="M3 3h18v18H3z"/><line x1="3" y1="9" x2="21" y2="9"/><line x1="9" y1="21" x2="9" y2="9"/>',
  trash:  '<polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/><path d="M10 11v6M14 11v6"/><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/>',
  warn:   '<path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"/><line x1="12" y1="9" x2="12" y2="13"/><line x1="12" y1="17" x2="12.01" y2="17"/>',
  close:  '<line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>',
};

// Type theme. Accent is a saturated color that reads on both light and dark
// surfaces; tint is a very subtle wash used for the icon background.
const TYPE_STYLE = {
  deploy: { label: "Deploy", accent: "#3b82f6", tint: "rgba(59,130,246,0.10)", icon: ICONS.deploy },
  wait:   { label: "Wait",   accent: "#f59e0b", tint: "rgba(245,158,11,0.12)", icon: ICONS.wait },
  plan:   { label: "Plan",   accent: "#8b5cf6", tint: "rgba(139,92,246,0.12)", icon: ICONS.plan },
};

function svgIcon(name, size = 16, strokeWidth = 2) {
  const path = typeof name === "string" && ICONS[name] ? ICONS[name] : name;
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("width", String(size));
  svg.setAttribute("height", String(size));
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", String(strokeWidth));
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.style.flexShrink = "0";
  svg.innerHTML = path;
  return svg;
}

// Tracks which editor most recently received user interaction. Keyboard
// shortcuts (Delete) only fire on the active editor so multiple builders
// on the same page can't interfere with each other.
let activeEditor = null;

class PipelineBuilder extends HTMLElement {
  connectedCallback() {
    this.stages = [];
    this.positions = {};            // { id: {x, y} }
    this._targetId = this.dataset.target || "";
    this._readonly = this.dataset.readonly === "true";
    this._storageKey = `pb-pos:${this._targetId}`;
    this._mode = "graph";           // "graph" | "json"
    this._selectedId = null;
    this._pan = { x: 0, y: 0 };
    this._zoom = 1;
    this._pending = null;           // { fromId, x, y } during port drag
    this._rawJson = null;

    const target = this._target();
    if (target && target.value.trim()) {
      try {
        this.stages = this._parseStages(JSON.parse(target.value.trim()));
      } catch (e) {
        this._rawJson = target.value.trim();
      }
    }

    this._loadPositions();
    this._autoLayoutMissing();
    this._render();

    if (!this._readonly) this._bindFormGuard();
  }

  disconnectedCallback() {
    this._unbindGlobal();
    if (this._resizeObserver) {
      this._resizeObserver.disconnect();
      this._resizeObserver = null;
    }
    if (this._formGuard) {
      const { form, handler } = this._formGuard;
      form.removeEventListener("submit", handler);
      this._formGuard = null;
    }
    if (activeEditor === this) activeEditor = null;
  }

  _bindFormGuard() {
    const form = this.closest("form");
    if (!form) return;
    const handler = (e) => {
      // If we're in JSON mode, the raw textarea drives submission directly
      // and the user is expected to fix invalid JSON themselves. Validate
      // only graph-derived state.
      if (this._mode === "json") {
        if (this._rawJson != null) {
          try { JSON.parse(this._rawJson); }
          catch (err) {
            e.preventDefault();
            alert("Pipeline JSON is invalid: " + err.message);
            return;
          }
        }
        return;
      }
      this._sync();
      const errors = this._validate();
      if (errors.length > 0) {
        e.preventDefault();
        alert("Cannot save pipeline:\n\n• " + errors.join("\n• "));
      }
    };
    form.addEventListener("submit", handler);
    this._formGuard = { form, handler };
  }

  // ─── data plumbing ──────────────────────────────────────────────────────

  _target() {
    return this._targetId ? document.getElementById(this._targetId) : null;
  }

  _stageType(config) {
    if (!config) return "deploy";
    if (config.Deploy !== undefined) return "deploy";
    if (config.Wait !== undefined) return "wait";
    if (config.Plan !== undefined) return "plan";
    return "deploy";
  }

  _normalizeStage(s) {
    if (s.id !== undefined) {
      return {
        id: s.id || "",
        depends_on: Array.isArray(s.depends_on) ? s.depends_on.slice() : [],
        config: s.config || { Deploy: { environment: "" } },
      };
    }
    // Legacy {name, type, depends_on}
    const type = s.type || "deploy";
    const config = type === "wait"
      ? { Wait: { duration_seconds: s.duration_seconds || 0 } }
      : type === "plan"
      ? { Plan: { environment: s.environment || "", auto_approve: !!s.auto_approve } }
      : { Deploy: { environment: s.environment || "" } };
    return {
      id: s.name || "",
      depends_on: Array.isArray(s.depends_on) ? s.depends_on.slice() : [],
      config,
    };
  }

  _parseStages(parsed) {
    if (Array.isArray(parsed)) return parsed.map((s) => this._normalizeStage(s));
    if (parsed && Array.isArray(parsed.stages)) return parsed.stages.map((s) => this._normalizeStage(s));
    if (parsed && typeof parsed === "object") {
      return Object.entries(parsed).map(([id, val]) => this._normalizeStage({ id, ...val }));
    }
    return [];
  }

  _sync() {
    const target = this._target();
    if (!target) return;
    const valid = this.stages.filter((s) => s.id.trim());
    target.value = JSON.stringify(valid, null, 2);
  }

  _validate() {
    const ids = this.stages.map((s) => s.id).filter(Boolean);
    const idSet = new Set(ids);
    const errors = [];

    if (ids.length !== idSet.size) errors.push("Duplicate stage IDs");

    for (const s of this.stages) {
      if (!s.id.trim()) {
        errors.push("A stage is missing an ID");
        continue;
      }
      for (const dep of s.depends_on) {
        if (!idSet.has(dep)) errors.push(`"${s.id}" depends on unknown "${dep}"`);
      }
    }

    // cycle detection (Kahn)
    const inDeg = {};
    const adj = {};
    for (const s of this.stages) {
      if (!s.id) continue;
      inDeg[s.id] = 0;
      adj[s.id] = [];
    }
    for (const s of this.stages) {
      if (!s.id) continue;
      for (const dep of s.depends_on) {
        if (adj[dep]) {
          adj[dep].push(s.id);
          inDeg[s.id]++;
        }
      }
    }
    const queue = Object.keys(inDeg).filter((k) => inDeg[k] === 0);
    let visited = 0;
    while (queue.length > 0) {
      const node = queue.shift();
      visited++;
      for (const next of adj[node] || []) {
        if (--inDeg[next] === 0) queue.push(next);
      }
    }
    if (visited < Object.keys(inDeg).length) errors.push("Cycle detected in dependencies");

    // dedupe
    return Array.from(new Set(errors));
  }

  // ─── positions / layout ─────────────────────────────────────────────────

  _loadPositions() {
    if (!this._storageKey) return;
    try {
      const raw = localStorage.getItem(this._storageKey);
      if (raw) this.positions = JSON.parse(raw) || {};
    } catch (e) { /* ignore */ }
  }

  _savePositions() {
    if (!this._storageKey) return;
    try {
      // only persist positions for currently named stages
      const live = {};
      for (const s of this.stages) {
        if (s.id && this.positions[s.id]) live[s.id] = this.positions[s.id];
      }
      this.positions = live;
      localStorage.setItem(this._storageKey, JSON.stringify(live));
    } catch (e) { /* ignore quota */ }
  }

  _computeLevels() {
    const byId = {};
    for (const s of this.stages) if (s.id) byId[s.id] = s;
    const levels = {};
    const visiting = new Set();

    const level = (id) => {
      if (levels[id] !== undefined) return levels[id];
      if (visiting.has(id)) return 0;
      visiting.add(id);
      const s = byId[id];
      if (!s || s.depends_on.length === 0) {
        levels[id] = 0;
        return 0;
      }
      let m = 0;
      for (const dep of s.depends_on) {
        if (byId[dep]) m = Math.max(m, level(dep) + 1);
      }
      levels[id] = m;
      return m;
    };
    for (const s of this.stages) if (s.id) level(s.id);
    return levels;
  }

  _autoLayoutMissing() {
    const missing = this.stages.filter((s) => s.id && !this.positions[s.id]);
    if (missing.length === 0) return;
    this._autoLayout(missing.map((s) => s.id));
  }

  _autoLayout(only) {
    const levels = this._computeLevels();
    const cols = {};
    for (const s of this.stages) {
      if (!s.id) continue;
      const l = levels[s.id] || 0;
      cols[l] = cols[l] || [];
      cols[l].push(s.id);
    }
    const colKeys = Object.keys(cols).map(Number).sort((a, b) => a - b);
    const COL_GAP = 80;
    const ROW_GAP = 30;
    for (const col of colKeys) {
      const ids = cols[col];
      const colH = ids.length * NODE_H + (ids.length - 1) * ROW_GAP;
      for (let i = 0; i < ids.length; i++) {
        const id = ids[i];
        if (only && !only.includes(id)) continue;
        this.positions[id] = {
          x: 40 + col * (NODE_W + COL_GAP),
          y: 40 + i * (NODE_H + ROW_GAP) + (colH < 200 ? (200 - colH) / 2 : 0),
        };
      }
    }
    this._savePositions();
  }

  _fit() {
    const stage = this.querySelector(".pb-stage");
    if (!stage) return;
    const named = this.stages.filter((s) => s.id && this.positions[s.id]);
    if (named.length === 0) {
      this._pan = { x: 0, y: 0 };
      this._zoom = 1;
      this._applyTransform();
      return;
    }
    let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
    for (const s of named) {
      const p = this.positions[s.id];
      minX = Math.min(minX, p.x);
      minY = Math.min(minY, p.y);
      maxX = Math.max(maxX, p.x + NODE_W);
      maxY = Math.max(maxY, p.y + NODE_H);
    }
    const pad = 40;
    const w = maxX - minX + pad * 2;
    const h = maxY - minY + pad * 2;
    const rect = stage.getBoundingClientRect();
    const zx = rect.width / w;
    const zy = rect.height / h;
    this._zoom = Math.min(1.25, Math.max(0.25, Math.min(zx, zy)));
    this._pan = {
      x: (rect.width - (maxX + minX) * this._zoom) / 2,
      y: (rect.height - (maxY + minY) * this._zoom) / 2,
    };
    this._applyTransform();
  }

  // ─── render ─────────────────────────────────────────────────────────────

  _render() {
    this._unbindGlobal();
    if (this._resizeObserver) {
      this._resizeObserver.disconnect();
      this._resizeObserver = null;
    }
    this.innerHTML = "";
    this.className = "block";

    if (this._readonly) {
      // Read-only: just show the graph, no inspector / toolbar editing
      this.append(this._renderGraph(true));
      return;
    }

    this.append(this._renderToolbar());

    if (this._mode === "json") {
      this.append(this._renderJsonMode());
      return;
    }

    const errors = this._validate();
    this._sync();

    const wrap = el("div", "pb-wrap relative flex border border-gray-200 rounded-xl overflow-hidden bg-white shadow-sm");
    wrap.style.height = "560px";

    const graph = this._renderGraph(false);
    graph.classList.add("flex-1", "min-w-0");
    wrap.append(graph);

    const inspector = this._renderInspector();
    if (inspector) wrap.append(inspector);

    this.append(wrap);

    if (errors.length > 0) {
      const box = el("div", "mt-2 px-3 py-2.5 bg-red-50 border border-red-200 rounded-lg text-xs flex items-start gap-2");
      const iconWrap = el("span", "inline-flex text-red-600 shrink-0 mt-0.5");
      iconWrap.append(svgIcon(ICONS.warn, 14, 2));
      box.append(iconWrap);
      const content = el("div", "flex-1 min-w-0");
      const head = el("p", "font-semibold text-red-800 mb-1");
      head.textContent = errors.length === 1 ? "1 issue prevents saving" : `${errors.length} issues prevent saving`;
      content.append(head);
      const list = el("ul", "list-disc pl-4 space-y-0.5 text-red-700");
      for (const e of errors) {
        const li = el("li", "");
        li.textContent = e;
        list.append(li);
      }
      content.append(list);
      box.append(content);
      this.append(box);
    }
  }

  _renderToolbar() {
    const bar = el("div", "flex items-center gap-1.5 mb-2 flex-wrap");

    if (this._mode === "graph") {
      const addBtn = (type) => {
        const style = TYPE_STYLE[type];
        const b = el("button", "text-xs px-2.5 py-1.5 rounded-md border border-gray-200 bg-white text-gray-700 inline-flex items-center gap-1.5 hover:bg-gray-50 hover:border-gray-300 transition-colors");
        b.type = "button";
        b.title = `Add ${style.label} stage`;
        const iconWrap = el("span", "inline-flex");
        iconWrap.style.color = style.accent;
        iconWrap.append(svgIcon(type, 14));
        b.append(iconWrap, document.createTextNode(style.label));
        b.onclick = () => this._addStage(type);
        return b;
      };

      const addLabel = el("span", "text-2xs uppercase tracking-wider text-gray-400 font-semibold mr-1");
      addLabel.textContent = "Add";
      bar.append(addLabel);

      const addGroup = el("div", "inline-flex items-center gap-1");
      addGroup.append(addBtn("deploy"), addBtn("wait"), addBtn("plan"));
      bar.append(addGroup);
    }

    const spacer = el("span", "flex-1");
    bar.append(spacer);

    // Mode toggle pill
    const modePill = el("div", "inline-flex p-0.5 bg-gray-100 rounded-md");
    const makeModeBtn = (mode, label) => {
      const isActive = this._mode === mode;
      const b = el("button", `text-xs px-2.5 py-1 rounded ${isActive ? "bg-white text-gray-900 shadow-sm" : "text-gray-500 hover:text-gray-700"} transition-colors`, label);
      b.type = "button";
      b.onclick = () => {
        if (this._mode === mode) return;
        if (this._mode === "json" && mode === "graph") {
          const ta = this.querySelector(".pb-json");
          if (ta) {
            try {
              this.stages = this._parseStages(JSON.parse(ta.value));
              this._rawJson = null;
              this._autoLayoutMissing();
            } catch (e) { this._rawJson = ta.value; }
          }
        }
        this._mode = mode;
        this._render();
        if (mode === "graph") requestAnimationFrame(() => this._fit());
      };
      return b;
    };
    modePill.append(makeModeBtn("graph", "Graph"), makeModeBtn("json", "JSON"));
    bar.append(modePill);

    return bar;
  }

  _renderCanvasControls(stage) {
    // Floating control cluster anchored bottom-right of the canvas.
    const wrap = el("div", "absolute bottom-3 right-3 flex flex-col gap-1");
    wrap.style.zIndex = "10";

    const makeBtn = (icon, title, onClick) => {
      const b = el("button", "w-8 h-8 flex items-center justify-center rounded-md border border-gray-200 bg-white text-gray-600 hover:text-gray-900 hover:bg-gray-50 shadow-sm transition-colors");
      b.type = "button";
      b.title = title;
      b.append(svgIcon(icon, 14, 2));
      b.onclick = (e) => { e.preventDefault(); onClick(); };
      // Don't let the canvas grab these as pan-starts
      b.onmousedown = (e) => e.stopPropagation();
      return b;
    };

    const group1 = el("div", "flex flex-col rounded-md overflow-hidden border border-gray-200 bg-white shadow-sm");
    const zin = makeBtn("plus", "Zoom in (⌘/Ctrl + scroll)", () => this._zoomBy(1.2));
    const zout = makeBtn("minus", "Zoom out (⌘/Ctrl + scroll)", () => this._zoomBy(1 / 1.2));
    // strip extra borders that came from makeBtn now that they sit in a grouped container
    [zin, zout].forEach((b) => { b.className = "w-8 h-8 flex items-center justify-center text-gray-600 hover:text-gray-900 hover:bg-gray-50 transition-colors"; });
    const div1 = el("div", "h-px bg-gray-200");
    group1.append(zin, div1, zout);

    const group2 = el("div", "flex flex-col rounded-md overflow-hidden border border-gray-200 bg-white shadow-sm");
    const fit = makeBtn("fit", "Fit to content", () => this._fit());
    const layout = makeBtn("layout", "Auto layout", () => {
      this._autoLayout();
      this._render();
      requestAnimationFrame(() => this._fit());
    });
    [fit, layout].forEach((b) => { b.className = "w-8 h-8 flex items-center justify-center text-gray-600 hover:text-gray-900 hover:bg-gray-50 transition-colors"; });
    const div2 = el("div", "h-px bg-gray-200");
    group2.append(fit, div2, layout);

    wrap.append(group1, group2);
    stage.append(wrap);
  }

  _zoomBy(factor) {
    const stage = this.querySelector(".pb-stage");
    if (!stage) return;
    const rect = stage.getBoundingClientRect();
    const cx = rect.width / 2;
    const cy = rect.height / 2;
    const before = this._screenToCanvas(cx, cy);
    this._zoom = Math.min(2.5, Math.max(0.2, this._zoom * factor));
    this._pan.x = cx - before.x * this._zoom;
    this._pan.y = cy - before.y * this._zoom;
    this._applyTransform();
  }

  _renderJsonMode() {
    const wrap = el("div", "");
    const target = this._target();
    const current = this._rawJson != null
      ? this._rawJson
      : (this.stages.length > 0 ? JSON.stringify(this.stages.filter((s) => s.id), null, 2) : (target ? target.value : ""));
    const ta = el("textarea", "pb-json w-full border border-gray-300 rounded-md px-3 py-2 text-sm font-mono focus:outline-none focus:ring-2 focus:ring-gray-900 resize-y");
    ta.rows = 14;
    ta.value = current;
    ta.spellcheck = false;
    ta.oninput = () => {
      const t = this._target();
      if (t) t.value = ta.value;
      this._rawJson = ta.value;
      this._updateJsonErrors(ta.value);
    };
    const errs = el("div", "pb-json-errs mt-2");
    wrap.append(ta, errs);
    requestAnimationFrame(() => this._updateJsonErrors(current));
    return wrap;
  }

  _updateJsonErrors(value) {
    const box = this.querySelector(".pb-json-errs");
    if (!box) return;
    box.innerHTML = "";
    if (!value.trim()) return;
    try {
      const parsed = JSON.parse(value);
      const stages = Array.isArray(parsed) ? parsed : (parsed.stages || []);
      const ids = stages.map((s) => s.id || s.name).filter(Boolean);
      if (new Set(ids).size !== ids.length) {
        box.append(el("p", "text-xs text-amber-600", "Warning: duplicate stage IDs"));
      }
    } catch (e) {
      box.append(el("p", "text-xs text-red-600", "Invalid JSON: " + e.message));
    }
  }

  // ─── canvas ─────────────────────────────────────────────────────────────

  _renderGraph(readonly) {
    const stage = el("div", "pb-stage relative overflow-hidden select-none");
    stage.style.background = "var(--color-gray-50)";
    stage.style.backgroundImage = "radial-gradient(circle, var(--color-gray-200) 1px, transparent 1px)";
    stage.style.backgroundSize = `${GRID}px ${GRID}px`;
    if (readonly) {
      stage.style.height = "260px";
      stage.style.border = "1px solid var(--color-gray-200)";
      stage.style.borderRadius = "8px";
    } else {
      stage.style.minHeight = "100%";
    }

    const viewport = el("div", "pb-viewport absolute top-0 left-0");
    viewport.style.transformOrigin = "0 0";
    viewport.style.width = "1px";
    viewport.style.height = "1px";

    // SVG for edges sits in viewport so it shares the transform
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.classList.add("pb-edges");
    svg.setAttribute("width", String(SVG_HALF * 2));
    svg.setAttribute("height", String(SVG_HALF * 2));
    svg.style.position = "absolute";
    svg.style.left = `-${SVG_HALF}px`;
    svg.style.top = `-${SVG_HALF}px`;
    svg.style.pointerEvents = "none";
    svg.style.overflow = "visible";
    const safeTarget = (this._targetId || "view").replace(/[^a-zA-Z0-9_-]/g, "_");
    const markerId = `pb-arrow-${safeTarget}`;
    this._markerId = markerId;
    const defs = document.createElementNS("http://www.w3.org/2000/svg", "defs");
    const marker = document.createElementNS("http://www.w3.org/2000/svg", "marker");
    marker.setAttribute("id", markerId);
    marker.setAttribute("viewBox", "0 0 10 10");
    marker.setAttribute("refX", "10");
    marker.setAttribute("refY", "5");
    marker.setAttribute("markerWidth", "6");
    marker.setAttribute("markerHeight", "6");
    marker.setAttribute("orient", "auto");
    const arrow = document.createElementNS("http://www.w3.org/2000/svg", "path");
    arrow.setAttribute("d", "M 0 0 L 10 5 L 0 10 z");
    arrow.setAttribute("fill", "currentColor");
    marker.append(arrow);
    defs.append(marker);
    svg.append(defs);
    // currentColor on the SVG drives both edge stroke and marker fill.
    svg.style.color = "var(--color-gray-400)";
    viewport.append(svg);

    const nodeLayer = el("div", "pb-nodes absolute top-0 left-0");
    viewport.append(nodeLayer);
    stage.append(viewport);

    if (!readonly) {
      // Hint pill, anchored bottom-left, kept visually quiet
      const hint = el("div", "absolute bottom-3 left-3 text-3xs text-gray-400 pointer-events-none flex items-center gap-1.5 bg-white/70 backdrop-blur-sm border border-gray-200 rounded-md px-2 py-1");
      hint.innerHTML = `<span>Drag to pan</span><span class="text-gray-300">·</span><span><kbd class="font-mono text-4xs">⌘</kbd>+scroll to zoom</span><span class="text-gray-300">·</span><span>drag port to connect</span>`;
      stage.append(hint);

      // Empty state when no stages exist
      if (this.stages.filter((s) => s.id).length === 0) {
        const empty = el("div", "absolute inset-0 flex flex-col items-center justify-center text-center px-6 pointer-events-none");
        const ring = el("div", "w-12 h-12 rounded-full bg-gray-100 flex items-center justify-center mb-3 text-gray-400");
        ring.append(svgIcon(ICONS.plus, 24, 1.5));
        empty.append(ring);
        const title = el("p", "text-sm font-medium text-gray-700 mb-1");
        title.textContent = "No stages yet";
        const subtitle = el("p", "text-xs text-gray-500 max-w-xs");
        subtitle.textContent = "Add a stage from the toolbar above, or double-click anywhere on the canvas.";
        empty.append(title, subtitle);
        stage.append(empty);
      }

      // Floating zoom/fit/layout controls
      this._renderCanvasControlsDeferred = () => this._renderCanvasControls(stage);
    }

    if (readonly && this.stages.filter((s) => s.id).length === 0) {
      const empty = el("p", "absolute inset-0 flex items-center justify-center text-xs text-gray-400 italic");
      empty.textContent = "No stages defined";
      stage.append(empty);
    }

    // defer initial draw to next frame so dimensions are known
    requestAnimationFrame(() => {
      this._renderNodes();
      this._renderEdges();
      if (this._renderCanvasControlsDeferred) {
        this._renderCanvasControlsDeferred();
        this._renderCanvasControlsDeferred = null;
      }
      const rect = stage.getBoundingClientRect();
      if (rect.width > 0 && rect.height > 0) {
        if (readonly || (this._pan.x === 0 && this._pan.y === 0 && this._zoom === 1)) this._fit();
        else this._applyTransform();
      } else if ("ResizeObserver" in window) {
        // stage hidden (e.g. inside a closed <details>) — fit when it becomes visible
        const ro = new ResizeObserver((entries) => {
          for (const entry of entries) {
            if (entry.contentRect.width > 0 && entry.contentRect.height > 0) {
              ro.disconnect();
              this._fit();
            }
          }
        });
        ro.observe(stage);
        this._resizeObserver = ro;
      }
    });

    if (!readonly) this._bindInteractions(stage);

    return stage;
  }

  _renderNodes() {
    const layer = this.querySelector(".pb-nodes");
    if (!layer) return;
    layer.innerHTML = "";

    for (const stage of this.stages) {
      if (!stage.id) continue;
      const pos = this.positions[stage.id];
      if (!pos) continue;
      const type = this._stageType(stage.config);
      const style = TYPE_STYLE[type];
      const isSelected = this._selectedId === stage.id;

      const node = el("div", "pb-node absolute rounded-lg overflow-hidden");
      node.dataset.id = stage.id;
      node.style.left = pos.x + "px";
      node.style.top = pos.y + "px";
      node.style.width = NODE_W + "px";
      node.style.height = NODE_H + "px";
      node.style.background = "var(--color-white)";
      node.style.border = "1px solid var(--color-gray-200)";
      node.style.color = "var(--color-gray-900)";
      node.style.cursor = this._readonly ? "default" : "grab";
      node.style.boxSizing = "border-box";
      node.style.transition = "border-color 120ms ease, box-shadow 120ms ease, transform 120ms ease";
      node.style.boxShadow = isSelected
        ? `0 0 0 2px ${style.accent}, 0 8px 24px -8px rgba(15,23,42,0.35)`
        : "0 1px 2px rgba(15,23,42,0.06)";

      // Left accent stripe (color-coded by stage type)
      const stripe = el("div", "absolute top-0 left-0 bottom-0");
      stripe.style.width = "4px";
      stripe.style.background = style.accent;
      node.append(stripe);

      // Body
      const body = el("div", "pb-node-body h-full pl-4 pr-3 py-2 flex items-center gap-2.5");

      // Icon disc
      const iconBox = el("div", "flex items-center justify-center rounded-md shrink-0");
      iconBox.style.width = "30px";
      iconBox.style.height = "30px";
      iconBox.style.background = style.tint;
      iconBox.style.color = style.accent;
      iconBox.append(svgIcon(type, 16, 2));
      body.append(iconBox);

      // Text column
      const text = el("div", "min-w-0 flex-1");

      const topRow = el("div", "pb-node-top flex items-center gap-1.5");
      const typeLabel = el("span", "text-3xs uppercase tracking-wider font-semibold text-gray-400");
      typeLabel.textContent = style.label;
      topRow.append(typeLabel);

      const cfgLabel = this._configLabel(stage.config);
      if (cfgLabel) {
        const dot = el("span", "text-gray-300");
        dot.textContent = "·";
        const sub = el("span", "text-3xs font-mono text-gray-500 truncate");
        sub.textContent = cfgLabel;
        sub.title = cfgLabel;
        sub.style.maxWidth = "100px";
        topRow.append(dot, sub);
      }
      text.append(topRow);

      const idEl = el("div", "text-sm font-semibold text-gray-900 truncate leading-tight mt-0.5");
      idEl.textContent = stage.id;
      idEl.title = stage.id;
      text.append(idEl);

      body.append(text);
      node.append(body);

      // input port (left)
      const inPort = el("div", "pb-port pb-port-in absolute rounded-full");
      inPort.dataset.role = "in";
      inPort.dataset.id = stage.id;
      inPort.style.width = "12px";
      inPort.style.height = "12px";
      inPort.style.left = "-7px";
      inPort.style.top = (NODE_H / 2 - 6) + "px";
      inPort.style.background = "var(--color-white)";
      inPort.style.border = `2px solid ${style.accent}`;
      inPort.style.cursor = this._readonly ? "default" : "crosshair";
      inPort.style.transition = "transform 120ms ease, background 120ms ease";
      inPort.style.zIndex = "2";
      node.append(inPort);

      // output port (right)
      const outPort = el("div", "pb-port pb-port-out absolute rounded-full");
      outPort.dataset.role = "out";
      outPort.dataset.id = stage.id;
      outPort.style.width = "12px";
      outPort.style.height = "12px";
      outPort.style.right = "-7px";
      outPort.style.top = (NODE_H / 2 - 6) + "px";
      outPort.style.background = style.accent;
      outPort.style.border = `2px solid ${style.accent}`;
      outPort.style.cursor = this._readonly ? "default" : "crosshair";
      outPort.style.transition = "transform 120ms ease";
      outPort.style.zIndex = "2";
      node.append(outPort);

      layer.append(node);
    }
  }

  _configLabel(config) {
    if (!config) return "";
    if (config.Deploy) return config.Deploy.environment || "";
    if (config.Wait) {
      const s = config.Wait.duration_seconds;
      return s ? (s >= 60 ? `${Math.floor(s/60)}m${s%60 ? ` ${s%60}s` : ""}` : `${s}s`) : "";
    }
    if (config.Plan) return (config.Plan.environment || "") + (config.Plan.auto_approve ? " · auto" : "");
    return "";
  }

  _renderEdges() {
    const svg = this.querySelector(".pb-edges");
    if (!svg) return;
    // wipe everything except <defs>
    Array.from(svg.children).forEach((c) => { if (c.tagName !== "defs") svg.removeChild(c); });

    const OFFSET_X = SVG_HALF;
    const OFFSET_Y = SVG_HALF;
    const markerId = this._markerId || `pb-arrow-${(this._targetId || "view").replace(/[^a-zA-Z0-9_-]/g, "_")}`;

    const portPos = (id, role) => {
      const p = this.positions[id];
      if (!p) return null;
      return {
        x: p.x + (role === "out" ? NODE_W : 0),
        y: p.y + NODE_H / 2,
      };
    };

    for (const s of this.stages) {
      if (!s.id) continue;
      const to = portPos(s.id, "in");
      if (!to) continue;
      for (const dep of s.depends_on) {
        const from = portPos(dep, "out");
        if (!from) continue;
        const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
        path.setAttribute("d", bezier(from, to, OFFSET_X, OFFSET_Y));
        path.setAttribute("fill", "none");
        path.setAttribute("stroke", "currentColor");
        path.setAttribute("stroke-width", "1.5");
        path.setAttribute("marker-end", `url(#${markerId})`);
        svg.append(path);
      }
    }

    if (this._pending) {
      const from = portPos(this._pending.fromId, "out");
      if (from) {
        const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
        path.setAttribute("d", bezier(from, { x: this._pending.x, y: this._pending.y }, OFFSET_X, OFFSET_Y));
        path.setAttribute("fill", "none");
        path.setAttribute("stroke", "var(--color-gray-900)");
        path.setAttribute("stroke-width", "1.5");
        path.setAttribute("stroke-dasharray", "5 4");
        path.setAttribute("opacity", "0.7");
        svg.append(path);
      }
    }
  }

  _applyTransform() {
    const viewport = this.querySelector(".pb-viewport");
    if (!viewport) return;
    viewport.style.transform = `translate(${this._pan.x}px, ${this._pan.y}px) scale(${this._zoom})`;
  }

  // ─── interactions ───────────────────────────────────────────────────────

  _bindInteractions(stage) {
    let mode = null;            // 'pan' | 'node' | 'port' | null
    let active = null;          // drag state
    const DRAG_THRESHOLD = 4;   // px before a click becomes a drag

    const claimFocus = () => { activeEditor = this; };

    const onDown = (e) => {
      if (e.button !== 0) return;
      claimFocus();
      const port = e.target.closest(".pb-port");
      if (port) {
        if (port.dataset.role !== "out") return;
        mode = "port";
        const rect = stage.getBoundingClientRect();
        const pt = this._screenToCanvas(e.clientX - rect.left, e.clientY - rect.top);
        this._pending = { fromId: port.dataset.id, x: pt.x, y: pt.y };
        this._renderEdges();
        e.preventDefault();
        e.stopPropagation();
        return;
      }
      const node = e.target.closest(".pb-node");
      if (node) {
        const id = node.dataset.id;
        const pos = this.positions[id];
        if (!pos) return;
        mode = "node";
        active = {
          id,
          startX: e.clientX,
          startY: e.clientY,
          origX: pos.x,
          origY: pos.y,
          moved: false,
          prevSelected: this._selectedId,
        };
        // Selection visuals + inspector update happen in-place (no full
        // render) so the in-progress drag closures stay valid.
        this._selectedId = id;
        this._updateSelectionVisuals();
        this._replaceInspector();
        node.style.cursor = "grabbing";
        e.preventDefault();
        return;
      }
      // background → pan (deselect deferred to onUp so we don't render
      // mid-event and orphan this closure's stage/handlers).
      mode = "pan";
      active = {
        startX: e.clientX,
        startY: e.clientY,
        origPan: { ...this._pan },
        moved: false,
      };
      stage.style.cursor = "grabbing";
    };

    const onMove = (e) => {
      if (mode === "pan") {
        const dx = e.clientX - active.startX;
        const dy = e.clientY - active.startY;
        if (!active.moved && (Math.abs(dx) > DRAG_THRESHOLD || Math.abs(dy) > DRAG_THRESHOLD)) {
          active.moved = true;
        }
        this._pan = { x: active.origPan.x + dx, y: active.origPan.y + dy };
        this._applyTransform();
      } else if (mode === "node") {
        const dx = (e.clientX - active.startX) / this._zoom;
        const dy = (e.clientY - active.startY) / this._zoom;
        if (!active.moved && (Math.abs(dx * this._zoom) > DRAG_THRESHOLD || Math.abs(dy * this._zoom) > DRAG_THRESHOLD)) {
          active.moved = true;
        }
        let nx = Math.round((active.origX + dx) / GRID) * GRID;
        let ny = Math.round((active.origY + dy) / GRID) * GRID;
        this.positions[active.id] = { x: nx, y: ny };
        const node = this.querySelector(`.pb-node[data-id="${cssEsc(active.id)}"]`);
        if (node) {
          node.style.left = nx + "px";
          node.style.top = ny + "px";
        }
        this._renderEdges();
      } else if (mode === "port") {
        const rect = stage.getBoundingClientRect();
        const pt = this._screenToCanvas(e.clientX - rect.left, e.clientY - rect.top);
        this._pending.x = pt.x;
        this._pending.y = pt.y;
        this._renderEdges();
      }
    };

    const onUp = (e) => {
      if (mode === "node") {
        const node = this.querySelector(`.pb-node[data-id="${cssEsc(active.id)}"]`);
        if (node) node.style.cursor = "grab";
        if (active.moved) this._savePositions();
      } else if (mode === "port") {
        const target = document.elementFromPoint(e.clientX, e.clientY);
        let toId = null;
        const port = target ? target.closest(".pb-port") : null;
        if (port && port.dataset.role === "in") {
          toId = port.dataset.id;
        } else {
          // Fallback: dropping on the node body counts as the input port.
          const dropNode = target ? target.closest(".pb-node") : null;
          if (dropNode) toId = dropNode.dataset.id;
        }
        if (toId && toId !== this._pending.fromId) {
          const result = this._connect(this._pending.fromId, toId);
          if (!result.ok) this._flashPort(toId, result.reason);
        }
        this._pending = null;
        this._renderEdges();
      } else if (mode === "pan") {
        stage.style.cursor = "";
        // A click on background (no drag) deselects.
        if (!active.moved && this._selectedId) {
          this._selectedId = null;
          this._updateSelectionVisuals();
          this._replaceInspector();
        }
      }
      mode = null;
      active = null;
    };

    const onWheel = (e) => {
      // Only zoom with a modifier so plain scroll still scrolls the page.
      if (!e.ctrlKey && !e.metaKey) return;
      e.preventDefault();
      const rect = stage.getBoundingClientRect();
      const cx = e.clientX - rect.left;
      const cy = e.clientY - rect.top;
      const before = this._screenToCanvas(cx, cy);
      const factor = e.deltaY < 0 ? 1.1 : 1 / 1.1;
      this._zoom = Math.min(2.5, Math.max(0.2, this._zoom * factor));
      this._pan.x = cx - before.x * this._zoom;
      this._pan.y = cy - before.y * this._zoom;
      this._applyTransform();
    };

    const onDbl = (e) => {
      if (e.target.closest(".pb-node") || e.target.closest(".pb-port")) return;
      const rect = stage.getBoundingClientRect();
      const pt = this._screenToCanvas(e.clientX - rect.left, e.clientY - rect.top);
      this._addStage("deploy", pt.x - NODE_W / 2, pt.y - NODE_H / 2);
    };

    const onKey = (e) => {
      if (activeEditor !== this) return;
      if (!this._selectedId) return;
      const tag = (e.target.tagName || "").toLowerCase();
      if (tag === "input" || tag === "textarea" || tag === "select") return;
      // Delete only — Backspace is dangerous because users press it to
      // navigate back; deleting a node should be deliberate.
      if (e.key === "Delete") {
        e.preventDefault();
        this._removeStage(this._selectedId);
      } else if (e.key === "Escape") {
        e.preventDefault();
        this._selectedId = null;
        this._updateSelectionVisuals();
        this._replaceInspector();
      }
    };

    stage.addEventListener("mousedown", onDown);
    stage.addEventListener("wheel", onWheel, { passive: false });
    stage.addEventListener("dblclick", onDbl);
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    window.addEventListener("keydown", onKey);

    this._unbindGlobal = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      window.removeEventListener("keydown", onKey);
      this._unbindGlobal = () => {};
    };
  }

  _updateSelectionVisuals() {
    const nodes = this.querySelectorAll(".pb-node");
    nodes.forEach((node) => {
      const id = node.dataset.id;
      const stage = this.stages.find((s) => s.id === id);
      if (!stage) return;
      const style = TYPE_STYLE[this._stageType(stage.config)];
      const isSel = id === this._selectedId;
      node.style.border = `2px solid ${isSel ? "#111827" : style.border}`;
      node.style.boxShadow = isSel ? "0 4px 14px -2px rgba(17,24,39,0.25)" : "";
    });
  }

  _replaceInspector() {
    const old = this.querySelector(".pb-inspector");
    if (!old) return;
    const fresh = this._renderInspector();
    if (fresh) old.replaceWith(fresh);
  }

  _flashPort(id, reason) {
    const node = this.querySelector(`.pb-node[data-id="${cssEsc(id)}"]`);
    if (!node) return;
    const port = node.querySelector(".pb-port-in");
    if (!port) return;
    const prevBorder = port.style.border;
    const prevBg = port.style.background;
    port.style.border = "2px solid #ef4444";
    port.style.background = "#fee2e2";
    if (reason) port.title = reason;
    setTimeout(() => {
      port.style.border = prevBorder;
      port.style.background = prevBg;
      port.title = "";
    }, 600);
  }

  _unbindGlobal() { /* replaced on bind */ }

  _screenToCanvas(sx, sy) {
    return {
      x: (sx - this._pan.x) / this._zoom,
      y: (sy - this._pan.y) / this._zoom,
    };
  }

  // ─── stage operations ──────────────────────────────────────────────────

  _uniqueId(base) {
    const taken = new Set(this.stages.map((s) => s.id));
    if (!taken.has(base)) return base;
    let i = 2;
    while (taken.has(`${base}-${i}`)) i++;
    return `${base}-${i}`;
  }

  _defaultConfig(type) {
    if (type === "wait") return { Wait: { duration_seconds: 30 } };
    if (type === "plan") return { Plan: { environment: "", auto_approve: false } };
    return { Deploy: { environment: "" } };
  }

  _addStage(type, x, y) {
    const id = this._uniqueId(type);
    this.stages.push({ id, depends_on: [], config: this._defaultConfig(type) });
    if (x !== undefined && y !== undefined) {
      this.positions[id] = { x: Math.round(x / GRID) * GRID, y: Math.round(y / GRID) * GRID };
    } else {
      this.positions[id] = this._findFreeSlot();
    }
    this._selectedId = id;
    this._savePositions();
    this._render();
    // Ensure the new stage is visible — refit only if it lies outside the
    // currently displayed region.
    requestAnimationFrame(() => this._ensureVisible(id));
  }

  _ensureVisible(id) {
    const pos = this.positions[id];
    const stage = this.querySelector(".pb-stage");
    if (!pos || !stage) return;
    const rect = stage.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return;
    // Convert node bounds to screen-space.
    const sx1 = pos.x * this._zoom + this._pan.x;
    const sy1 = pos.y * this._zoom + this._pan.y;
    const sx2 = sx1 + NODE_W * this._zoom;
    const sy2 = sy1 + NODE_H * this._zoom;
    const margin = 20;
    if (sx1 < margin || sy1 < margin || sx2 > rect.width - margin || sy2 > rect.height - margin) {
      this._fit();
    }
  }

  _findFreeSlot() {
    // Pick a non-colliding canvas position: to the right of the existing
    // rightmost node when there are stages, otherwise the visible centre.
    const others = Object.entries(this.positions).map(([_, p]) => p);
    let target;
    if (others.length === 0) {
      const stageEl = this.querySelector(".pb-stage");
      if (stageEl) {
        const rect = stageEl.getBoundingClientRect();
        const pt = this._screenToCanvas(rect.width / 2, rect.height / 2);
        target = { x: pt.x - NODE_W / 2, y: pt.y - NODE_H / 2 };
      } else {
        target = { x: 60, y: 60 };
      }
    } else {
      let maxX = -Infinity, sumY = 0, count = 0;
      for (const p of others) {
        if (p.x > maxX) maxX = p.x;
        sumY += p.y;
        count++;
      }
      target = { x: maxX + NODE_W + 60, y: count > 0 ? sumY / count : 60 };
    }
    let x = Math.round(target.x / GRID) * GRID;
    let y = Math.round(target.y / GRID) * GRID;
    const overlaps = (px, py) => {
      for (const p of others) {
        if (Math.abs(p.x - px) < NODE_W * 0.6 && Math.abs(p.y - py) < NODE_H * 0.9) return true;
      }
      return false;
    };
    let guard = 0;
    while (overlaps(x, y) && guard++ < 40) {
      y += NODE_H + 30;
    }
    return { x, y };
  }

  _removeStage(id) {
    this.stages = this.stages.filter((s) => s.id !== id);
    for (const s of this.stages) s.depends_on = s.depends_on.filter((d) => d !== id);
    delete this.positions[id];
    if (this._selectedId === id) this._selectedId = null;
    this._savePositions();
    this._render();
  }

  _renameStage(oldId, newIdRaw) {
    const newId = (newIdRaw || "").trim().toLowerCase().replace(/[^a-z0-9_-]/g, "-");
    if (!newId || newId === oldId) return false;
    if (this.stages.some((s) => s.id === newId)) return false;
    for (const s of this.stages) {
      if (s.id === oldId) s.id = newId;
      s.depends_on = s.depends_on.map((d) => d === oldId ? newId : d);
    }
    if (this.positions[oldId]) {
      this.positions[newId] = this.positions[oldId];
      delete this.positions[oldId];
    }
    if (this._selectedId === oldId) this._selectedId = newId;
    this._savePositions();
    return true;
  }

  _connect(fromId, toId) {
    const dst = this.stages.find((s) => s.id === toId);
    if (!dst) return { ok: false, reason: "unknown target" };
    if (dst.depends_on.includes(fromId)) return { ok: false, reason: "already connected" };
    if (this._reachable(toId, fromId)) return { ok: false, reason: "would create a cycle" };
    dst.depends_on.push(fromId);
    this._render();
    return { ok: true };
  }

  _disconnect(fromId, toId) {
    const dst = this.stages.find((s) => s.id === toId);
    if (!dst) return;
    dst.depends_on = dst.depends_on.filter((d) => d !== fromId);
    this._render();
  }

  _reachable(start, target) {
    // is there a path start → ... → target via depends_on edges (out: dep -> stage)
    const adj = {};
    for (const s of this.stages) {
      if (!s.id) continue;
      for (const dep of s.depends_on) {
        adj[dep] = adj[dep] || [];
        adj[dep].push(s.id);
      }
    }
    const seen = new Set();
    const stack = [start];
    while (stack.length) {
      const n = stack.pop();
      if (n === target) return true;
      if (seen.has(n)) continue;
      seen.add(n);
      for (const next of adj[n] || []) stack.push(next);
    }
    return false;
  }

  // ─── inspector ──────────────────────────────────────────────────────────

  _renderInspector() {
    if (!this._selectedId) {
      const empty = el("aside", "pb-inspector w-72 shrink-0 border-l border-gray-200 bg-gray-50 flex flex-col items-center justify-center text-center px-6");
      const ring = el("div", "w-10 h-10 rounded-full bg-white border border-gray-200 flex items-center justify-center text-gray-400 mb-3");
      ring.append(svgIcon(ICONS.plus, 20, 1.5));
      const t1 = el("p", "text-sm font-medium text-gray-700");
      t1.textContent = "Nothing selected";
      const t2 = el("p", "text-xs text-gray-500 mt-1 leading-relaxed");
      t2.innerHTML = "Click a node to edit it,<br>or double-click the canvas to add a stage.";
      empty.append(ring, t1, t2);
      return empty;
    }
    const stage = this.stages.find((s) => s.id === this._selectedId);
    if (!stage) return null;
    const type = this._stageType(stage.config);
    const style = TYPE_STYLE[type];

    const aside = el("aside", "pb-inspector w-72 shrink-0 border-l border-gray-200 bg-white flex flex-col");
    aside.style.minWidth = "0";

    // Header
    const head = el("div", "px-4 py-3 border-b border-gray-200 flex items-center gap-2.5");
    const iconBox = el("div", "flex items-center justify-center rounded-md shrink-0");
    iconBox.style.width = "28px";
    iconBox.style.height = "28px";
    iconBox.style.background = style.tint;
    iconBox.style.color = style.accent;
    iconBox.append(svgIcon(type, 16, 2));
    head.append(iconBox);
    const headText = el("div", "min-w-0 flex-1");
    const headType = el("div", "text-3xs uppercase tracking-wider font-semibold text-gray-400");
    headType.textContent = `${style.label} Stage`;
    const headId = el("div", "text-sm font-semibold text-gray-900 truncate");
    headId.textContent = stage.id;
    headText.append(headType, headId);
    head.append(headText);
    const del = el("button", "shrink-0 w-7 h-7 inline-flex items-center justify-center rounded text-gray-400 hover:text-red-600 hover:bg-red-50 transition-colors");
    del.type = "button";
    del.title = "Delete stage";
    del.append(svgIcon(ICONS.trash, 14, 2));
    del.onclick = () => this._removeStage(stage.id);
    head.append(del);
    aside.append(head);

    // Body
    const body = el("div", "px-4 py-3 overflow-y-auto flex-1 space-y-3");

    // ID
    body.append(this._field("Stage ID", (() => {
      const inp = el("input", "w-full border border-gray-300 rounded-md px-2.5 py-1.5 text-sm font-mono focus:outline-none focus:ring-2 focus:ring-gray-900 focus:border-gray-900 bg-white");
      inp.type = "text";
      inp.value = stage.id;
      inp.spellcheck = false;
      let pending = stage.id;
      inp.oninput = () => { pending = inp.value; };
      const commit = () => {
        if (pending === stage.id) return;
        if (!this._renameStage(stage.id, pending)) {
          inp.value = stage.id;
          inp.classList.add("border-red-400");
          inp.classList.add("ring-1");
          inp.classList.add("ring-red-300");
          setTimeout(() => {
            inp.classList.remove("border-red-400", "ring-1", "ring-red-300");
          }, 800);
        } else {
          this._render();
        }
      };
      inp.onblur = commit;
      inp.onkeydown = (e) => { if (e.key === "Enter") { e.preventDefault(); inp.blur(); } };
      return inp;
    })(), "Unique identifier · lowercase, dashes, underscores"));

    // Type
    body.append(this._field("Type", (() => {
      const group = el("div", "inline-flex w-full p-0.5 bg-gray-100 rounded-md");
      for (const t of TYPES) {
        const ts = TYPE_STYLE[t];
        const isActive = t === type;
        const b = el("button", `flex-1 text-xs px-2 py-1 rounded inline-flex items-center justify-center gap-1.5 transition-colors ${isActive ? "bg-white text-gray-900 shadow-sm" : "text-gray-500 hover:text-gray-700"}`);
        b.type = "button";
        const iconWrap = el("span", "inline-flex");
        if (isActive) iconWrap.style.color = ts.accent;
        iconWrap.append(svgIcon(t, 12, 2));
        b.append(iconWrap, document.createTextNode(ts.label));
        b.onclick = () => {
          if (t === type) return;
          stage.config = this._defaultConfig(t);
          this._render();
        };
        group.append(b);
      }
      return group;
    })()));

    // Config (type-specific)
    if (type === "deploy") {
      body.append(this._field("Environment", this._textInput(stage.config.Deploy.environment || "", (v) => {
        stage.config.Deploy.environment = v.trim();
        this._sync();
        this._refreshSelectedNode();
      }, "e.g. production")));
    } else if (type === "wait") {
      const inp = this._numInput(stage.config.Wait.duration_seconds || 0, (v) => {
        stage.config.Wait.duration_seconds = v;
        this._sync();
        this._refreshSelectedNode();
      });
      // wrap with seconds suffix
      const wrap = el("div", "relative");
      inp.classList.add("pr-16");
      const suffix = el("span", "absolute right-2.5 top-1/2 -translate-y-1/2 text-xs text-gray-400 pointer-events-none");
      suffix.textContent = "seconds";
      wrap.append(inp, suffix);
      body.append(this._field("Duration", wrap));
    } else if (type === "plan") {
      body.append(this._field("Environment", this._textInput(stage.config.Plan.environment || "", (v) => {
        stage.config.Plan.environment = v.trim();
        this._sync();
        this._refreshSelectedNode();
      }, "e.g. production")));

      const check = el("input", "h-4 w-4 rounded border-gray-300 text-gray-900 focus:ring-gray-900");
      check.type = "checkbox";
      check.checked = !!stage.config.Plan.auto_approve;
      check.onchange = () => {
        stage.config.Plan.auto_approve = check.checked;
        this._sync();
        this._refreshSelectedNode();
      };
      const label = el("label", "flex items-center gap-2 text-sm text-gray-700 cursor-pointer");
      const lt = el("span", "flex flex-col");
      const lt1 = el("span", "text-sm text-gray-900");
      lt1.textContent = "Auto-approve";
      const lt2 = el("span", "text-xs text-gray-500");
      lt2.textContent = "Skip manual approval gate";
      lt.append(lt1, lt2);
      label.append(check, lt);
      body.append(label);
    }

    // Dependencies
    const otherIds = this.stages.map((s) => s.id).filter((id) => id && id !== stage.id);
    if (otherIds.length > 0) {
      const divider = el("div", "border-t border-gray-100 -mx-4");
      body.append(divider);

      const depHead = el("div", "flex items-center justify-between");
      const dl = el("span", "text-2xs uppercase tracking-wider text-gray-500 font-semibold");
      dl.textContent = "Depends on";
      depHead.append(dl);
      if (stage.depends_on.length > 0) {
        const count = el("span", "text-2xs text-gray-400");
        count.textContent = `${stage.depends_on.length} selected`;
        depHead.append(count);
      }
      body.append(depHead);

      const chips = el("div", "flex flex-wrap gap-1");
      for (const id of otherIds) {
        const sel = stage.depends_on.includes(id);
        const otherType = this._stageType((this.stages.find((s) => s.id === id) || {}).config);
        const ts = TYPE_STYLE[otherType];
        const chip = el("button", `text-xs px-2 py-1 rounded-md border inline-flex items-center gap-1.5 transition-colors ${sel ? "bg-gray-900 text-white border-gray-900" : "border-gray-200 bg-white text-gray-700 hover:border-gray-400"}`);
        chip.type = "button";
        const dot = el("span", "inline-block w-1.5 h-1.5 rounded-full shrink-0");
        dot.style.background = sel ? "#ffffff" : ts.accent;
        chip.append(dot, document.createTextNode(id));
        chip.onclick = () => {
          if (sel) {
            this._disconnect(id, stage.id);
          } else {
            const result = this._connect(id, stage.id);
            if (!result.ok) {
              chip.classList.add("border-red-400", "text-red-600");
              chip.title = result.reason;
              setTimeout(() => {
                chip.classList.remove("border-red-400", "text-red-600");
                chip.title = "";
              }, 700);
            }
          }
        };
        chips.append(chip);
      }
      body.append(chips);
    }

    aside.append(body);
    return aside;
  }

  _field(label, control, help) {
    const wrap = el("div", "");
    const l = el("label", "block text-2xs uppercase tracking-wider text-gray-500 font-semibold mb-1.5");
    l.textContent = label;
    wrap.append(l, control);
    if (help) {
      const h = el("p", "text-2xs text-gray-400 mt-1");
      h.textContent = help;
      wrap.append(h);
    }
    return wrap;
  }

  _textInput(value, onCommit, placeholder) {
    const inp = el("input", "w-full border border-gray-300 rounded-md px-2.5 py-1.5 text-sm focus:outline-none focus:ring-2 focus:ring-gray-900 focus:border-gray-900 bg-white");
    inp.type = "text";
    inp.value = value;
    if (placeholder) inp.placeholder = placeholder;
    inp.oninput = () => onCommit(inp.value);
    return inp;
  }

  _numInput(value, onCommit) {
    const inp = el("input", "w-full border border-gray-300 rounded-md px-2.5 py-1.5 text-sm focus:outline-none focus:ring-2 focus:ring-gray-900 focus:border-gray-900 bg-white");
    inp.type = "number";
    inp.min = "0";
    inp.value = value;
    inp.oninput = () => onCommit(parseInt(inp.value) || 0);
    return inp;
  }

  _refreshSelectedNode() {
    // Light in-place update of the selected node's sub-label, so the
    // inspector input keeps focus while the user types. Triggers a
    // full re-render as a fallback if the inline structure changed.
    if (!this._selectedId) return;
    const node = this.querySelector(`.pb-node[data-id="${cssEsc(this._selectedId)}"]`);
    if (!node) return;
    const stage = this.stages.find((s) => s.id === this._selectedId);
    if (!stage) return;
    const topRow = node.querySelector(".pb-node-top");
    if (!topRow) return;
    const label = this._configLabel(stage.config);
    // topRow children: [typeLabel, (dot), (sub)]
    // remove any existing dot+sub
    while (topRow.children.length > 1) topRow.removeChild(topRow.lastElementChild);
    if (label) {
      const dot = el("span", "text-gray-300");
      dot.textContent = "·";
      const sub = el("span", "text-3xs font-mono text-gray-500 truncate");
      sub.style.maxWidth = "100px";
      sub.textContent = label;
      sub.title = label;
      topRow.append(dot, sub);
    }
  }
}

function el(tag, className, text) {
  const e = document.createElement(tag);
  if (className) e.className = className;
  if (text) e.textContent = text;
  return e;
}

function cssEsc(s) {
  if (window.CSS && CSS.escape) return CSS.escape(s);
  return String(s).replace(/[^a-zA-Z0-9_-]/g, "\\$&");
}

function bezier(from, to, ox, oy) {
  const x1 = from.x + ox;
  const y1 = from.y + oy;
  const x2 = to.x + ox;
  const y2 = to.y + oy;
  const dx = Math.max(40, Math.abs(x2 - x1) * 0.5);
  return `M ${x1} ${y1} C ${x1 + dx} ${y1}, ${x2 - dx} ${y2}, ${x2} ${y2}`;
}

customElements.define("pipeline-builder", PipelineBuilder);
