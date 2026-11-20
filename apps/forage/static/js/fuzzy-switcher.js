const SCORE_MATCH = 16;
const SCORE_GAP_START = -3;
const SCORE_GAP_EXTENSION = -1;

const BONUS_BOUNDARY = SCORE_MATCH / 2;
const BONUS_NON_WORD = SCORE_MATCH / 2;
const BONUS_BOUNDARY_WHITE = BONUS_BOUNDARY + 2;
const BONUS_BOUNDARY_DELIMITER = BONUS_BOUNDARY + 1;
const BONUS_CAMEL_123 = BONUS_BOUNDARY + SCORE_GAP_EXTENSION;
const BONUS_CONSECUTIVE = -(SCORE_GAP_START + SCORE_GAP_EXTENSION);
const BONUS_FIRST_CHAR_MULTIPLIER = 2;

const CLASS_WHITE = 0;
const CLASS_NON_WORD = 1;
const CLASS_DELIMITER = 2;
const CLASS_LOWER = 3;
const CLASS_UPPER = 4;
const CLASS_LETTER = 5;
const CLASS_NUMBER = 6;

const DELIMITERS = "/,:;|";

function charClass(ch) {
  if (ch >= "a" && ch <= "z") return CLASS_LOWER;
  if (ch >= "A" && ch <= "Z") return CLASS_UPPER;
  if (ch >= "0" && ch <= "9") return CLASS_NUMBER;
  if (ch === " " || ch === "\t" || ch === "\n" || ch === "\r") return CLASS_WHITE;
  if (DELIMITERS.includes(ch)) return CLASS_DELIMITER;
  if (ch.toLowerCase() !== ch.toUpperCase()) return CLASS_LETTER;
  return CLASS_NON_WORD;
}

function bonusFor(prevClass, cls) {
  if (cls >= CLASS_NON_WORD) {
    if (prevClass === CLASS_WHITE) return BONUS_BOUNDARY_WHITE;
    if (prevClass === CLASS_DELIMITER) return BONUS_BOUNDARY_DELIMITER;
    if (prevClass === CLASS_NON_WORD) return BONUS_BOUNDARY;
  }
  if (
    (prevClass === CLASS_LOWER && cls === CLASS_UPPER) ||
    (prevClass !== CLASS_NUMBER && cls === CLASS_NUMBER)
  ) {
    return BONUS_CAMEL_123;
  }
  if (cls === CLASS_NON_WORD || cls === CLASS_DELIMITER) return BONUS_NON_WORD;
  if (cls === CLASS_WHITE) return BONUS_BOUNDARY_WHITE;
  return 0;
}

function bonusTable(text) {
  const bonus = new Int32Array(text.length);
  let prev = CLASS_WHITE;
  for (let i = 0; i < text.length; i++) {
    const cls = charClass(text[i]);
    bonus[i] = bonusFor(prev, cls);
    prev = cls;
  }
  return bonus;
}

