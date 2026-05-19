/**
 * <pipeline-builder> web component
 *
 * Visual DAG builder for release pipeline stages.
 * Syncs to a hidden textarea (data-target) as JSON.
 *
 * Stage format (matches Rust serde of PipelineStage):
 *   { "id": "stage-name", "depends_on": ["other"], "config": {"Deploy": {"environment": "prod"}} }
 *
 * Usage:
 *   <pipeline-builder data-target="pipeline-stages"></pipeline-builder>
 *   <textarea id="pipeline-stages" name="stages_json" hidden></textarea>
 */

class PipelineBuilder extends HTMLElement {
  connectedCallback() {
    this.stages = [];
    this._targetId = this.dataset.target;
    this._readonly = this.dataset.readonly === "true";
    this._mode = "builder"; // "builder" | "json"

    // Load initial value from target textarea
    const target = this._target();
    if (target && target.value.trim()) {
      try {
        const parsed = JSON.parse(target.value.trim());
        this.stages = this._parseStages(parsed);
      } catch (e) {
        this._rawJson = target.value.trim();
      }
    }

    this._render();
  }

  _target() {
    return this._targetId ? document.getElementById(this._targetId) : null;
  }

  // Extract the stage type string from a config object
  _stageType(config) {
    if (!config) return "deploy";
    if (config.Deploy !== undefined) return "deploy";
    if (config.Wait !== undefined) return "wait";
    if (config.Plan !== undefined) return "plan";
    return "deploy";
  }

  // Extract display info from config
  _configLabel(config) {
    if (!config) return "";
    if (config.Deploy) return config.Deploy.environment || "";
    if (config.Wait) return config.Wait.duration_seconds ? `${config.Wait.duration_seconds}s` : "";
    if (config.Plan) return config.Plan.environment || "";
    return "";
  }

  _normalizeStage(s) {
    // Handle the new typed format: {id, depends_on, config: {Deploy: {environment}}}
    if (s.id !== undefined) {
      return {
        id: s.id || "",
        depends_on: Array.isArray(s.depends_on) ? s.depends_on : [],
        config: s.config || { Deploy: { environment: "" } },
      };
    }
    // Legacy format: {name, type, depends_on}
    const type = s.type || "deploy";
    const config = type === "wait"
      ? { Wait: { duration_seconds: s.duration_seconds || 0 } }
      : { Deploy: { environment: s.environment || "" } };
    return {
      id: s.name || "",
      depends_on: Array.isArray(s.depends_on) ? s.depends_on : [],
      config,
    };
  }

  _parseStages(parsed) {
    if (Array.isArray(parsed)) {
      return parsed.map((s) => this._normalizeStage(s));
    }
    if (parsed.stages && Array.isArray(parsed.stages)) {
      return parsed.stages.map((s) => this._normalizeStage(s));
    }
    // Map format: { "id": { depends_on, config } }
    if (typeof parsed === "object" && parsed !== null) {
      return Object.entries(parsed).map(([id, val]) =>
        this._normalizeStage({ id, ...val })
      );
    }
    return [];
  }

  _sync() {
    const target = this._target();
    if (!target) return;
    if (this.stages.length === 0) {
      target.value = "";
      return;
    }
    // Filter out stages with no id
    const valid = this.stages.filter((s) => s.id.trim());
    target.value = JSON.stringify(valid, null, 2);
  }

