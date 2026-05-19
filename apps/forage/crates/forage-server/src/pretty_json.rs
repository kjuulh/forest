//! Server-side pretty JSON renderer with light syntax highlighting.
//!
//! Spec: `specs/features/007-tools-tab.md` — the manifest section of a tool
//! detail page renders the raw JSON with class-based syntax tokens
//! (`.json-key`, `.json-string`, `.json-number`, `.json-bool`, `.json-null`,
//! `.json-punct`). We tokenise server-side to avoid a client-side
//! highlighting library; the macro `pretty_json_block(...)` wraps the
//! pre-tokenised HTML in a styled `<pre>` block.
//!
//! Properties (per spec P11/P12, E13/E18):
//! - **Total**: every `&str` input produces a `String` output without
//!   panicking. Invalid JSON falls through to an HTML-escaped raw block.
//! - **HTML-safe**: `<`, `>`, `&`, `"`, `'` are escaped inside any string
//!   literal or fallback path. The output is safe to interpolate via
//!   MiniJinja's `{{ ... | safe }}`.
//! - **Deterministic**: identical input always yields identical output.

use serde_json::Value;
use std::fmt::Write as _;

/// Pretty-print + tokenise a JSON string. The returned HTML is safe to
/// inject via `{{ tokens | safe }}` inside a `<pre><code>` element.
///
/// On parse failure, returns the HTML-escaped raw input wrapped in a single
/// `<span class="json-raw">…</span>` so the page still renders something
/// useful for debugging (E13).
pub fn tokenize(raw: &str) -> String {
    match serde_json::from_str::<Value>(raw) {
        Ok(value) => {
            let mut out = String::with_capacity(raw.len() * 2);
            render_value(&value, 0, &mut out);
            out
        }
        Err(_) => {
            let mut out = String::from("<span class=\"json-raw\">");
            html_escape_into(raw, &mut out);
            out.push_str("</span>");
            out
        }
    }
}

fn render_value(v: &Value, indent: usize, out: &mut String) {
    match v {
        Value::Null => out.push_str("<span class=\"json-null\">null</span>"),
        Value::Bool(b) => {
            let _ = write!(out, "<span class=\"json-bool\">{b}</span>");
        }
        Value::Number(n) => {
            let _ = write!(out, "<span class=\"json-number\">{n}</span>");
        }
        Value::String(s) => render_string(s, out),
        Value::Array(items) => render_array(items, indent, out),
        Value::Object(map) => render_object(map, indent, out),
    }
}

fn render_string(s: &str, out: &mut String) {
    out.push_str("<span class=\"json-string\">\"");
    html_escape_into(s, out);
    out.push_str("\"</span>");
}

fn render_array(items: &[Value], indent: usize, out: &mut String) {
    if items.is_empty() {
        out.push_str("<span class=\"json-punct\">[]</span>");
        return;
    }
    out.push_str("<span class=\"json-punct\">[</span>\n");
    let inner = indent + 1;
    for (i, item) in items.iter().enumerate() {
        push_indent(inner, out);
        render_value(item, inner, out);
        if i + 1 != items.len() {
            out.push_str("<span class=\"json-punct\">,</span>");
        }
        out.push('\n');
    }
    push_indent(indent, out);
    out.push_str("<span class=\"json-punct\">]</span>");
}

fn render_object(map: &serde_json::Map<String, Value>, indent: usize, out: &mut String) {
    if map.is_empty() {
        out.push_str("<span class=\"json-punct\">{}</span>");
        return;
    }
    out.push_str("<span class=\"json-punct\">{</span>\n");
    let inner = indent + 1;
    let last = map.len() - 1;
    for (i, (k, v)) in map.iter().enumerate() {
        push_indent(inner, out);
        out.push_str("<span class=\"json-key\">\"");
        html_escape_into(k, out);
        out.push_str("\"</span><span class=\"json-punct\">:</span> ");
        render_value(v, inner, out);
        if i != last {
            out.push_str("<span class=\"json-punct\">,</span>");
        }
        out.push('\n');
    }
    push_indent(indent, out);
    out.push_str("<span class=\"json-punct\">}</span>");
}

fn push_indent(level: usize, out: &mut String) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

fn html_escape_into(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_handles_all_value_types() {
        // P11 — every JSON value type produces a recognised span class.
        let json = r#"{"s":"x","n":42,"b":true,"z":null,"arr":[1,2]}"#;
        let html = tokenize(json);
        assert!(html.contains("json-key"));
        assert!(html.contains("json-string"));
        assert!(html.contains("json-number"));
        assert!(html.contains("json-bool"));
        assert!(html.contains("json-null"));
        assert!(html.contains("json-punct"));
    }

    #[test]
    fn tokenize_escapes_html_in_strings() {
        // P12 — `<`, `>`, `&` in strings are escaped so we never produce
        // raw HTML the user could inject.
        let json = r#"{"xss":"<script>alert('x')</script>"}"#;
        let html = tokenize(json);
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("&#39;"));
    }

    #[test]
    fn tokenize_escapes_html_in_keys() {
        // Keys are user-controlled too — escape them on the same axis.
        let json = r#"{"<bad>":"ok"}"#;
        let html = tokenize(json);
        assert!(!html.contains("<bad>"));
        assert!(html.contains("&lt;bad&gt;"));
    }

    #[test]
    fn tokenize_is_total_on_invalid_json() {
        // E13/E18 — malformed input falls back to an escaped raw span;
        // never panics, never produces unescaped HTML.
        let html = tokenize("{not valid <json>");
        assert!(html.contains("json-raw"));
        assert!(html.contains("&lt;json&gt;"));
    }

    #[test]
    fn tokenize_empty_string_is_total() {
        let html = tokenize("");
        assert!(html.contains("json-raw"));
    }

    #[test]
    fn tokenize_handles_nested_structures() {
        let json = r#"{"a":{"b":[1,{"c":null}]}}"#;
        let html = tokenize(json);
        // Should contain at least one nested key and the null token.
        assert!(html.contains("\"b\""));
        assert!(html.contains("json-null"));
    }

    #[test]
    fn tokenize_is_deterministic() {
        let json = r#"{"k":"v","n":1}"#;
        assert_eq!(tokenize(json), tokenize(json));
    }
}

// proptest-based totality / XSS-injection coverage lives in the Adversarial
// Review phase if/when proptest is added as a dev-dep. The unit tests above
// already exercise P11/P12 directly.
