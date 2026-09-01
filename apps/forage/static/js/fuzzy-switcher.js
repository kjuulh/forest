/**
 * Type-to-filter for the breadcrumb switchers.
 *
 * The project switcher lists every project in the org — thirty-odd entries for
 * understory — as one unfiltered column you have to scroll and read. This turns
 * it into a search: open it, type `cdg`, hit Enter, you're on
 * canopy-data-gateway without touching the mouse.
 *
 * Opt in from the template, nothing more:
 *
 *   <details data-fuzzy-switcher data-fuzzy-label="Search projects">
 *     <summary>…</summary>
 *     <div class="…panel…">
 *       <div data-fuzzy-list>
 *         <a href="…">canopy-data-gateway</a>
 *         …
 *       </div>
 *     </div>
 *   </details>
 *
 * The search box is injected here, never by the template. With JS off the
 * dropdown stays exactly what it was — a plain, scrollable list of links —
 * rather than a text field that silently does nothing.
 *
 * This is a module so the matcher below can be unit-tested directly
 * (frontend/src/lib/fuzzy-switcher.test.js). The DOM half is guarded, so
 * importing it under node is side-effect free.
 */

// ── Matching ─────────────────────────────────────────────────────────────
//
// A port of fzf's FuzzyMatchV2 — the same Smith-Waterman variant, the same
// bonus and gap constants, the same backtrace. Not a lookalike: `cdg` scores
// canopy-data-gateway the way fzf scores it, because it is the same
// recurrence, and the ranking you get is the ranking you have in your
// terminal.
//
// It is here rather than pulled from npm on purpose. fzf-for-js is the real
// thing too, but it is a package — it would drag this file into the vite
// bundle it currently has no need of. The algorithm is ~120 lines; the
// dependency is not worth its own build step.
//
// Reference: junegunn/fzf, src/algo/algo.go.

const SCORE_MATCH = 16;
const SCORE_GAP_START = -3;
const SCORE_GAP_EXTENSION = -1;

// A match at the start of a word is worth more than one in the middle of it,
// but not so much more that a long acronym always beats a short close match.
const BONUS_BOUNDARY = SCORE_MATCH / 2; // 8
const BONUS_NON_WORD = SCORE_MATCH / 2; // 8
const BONUS_BOUNDARY_WHITE = BONUS_BOUNDARY + 2;
const BONUS_BOUNDARY_DELIMITER = BONUS_BOUNDARY + 1;
// camelCase boundaries come without the separator a kebab boundary has, so
// they are worth one gap-extension less.
const BONUS_CAMEL_123 = BONUS_BOUNDARY + SCORE_GAP_EXTENSION; // 7
// Enough that an unbroken run is never worth breaking up.
const BONUS_CONSECUTIVE = -(SCORE_GAP_START + SCORE_GAP_EXTENSION); // 4
// The first character you type says the most about what you meant.
const BONUS_FIRST_CHAR_MULTIPLIER = 2;

// Ordered: everything above NON_WORD counts as part of a word.
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
  // Anything else letter-ish (accents, non-latin) still reads as a word.
  if (ch.toLowerCase() !== ch.toUpperCase()) return CLASS_LETTER;
  return CLASS_NON_WORD;
}