  _validate() {
    const ids = this.stages.map((s) => s.id).filter(Boolean);
    const idSet = new Set(ids);
    const errors = [];

    if (ids.length !== idSet.size) {
      errors.push("Duplicate stage IDs detected");
    }

    for (const s of this.stages) {
      for (const dep of s.depends_on) {
        if (!idSet.has(dep)) {
          errors.push(`"${s.id}" depends on unknown stage "${dep}"`);
        }
      }
    }

    // Cycle detection (Kahn's algorithm)
    const inDegree = {};
    const adj = {};
    for (const s of this.stages) {
      if (!s.id) continue;
      inDegree[s.id] = 0;
      adj[s.id] = [];
    }
    for (const s of this.stages) {
      if (!s.id) continue;
      for (const dep of s.depends_on) {
        if (adj[dep]) {
          adj[dep].push(s.id);
          inDegree[s.id]++;
        }
      }
    }
    const queue = Object.keys(inDegree).filter((k) => inDegree[k] === 0);
    let visited = 0;
    while (queue.length > 0) {
      const node = queue.shift();
      visited++;
      for (const next of adj[node] || []) {
        inDegree[next]--;
        if (inDegree[next] === 0) queue.push(next);
      }
    }
    if (visited < Object.keys(inDegree).length) {
      errors.push("Cycle detected in stage dependencies");
    }

    for (let i = 0; i < this.stages.length; i++) {
      if (!this.stages[i].id.trim()) {
        errors.push(`Stage ${i + 1} has no ID`);
      }
    }

    return errors;
  }

  _computeLevels() {
    const byId = {};
    for (const s of this.stages) {
      if (s.id) byId[s.id] = s;
    }
    const levels = {};
    const visited = new Set();

    const getLevel = (id) => {
      if (levels[id] !== undefined) return levels[id];
      if (visited.has(id)) return 0;
      visited.add(id);
      const s = byId[id];
      if (!s || s.depends_on.length === 0) {
        levels[id] = 0;
        return 0;
      }
      let maxDep = 0;
      for (const dep of s.depends_on) {
        if (byId[dep]) {
          maxDep = Math.max(maxDep, getLevel(dep) + 1);
        }
      }
      levels[id] = maxDep;
      return maxDep;
    };

    for (const s of this.stages) {
      if (s.id) getLevel(s.id);
    }
    return levels;
  }

  _render() {
    const errors = this._validate();
    if (!this._readonly) this._sync();

    this.innerHTML = "";
    this.className = "block";

    // Readonly mode: just show the DAG
    if (this._readonly) {
      if (this.stages.length > 0) {
        const canvas = el("div", "dag-canvas overflow-x-auto");
        this._renderDag(canvas);
        this.append(canvas);
      } else {
        this.append(el("p", "text-xs text-gray-400 italic", "No stages defined"));
      }
      return;
    }

    // Mode toggle
    const toolbar = el("div", "flex items-center gap-2 mb-3");
    const builderBtn = el(
      "button",
      `text-xs px-2.5 py-1 rounded border ${this._mode === "builder" ? "bg-gray-900 text-white border-gray-900" : "border-gray-300 text-gray-600 hover:bg-gray-50"}`,
      "Builder"
    );
    builderBtn.type = "button";
    builderBtn.onclick = () => {
      if (this._mode === "json") {
        const ta = this.querySelector(".json-editor");
        if (ta) {
          try {
            const parsed = JSON.parse(ta.value);
            this.stages = this._parseStages(parsed);
            this._rawJson = null;
          } catch (e) {
            this._rawJson = ta.value;
          }
        }
        this._mode = "builder";
        this._render();
      }
    };
    const jsonBtn = el(
      "button",
      `text-xs px-2.5 py-1 rounded border ${this._mode === "json" ? "bg-gray-900 text-white border-gray-900" : "border-gray-300 text-gray-600 hover:bg-gray-50"}`,
      "JSON"
    );
    jsonBtn.type = "button";
    jsonBtn.onclick = () => {
      this._mode = "json";
      this._render();
    };
    toolbar.append(builderBtn, jsonBtn);

    if (this._mode === "builder" && this.stages.length > 0) {
      const stageCount = el("span", "text-xs text-gray-400 ml-auto", `${this.stages.length} stage${this.stages.length !== 1 ? "s" : ""}`);
      toolbar.append(stageCount);
    }

    this.append(toolbar);

    if (this._mode === "json") {
      this._renderJsonMode();
    } else {
      this._renderBuilderMode(errors);
    }
  }