export function fuzzyMatch(query, text) {
  const original = String(text);
  const pattern = String(query).trim().toLowerCase();
  if (!pattern) return { score: 0, positions: [] };

  const lower = original.toLowerCase();
  const m = pattern.length;
  const n = lower.length;
  if (m > n) return null;

  const first = new Int32Array(m);
  let from = 0;
  for (let i = 0; i < m; i++) {
    const at = lower.indexOf(pattern[i], from);
    if (at === -1) return null;
    first[i] = at;
    from = at + 1;
  }

  const last = lower.lastIndexOf(pattern[m - 1]);

  const bonus = bonusTable(original);
  const h = [];
  const c = [];
  for (let i = 0; i < m; i++) {
    h.push(new Int32Array(n));
    c.push(new Int32Array(n));
  }

  {
    const pchar = pattern[0];
    let inGap = false;
    for (let j = first[0]; j <= last; j++) {
      if (pchar === lower[j]) {
        h[0][j] = SCORE_MATCH + bonus[j] * BONUS_FIRST_CHAR_MULTIPLIER;
        c[0][j] = 1;
        inGap = false;
      } else {
        const carried =
          (j > first[0] ? h[0][j - 1] : 0) +
          (inGap ? SCORE_GAP_EXTENSION : SCORE_GAP_START);
        h[0][j] = Math.max(carried, 0);
        inGap = true;
      }
    }
  }

  for (let i = 1; i < m; i++) {
    const pchar = pattern[i];
    const start = first[i];
    let inGap = false;
    for (let j = start; j <= last; j++) {
      const gapScore =
        j > start ? h[i][j - 1] + (inGap ? SCORE_GAP_EXTENSION : SCORE_GAP_START) : 0;

      let matchScore = 0;
      let consecutive = 0;
      if (pchar === lower[j]) {
        matchScore = h[i - 1][j - 1] + SCORE_MATCH;

        let b = bonus[j];
        consecutive = c[i - 1][j - 1] + 1;
        if (consecutive > 1) {
          const runStart = bonus[j - consecutive + 1];
          if (b >= BONUS_BOUNDARY && b > runStart) {
            consecutive = 1;
          } else {
            b = Math.max(b, Math.max(BONUS_CONSECUTIVE, runStart));
          }
        }

        if (matchScore + b < gapScore) {
          matchScore += bonus[j];
          consecutive = 0;
        } else {
          matchScore += b;
        }
      }

      c[i][j] = consecutive;
      inGap = matchScore < gapScore;
      h[i][j] = Math.max(matchScore, gapScore, 0);
    }
  }

  let score = 0;
  let end = first[m - 1];
  for (let j = first[m - 1]; j <= last; j++) {
    if (h[m - 1][j] > score) {
      score = h[m - 1][j];
      end = j;
    }
  }

  const positions = [];
  let i = m - 1;
  let j = end;
  let preferMatch = true;
  while (j >= 0) {
    const row = i;
    const here = h[row][j];
    const diag = row > 0 && j > 0 && j >= first[row] ? h[row - 1][j - 1] : 0;
    const leftward = j > first[row] ? h[row][j - 1] : 0;

    if (here > diag && (here > leftward || (here === leftward && preferMatch))) {
      positions.push(j);
      if (row === 0) break;
      i--;
    }
    preferMatch =
      c[row][j] > 1 || (row + 1 < m && j + 1 <= last && c[row + 1][j + 1] > 0);
    j--;
  }
  positions.reverse();

  if (positions.length !== m) return { score, positions: Array.from(first) };

  return { score, positions };
}

export function rankMatches(query, items) {
  const scored = [];
  for (let index = 0; index < items.length; index++) {
    const label = items[index];
    const match = fuzzyMatch(query, label);
    if (match) scored.push({ index, label, ...match });
  }
  if (!String(query).trim()) return scored;

  scored.sort(
    (a, b) => b.score - a.score || a.label.length - b.label.length || a.index - b.index,
  );
  return scored;
}

const DEFAULT_LIMIT = 10;
const DEFAULT_MIN_ITEMS = 8;

let uid = 0;

const toInt = (value, fallback) => {
  const n = Number.parseInt(value, 10);
  return Number.isFinite(n) ? n : fallback;
};

function paint(link, label, positions) {
  link.textContent = "";
  if (!positions.length) {
    link.textContent = label;
    return;
  }
  let at = 0;
  for (let k = 0; k < positions.length; ) {
    let end = k;
    while (end + 1 < positions.length && positions[end + 1] === positions[end] + 1) end++;
    const from = positions[k];
    const to = positions[end] + 1;
    if (from > at) link.appendChild(document.createTextNode(label.slice(at, from)));
    const mark = document.createElement("mark");
    mark.textContent = label.slice(from, to);
    link.appendChild(mark);
    at = to;
    k = end + 1;
  }
  if (at < label.length) link.appendChild(document.createTextNode(label.slice(at)));
}

const bindings = new WeakMap();