function bonusFor(prevClass, cls) {
  // `>=`, not `>`: a non-word character opening a string or following a
  // separator is itself a boundary, which is what puts `.github/…` above
  // `v2.10.3` when you type a bare `.`.
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

/** Per-position boundary bonus, computed on the original (cased) text. */
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

/**
 * Score `text` against `query`, returning `{ score, positions }` or `null`
 * when the query is not a subsequence of the text at all.
 *
 * `positions` are the characters the optimal alignment landed on — what the
 * dropdown underscores as you type.
 *
 * Matching is always case-insensitive. fzf's smart-case would make `CDG` match
 * nothing here, since every project name is lowercase; in a six-entry dropdown
 * that reads as broken rather than as a feature.
 */
export function fuzzyMatch(query, text) {
  const original = String(text);
  const pattern = String(query).trim().toLowerCase();
  if (!pattern) return { score: 0, positions: [] };

  const lower = original.toLowerCase();
  const m = pattern.length;
  const n = lower.length;
  if (m > n) return null;

  // Earliest each pattern character can land, scanning forward. Doubles as the
  // cheap reject, and bounds each row of the matrix below.
  const first = new Int32Array(m);
  let from = 0;
  for (let i = 0; i < m; i++) {
    const at = lower.indexOf(pattern[i], from);
    if (at === -1) return null;
    first[i] = at;
    from = at + 1;
  }

  // Latest the final character can land — nothing past it can be part of any
  // match, so the matrix stops there.
  const last = lower.lastIndexOf(pattern[m - 1]);

  const bonus = bonusTable(original);
  // h[i][j]: best score aligning pattern[0..i] within text[0..j].
  // c[i][j]: length of the unbroken run of matches ending at (i, j).
  const h = [];
  const c = [];
  for (let i = 0; i < m; i++) {
    h.push(new Int32Array(n));
    c.push(new Int32Array(n));
  }

  // The first row is not the general recurrence. A match on the first pattern
  // character *starts* the alignment, so it takes the match score outright
  // instead of the better of that and the score carried along from an earlier
  // occurrence of the same character. That asymmetry is why a repeated leading
  // character costs you: on `cp`, company-data-check outranks
  // contact-product-interest even though the latter puts `p` on a boundary.
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
      // Carrying the match one column right without consuming a pattern
      // character: cheap to continue a gap, dearer to open one.
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
          // Landing on a word boundary that beats the one this run started
          // from means a better run starts here — cut the old one loose.
          if (b >= BONUS_BOUNDARY && b > runStart) {
            consecutive = 1;
          } else {
            b = Math.max(b, Math.max(BONUS_CONSECUTIVE, runStart));
          }
        }

        if (matchScore + b < gapScore) {
          // Taking the gap scores better than honouring the run.
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

  // Walk the matrix back to the characters the best alignment actually used.
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

  // The backtrace can only be short if the matrix and the forward scan
  // disagree, which they should not. Highlighting is not worth a wrong answer,
  // so fall back to the leftmost alignment we already know exists.
  if (positions.length !== m) return { score, positions: Array.from(first) };

  return { score, positions };
}

/**
 * Rank `items` (strings) against `query`, best first, dropping non-matches.
 *
 * Ties break the way fzf's default `--tiebreak=length` does: shorter name
 * first, then the order the server sent them in.
 *
 * Returns `{ index, label, score, positions }` so a caller holding a parallel
 * array of DOM nodes can map back to them — two entries could in principle
 * share a label, and the index never lies.
 */
export function rankMatches(query, items) {
  const scored = [];
  for (let index = 0; index < items.length; index++) {
    const label = items[index];
    const match = fuzzyMatch(query, label);
    if (match) scored.push({ index, label, ...match });
  }
  // With nothing typed every entry scores 0, and sorting would quietly
  // reorder the untouched list by name length. Leave it as the server sent it
  // — that is the order with the current project in it.
  if (!String(query).trim()) return scored;

  scored.sort(
    (a, b) => b.score - a.score || a.label.length - b.label.length || a.index - b.index,
  );
  return scored;
}

// ── DOM ──────────────────────────────────────────────────────────────────

/** Matches shown once you start typing. Beyond this it's a "+N more" hint. */
const DEFAULT_LIMIT = 10;
/** Below this many entries a list is quicker to read than to search. */
const DEFAULT_MIN_ITEMS = 8;

let uid = 0;

const toInt = (value, fallback) => {
  const n = Number.parseInt(value, 10);
  return Number.isFinite(n) ? n : fallback;
};

/**
 * Rewrite a link's text with the matched characters wrapped in <mark>.
 * Built from text nodes rather than innerHTML — the labels are project names
 * off the server, and this is not the place to start trusting them.
 */
function paint(link, label, positions) {
  link.textContent = "";
  if (!positions.length) {
    link.textContent = label;
    return;
  }
  let at = 0;
  for (let k = 0; k < positions.length; ) {
    // Collapse a run of adjacent matches into one <mark>.
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

function enhance(details) {
  const list = details.querySelector("[data-fuzzy-list]");
  if (!list || list.dataset.fuzzyReady) return;

  const links = Array.from(list.querySelectorAll(":scope > a"));
  if (links.length < toInt(details.dataset.fuzzyMin, DEFAULT_MIN_ITEMS)) return;

  list.dataset.fuzzyReady = "1";
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
  // Held as a node, not looked up through `visible[active]`: filtering
  // replaces `visible` wholesale, and an index into the old array cannot clear
  // the row it used to point at. Getting this wrong leaves every row you have
  // ever highlighted still highlighted.
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
    // Re-appending an existing node moves it. The anchors keep their classes,
    // so the current-project styling travels with them.
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

    // Arm the top match, so Enter means "the obvious one". With nothing typed
    // there is no top match, and the obvious one is where you already are —
    // opening the switcher and hitting Enter should not fling you at whatever
    // sorts first.
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
    // preventScroll: the panel is absolutely positioned in the top bar, and
    // letting the browser scroll to the field jumps the page.
    input.focus({ preventScroll: true });
  }

  input.addEventListener("input", apply);

  // Bound on the <details> so the keys work whether focus is in the input or
  // has tabbed onto one of the links.
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
  });

  details.addEventListener("toggle", () => {
    if (details.open) opened();
  });

  // `toggle` alone is not enough. The browser queues it and coalesces the
  // queued pair, so closing and reopening inside one task — Esc then straight
  // back in, or a double click on the summary — fires nothing at all, and the
  // panel returns with the last query still in it and no focus. The summary's
  // own click is never coalesced. Deferred by a frame because a summary's
  // activation behaviour runs after its listeners, so `open` is still stale
  // here. Both paths land on `opened()`, which is safe to run twice.
  summary?.addEventListener("click", () => {
    requestAnimationFrame(() => {
      if (details.open) opened();
    });
  });

  // Establish the closed-state layout now rather than on first open, so
  // opening the panel doesn't reflow under the pointer.
  apply();
}

export function initFuzzySwitchers(root = document) {
  root.querySelectorAll("details[data-fuzzy-switcher]").forEach(enhance);
}

if (typeof document !== "undefined") {
  initFuzzySwitchers();
}