  _renderJsonMode() {
    const target = this._target();
    const currentJson = this._rawJson || (target ? target.value : "") || "[]";

    const ta = el("textarea", "json-editor w-full border border-gray-300 rounded-md px-3 py-2 text-sm font-mono focus:outline-none focus:ring-2 focus:ring-gray-900 resize-y");
    ta.rows = 12;
    ta.value = currentJson;
    ta.spellcheck = false;
    ta.oninput = () => {
      const t = this._target();
      if (t) t.value = ta.value;
      this._updateJsonErrors(ta.value);
    };

    const errBox = el("div", "json-errors mt-2");
    this.append(ta, errBox);
    this._updateJsonErrors(currentJson);
  }

  _updateJsonErrors(value) {
    const errBox = this.querySelector(".json-errors");
    if (!errBox) return;
    errBox.innerHTML = "";
    if (!value.trim()) return;
    try {
      const parsed = JSON.parse(value);
      const stages = Array.isArray(parsed) ? parsed : (parsed.stages || []);
      const ids = stages.map((s) => s.id || s.name).filter(Boolean);
      if (new Set(ids).size !== ids.length) {
        errBox.append(el("p", "text-xs text-amber-600", "Warning: duplicate stage IDs"));
      }
    } catch (e) {
      errBox.append(el("p", "text-xs text-red-600", "Invalid JSON: " + e.message));
    }
  }

  _renderBuilderMode(errors) {
    if (this.stages.length > 0) {
      const dagBox = el("div", "mb-4 border border-gray-200 rounded-lg overflow-hidden");
      const canvas = el("div", "dag-canvas p-4 bg-gray-50 overflow-x-auto");
      canvas.style.minHeight = "80px";
      this._renderDag(canvas);
      dagBox.append(canvas);
      this.append(dagBox);
    }

    const list = el("div", "space-y-2 mb-3");
    for (let i = 0; i < this.stages.length; i++) {
      list.append(this._renderStageCard(i));
    }
    this.append(list);

    if (errors.length > 0) {
      const errBox = el("div", "mb-3 p-3 bg-red-50 border border-red-200 rounded-md");
      for (const err of errors) {
        errBox.append(el("p", "text-xs text-red-700", err));
      }
      this.append(errBox);
    }

    const addBtn = el("button", "text-sm px-3 py-1.5 rounded border border-dashed border-gray-300 text-gray-500 hover:border-gray-400 hover:text-gray-700 w-full", "+ Add stage");
    addBtn.type = "button";
    addBtn.onmousedown = (e) => e.preventDefault();
    addBtn.onclick = () => {
      clearTimeout(this._blurTimer);
      this.stages.push({ id: "", depends_on: [], config: { Deploy: { environment: "" } } });
      this._render();
      requestAnimationFrame(() => {
        const inputs = this.querySelectorAll('input[data-field="id"]');
        if (inputs.length) inputs[inputs.length - 1].focus();
      });
    };
    this.append(addBtn);
  }