function enhance(details) {
  const list = details.querySelector("[data-fuzzy-list]");
  if (!list || list.dataset.fuzzyReady) return;

  const links = Array.from(list.querySelectorAll(":scope > a"));
  if (links.length < toInt(details.dataset.fuzzyMin, DEFAULT_MIN_ITEMS)) return;

  list.dataset.fuzzyReady = "1";
  bindings.get(details)?.abort();
  const controller = new AbortController();
  bindings.set(details, controller);
  const { signal } = controller;
  const limit = toInt(details.dataset.fuzzyLimit, DEFAULT_LIMIT);
  const labels = links.map((a) => a.textContent.trim());
  const id = `fuzzy-${++uid}`;
  const placeholder = details.dataset.fuzzyLabel || "Search…";

  const field = document.createElement("div");
  field.className = "px-2 pt-2 pb-1.5 border-b border-gray-100";

  const input = document.createElement("input");
  input.type = "text";
  input.className =
    "w-full px-2 py-1 text-sm bg-white text-gray-900 border border-gray-200 rounded " +
    "placeholder:text-gray-400 focus:outline-none focus:border-gray-400";
  input.placeholder = placeholder;
  input.autocomplete = "off";
  input.spellcheck = false;
  input.setAttribute("aria-label", placeholder);
  input.setAttribute("role", "combobox");
  input.setAttribute("aria-expanded", "true");
  input.setAttribute("aria-controls", id);
  input.setAttribute("aria-autocomplete", "list");
  field.appendChild(input);

  const empty = document.createElement("div");
  empty.className = "px-3 py-2 text-sm text-gray-400";
  empty.textContent = "No matches";
  empty.hidden = true;

  const more = document.createElement("div");
  more.className = "px-3 py-1.5 text-xs text-gray-400 border-t border-gray-100";
  more.hidden = true;

  list.id = id;
  list.setAttribute("role", "listbox");
  links.forEach((a, i) => {
    a.id = `${id}-opt-${i}`;
    a.setAttribute("role", "option");
    a.setAttribute("aria-selected", "false");
  });

  list.before(field);
  list.after(empty, more);

  let visible = links;
  let active = -1;
  let activeEl = null;

  function setActive(next) {
    if (activeEl) {
      activeEl.removeAttribute("data-fuzzy-active");
      activeEl.setAttribute("aria-selected", "false");
    }
    active = next;
    activeEl = visible[active] ?? null;
    if (!activeEl) {
      input.removeAttribute("aria-activedescendant");
      return;
    }
    activeEl.setAttribute("data-fuzzy-active", "");
    activeEl.setAttribute("aria-selected", "true");
    input.setAttribute("aria-activedescendant", activeEl.id);
    activeEl.scrollIntoView({ block: "nearest" });
  }

  function apply() {
    const query = input.value;
    const ranked = rankMatches(query, labels);
    const shown = query.trim() ? ranked.slice(0, limit) : ranked;

    for (const a of links) a.hidden = true;
    for (const match of shown) {
      const a = links[match.index];
      a.hidden = false;
      paint(a, match.label, match.positions);
      list.appendChild(a);
    }
    visible = shown.map((match) => links[match.index]);

    empty.hidden = visible.length > 0;
    const rest = ranked.length - shown.length;
    more.hidden = rest <= 0;
    if (rest > 0) more.textContent = `+${rest} more — keep typing`;

    if (!visible.length) setActive(-1);
    else if (query.trim()) setActive(0);
    else setActive(Math.max(0, visible.findIndex((a) => a.hasAttribute("aria-current"))));
  }

  function move(step) {
    if (!visible.length) return;
    setActive((active + step + visible.length) % visible.length);
  }

  const summary = details.querySelector("summary");

  function close() {
    details.open = false;
    summary?.focus();
  }

  function opened() {
    input.value = "";
    apply();
    input.focus({ preventScroll: true });
  }

  input.addEventListener("input", apply);

  details.addEventListener("keydown", (event) => {
    if (event.defaultPrevented) return;
    switch (event.key) {
      case "Escape":
        event.preventDefault();
        close();
        break;
      case "ArrowDown":
        event.preventDefault();
        move(1);
        break;
      case "ArrowUp":
        event.preventDefault();
        move(-1);
        break;
      case "Enter": {
        const target = visible[active];
        if (!target) break;
        event.preventDefault();
        window.location.assign(target.href);
        break;
      }
      default:
        break;
    }
  }, { signal });

  details.addEventListener("toggle", () => {
    if (details.open) opened();
  }, { signal });

  summary?.addEventListener("click", () => {
    requestAnimationFrame(() => {
      if (details.open) opened();
    });
  }, { signal });

  apply();
}

export function initFuzzySwitchers(root = document) {
  root.querySelectorAll("details[data-fuzzy-switcher]").forEach(enhance);
}

if (typeof document !== "undefined") {
  initFuzzySwitchers();
  document.addEventListener("sedx:load", () => initFuzzySwitchers());
}
