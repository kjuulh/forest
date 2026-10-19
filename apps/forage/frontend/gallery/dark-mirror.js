const MEDIA = "@media (prefers-color-scheme: dark)";

function matchBrace(css, open) {
  let depth = 0;
  for (let i = open; i < css.length; i++) {
    if (css[i] === "{") depth++;
    else if (css[i] === "}" && --depth === 0) return i + 1;
  }
  return -1;
}

function topLevelRules(body) {
  const rules = [];
  let i = 0;
  while (i < body.length) {
    const open = body.indexOf("{", i);
    if (open === -1) break;
    const end = matchBrace(body, open);
    if (end === -1) throw new Error("unbalanced braces in the dark-mode block");
    rules.push({
      selector: body.slice(i, open).trim(),
      body: body.slice(open + 1, end - 1),
    });
    i = end;
  }
  return rules;
}

function scopeSelector(selector, scope) {
  return selector
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean)
    .map((s) => (s === ":root" || s === ":host" ? scope : `${scope} ${s}`))
    .join(", ");
}

export function darkMirrorCss(inputCss, scope = ':root[data-theme="dark"]') {
  const start = inputCss.indexOf(MEDIA);
  if (start === -1) {
    throw new Error(
      `dark-mirror: could not find "${MEDIA}" in input.css — the gallery's ` +
        `theme toggle is derived from it. Update dark-mirror.js to match.`,
    );
  }
  const open = inputCss.indexOf("{", start);
  const end = matchBrace(inputCss, open);
  const body = inputCss.slice(open + 1, end - 1);

  const rules = topLevelRules(body);
  if (rules.length === 0) {
    throw new Error("dark-mirror: the dark-mode block parsed as empty");
  }

  return [
    "/* Generated from static/css/input.css by gallery/dark-mirror.js. */",
    ...rules.map((r) => `${scopeSelector(r.selector, scope)} {${r.body}}`),
  ].join("\n");
}