  _renderStageCard(index) {
    const stage = this.stages[index];
    const type = this._stageType(stage.config);
    const otherIds = this.stages
      .map((s, i) => (i !== index && s.id.trim() ? s.id.trim() : null))
      .filter(Boolean);

    const card = el("div", "border border-gray-200 rounded-md bg-white");

    // Header row
    const header = el("div", "flex items-center gap-2 px-3 py-2");
    const badge = el("span", "text-xs font-mono text-gray-400 w-5 shrink-0", `${index + 1}`);

    // ID input
    const idInput = el("input", "flex-1 border border-gray-200 rounded px-2 py-1 text-sm focus:outline-none focus:ring-1 focus:ring-gray-400 min-w-0");
    idInput.type = "text";
    idInput.value = stage.id;
    idInput.placeholder = "stage id";
    idInput.dataset.field = "id";
    idInput.oninput = () => {
      this.stages[index].id = idInput.value.trim().toLowerCase().replace(/[^a-z0-9_-]/g, "-");
      idInput.value = this.stages[index].id;
      this._sync();
      this._renderDagIfPresent();
    };
    idInput.onblur = () => {
      this._blurTimer = setTimeout(() => this._render(), 150);
    };

    // Type select (deploy / wait)
    const typeSelect = el("select", "border border-gray-200 rounded px-2 py-1 text-xs bg-white shrink-0");
    for (const t of ["deploy", "wait", "plan"]) {
      const opt = document.createElement("option");
      opt.value = t;
      opt.textContent = t;
      opt.selected = type === t;
      typeSelect.append(opt);
    }
    typeSelect.onmousedown = (e) => e.stopPropagation();
    typeSelect.onchange = () => {
      clearTimeout(this._blurTimer);
      if (typeSelect.value === "wait") {
        this.stages[index].config = { Wait: { duration_seconds: 0 } };
      } else if (typeSelect.value === "plan") {
        this.stages[index].config = { Plan: { environment: "", auto_approve: false } };
      } else {
        this.stages[index].config = { Deploy: { environment: "" } };
      }
      this._render();
    };

    // Remove button
    const removeBtn = el("button", "text-gray-400 hover:text-red-500 shrink-0 p-1");
    removeBtn.type = "button";
    removeBtn.innerHTML = `<svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"/></svg>`;
    removeBtn.title = "Remove stage";
    removeBtn.onmousedown = (e) => e.preventDefault();
    removeBtn.onclick = () => {
      clearTimeout(this._blurTimer);
      const removedId = this.stages[index].id;
      this.stages.splice(index, 1);
      for (const s of this.stages) {
        s.depends_on = s.depends_on.filter((d) => d !== removedId);
      }
      this._render();
    };

    header.append(badge, idInput, typeSelect, removeBtn);
    card.append(header);

    // Config row (type-specific fields)
    const configRow = el("div", "px-3 pb-2 flex items-center gap-2 flex-wrap");
    if (type === "deploy") {
      const envLabel = el("span", "text-xs text-gray-500 shrink-0", "env:");
      const envInput = el("input", "border border-gray-200 rounded px-2 py-1 text-xs w-32 focus:outline-none focus:ring-1 focus:ring-gray-400");
      envInput.type = "text";
      envInput.value = (stage.config.Deploy && stage.config.Deploy.environment) || "";
      envInput.placeholder = "environment";
      envInput.onmousedown = (e) => e.stopPropagation();
      envInput.oninput = () => {
        if (!this.stages[index].config.Deploy) this.stages[index].config = { Deploy: { environment: "" } };
        this.stages[index].config.Deploy.environment = envInput.value.trim();
        this._sync();
      };
      envInput.onblur = () => {
        this._blurTimer = setTimeout(() => this._render(), 150);
      };
      configRow.append(envLabel, envInput);
    } else if (type === "wait") {
      const durLabel = el("span", "text-xs text-gray-500 shrink-0", "wait:");
      const durInput = el("input", "border border-gray-200 rounded px-2 py-1 text-xs w-20 focus:outline-none focus:ring-1 focus:ring-gray-400");
      durInput.type = "number";
      durInput.min = "0";
      durInput.value = (stage.config.Wait && stage.config.Wait.duration_seconds) || 0;
      durInput.placeholder = "seconds";
      durInput.onmousedown = (e) => e.stopPropagation();
      durInput.oninput = () => {
        if (!this.stages[index].config.Wait) this.stages[index].config = { Wait: { duration_seconds: 0 } };
        this.stages[index].config.Wait.duration_seconds = parseInt(durInput.value) || 0;
        this._sync();
      };
      durInput.onblur = () => {
        this._blurTimer = setTimeout(() => this._render(), 150);
      };
      const secLabel = el("span", "text-xs text-gray-400", "seconds");
      configRow.append(durLabel, durInput, secLabel);
    } else if (type === "plan") {
      const envLabel = el("span", "text-xs text-gray-500 shrink-0", "env:");
      const envInput = el("input", "border border-gray-200 rounded px-2 py-1 text-xs w-32 focus:outline-none focus:ring-1 focus:ring-gray-400");
      envInput.type = "text";
      envInput.value = (stage.config.Plan && stage.config.Plan.environment) || "";
      envInput.placeholder = "environment";
      envInput.onmousedown = (e) => e.stopPropagation();
      envInput.oninput = () => {
        if (!this.stages[index].config.Plan) this.stages[index].config = { Plan: { environment: "", auto_approve: false } };
        this.stages[index].config.Plan.environment = envInput.value.trim();
        this._sync();
      };
      envInput.onblur = () => {
        this._blurTimer = setTimeout(() => this._render(), 150);
      };
      const autoLabel = el("label", "text-xs text-gray-500 flex items-center gap-1 ml-2 shrink-0");
      const autoCheck = el("input", "");
      autoCheck.type = "checkbox";
      autoCheck.checked = !!(stage.config.Plan && stage.config.Plan.auto_approve);
      autoCheck.onmousedown = (e) => e.stopPropagation();
      autoCheck.onchange = () => {
        if (!this.stages[index].config.Plan) this.stages[index].config = { Plan: { environment: "", auto_approve: false } };
        this.stages[index].config.Plan.auto_approve = autoCheck.checked;
        this._sync();
      };
      autoLabel.append(autoCheck, document.createTextNode("auto-approve"));
      configRow.append(envLabel, envInput, autoLabel);
    }
    card.append(configRow);

    // Dependencies row
    if (otherIds.length > 0) {
      const depsRow = el("div", "px-3 pb-2 flex items-center gap-2 flex-wrap");
      const label = el("span", "text-xs text-gray-500 shrink-0", "after:");
      depsRow.append(label);

      for (const dep of otherIds) {
        const isSelected = stage.depends_on.includes(dep);
        const chip = el(
          "button",
          `text-xs px-2 py-0.5 rounded-full border transition-colors ${isSelected ? "bg-gray-900 text-white border-gray-900" : "border-gray-300 text-gray-500 hover:border-gray-400"}`,
          dep
        );
        chip.type = "button";
        chip.onmousedown = (e) => e.preventDefault();
        chip.onclick = () => {
          clearTimeout(this._blurTimer);
          if (isSelected) {
            this.stages[index].depends_on = this.stages[index].depends_on.filter((d) => d !== dep);
          } else {
            this.stages[index].depends_on.push(dep);
          }
          this._render();
        };
        depsRow.append(chip);
      }
      card.append(depsRow);
    }

    return card;
  }

  _renderDagIfPresent() {
    const canvas = this.querySelector(".dag-canvas");
    if (canvas) this._renderDag(canvas);
  }

  _renderDag(canvas) {
    canvas.innerHTML = "";
    const named = this.stages.filter((s) => s.id.trim());
    if (named.length === 0) {
      canvas.append(el("p", "text-xs text-gray-400 italic", "Add stages to see the pipeline graph"));
      return;
    }

    const levels = this._computeLevels();
    const maxLevel = Math.max(0, ...Object.values(levels));

    const columns = [];
    for (let l = 0; l <= maxLevel; l++) columns.push([]);
    for (const s of named) {
      const lvl = levels[s.id] || 0;
      columns[lvl].push(s);
    }

    const svgNS = "http://www.w3.org/2000/svg";
    const NODE_W = 120;
    const NODE_H = 40;
    const COL_GAP = 60;
    const ROW_GAP = 12;

    const positions = {};
    let totalW = 0;
    let totalH = 0;

    for (let col = 0; col <= maxLevel; col++) {
      const stages = columns[col];
      for (let row = 0; row < stages.length; row++) {
        const x = col * (NODE_W + COL_GAP);
        const y = row * (NODE_H + ROW_GAP);
        positions[stages[row].id] = { x, y };
        totalW = Math.max(totalW, x + NODE_W);
        totalH = Math.max(totalH, y + NODE_H);
      }
    }

    const PAD = 8;
    const svgW = totalW + PAD * 2;
    const svgH = totalH + PAD * 2;

    const svg = document.createElementNS(svgNS, "svg");
    svg.setAttribute("width", svgW);
    svg.setAttribute("height", svgH);
    svg.style.display = "block";

    // Arrowhead marker
    const defs = document.createElementNS(svgNS, "defs");
    const marker = document.createElementNS(svgNS, "marker");
    marker.setAttribute("id", "pb-arrow");
    marker.setAttribute("viewBox", "0 0 10 10");
    marker.setAttribute("refX", "10");
    marker.setAttribute("refY", "5");
    marker.setAttribute("markerWidth", "6");
    marker.setAttribute("markerHeight", "6");
    marker.setAttribute("orient", "auto-start-reverse");
    const arrowPath = document.createElementNS(svgNS, "path");
    arrowPath.setAttribute("d", "M 0 0 L 10 5 L 0 10 z");
    arrowPath.setAttribute("fill", "#9ca3af");
    marker.append(arrowPath);
    defs.append(marker);
    svg.append(defs);

    // Draw edges
    for (const s of named) {
      const to = positions[s.id];
      if (!to) continue;
      for (const dep of s.depends_on) {
        const from = positions[dep];
        if (!from) continue;
        const line = document.createElementNS(svgNS, "line");
        line.setAttribute("x1", from.x + NODE_W + PAD);
        line.setAttribute("y1", from.y + NODE_H / 2 + PAD);
        line.setAttribute("x2", to.x + PAD);
        line.setAttribute("y2", to.y + NODE_H / 2 + PAD);
        line.setAttribute("stroke", "#d1d5db");
        line.setAttribute("stroke-width", "2");
        line.setAttribute("marker-end", "url(#pb-arrow)");
        svg.append(line);
      }
    }

    // Draw nodes
    const TYPE_COLORS = {
      deploy: { bg: "#dbeafe", border: "#93c5fd", text: "#1e40af" },
      wait: { bg: "#fef3c7", border: "#fcd34d", text: "#92400e" },
      plan: { bg: "#ede9fe", border: "#c4b5fd", text: "#5b21b6" },
    };

    for (const s of named) {
      const pos = positions[s.id];
      if (!pos) continue;
      const type = this._stageType(s.config);
      const colors = TYPE_COLORS[type] || TYPE_COLORS.deploy;
      const label = this._configLabel(s.config);

      const rect = document.createElementNS(svgNS, "rect");
      rect.setAttribute("x", pos.x + PAD);
      rect.setAttribute("y", pos.y + PAD);
      rect.setAttribute("width", NODE_W);
      rect.setAttribute("height", NODE_H);
      rect.setAttribute("rx", "6");
      rect.setAttribute("fill", colors.bg);
      rect.setAttribute("stroke", colors.border);
      rect.setAttribute("stroke-width", "1.5");
      svg.append(rect);

      // Stage ID text
      const text = document.createElementNS(svgNS, "text");
      text.setAttribute("x", pos.x + NODE_W / 2 + PAD);
      text.setAttribute("y", pos.y + NODE_H / 2 + PAD + (label ? -4 : 0));
      text.setAttribute("text-anchor", "middle");
      text.setAttribute("dominant-baseline", "middle");
      text.setAttribute("fill", colors.text);
      text.setAttribute("font-size", "12");
      text.setAttribute("font-weight", "600");
      text.textContent = s.id.length > 14 ? s.id.slice(0, 13) + "…" : s.id;
      svg.append(text);

      // Config label (environment or duration)
      if (label) {
        const sub = document.createElementNS(svgNS, "text");
        sub.setAttribute("x", pos.x + NODE_W / 2 + PAD);
        sub.setAttribute("y", pos.y + NODE_H / 2 + 10 + PAD);
        sub.setAttribute("text-anchor", "middle");
        sub.setAttribute("dominant-baseline", "middle");
        sub.setAttribute("fill", colors.text);
        sub.setAttribute("font-size", "9");
        sub.setAttribute("opacity", "0.7");
        sub.textContent = label;
        svg.append(sub);
      }
    }

    canvas.append(svg);
  }
}

function el(tag, className, text) {
  const e = document.createElement(tag);
  if (className) e.className = className;
  if (text) e.textContent = text;
  return e;
}

customElements.define("pipeline-builder", PipelineBuilder);
