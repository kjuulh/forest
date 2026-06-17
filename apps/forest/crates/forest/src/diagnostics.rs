//! miette diagnostics for the build/publish dispatch path (DATA-312).
//!
//! Forest's command machinery traffics in `anyhow` and flattens errors to
//! strings as they pass through the `notmad` runner (see
//! [`crate::cli::unwrap_run_errors`]). A `miette::Diagnostic`'s rich structure
//! — labelled spans, related sub-diagnostics, help text — would not survive
//! that flattening. So instead of returning Diagnostics up the stack, the
//! build/publish leaves *render* them to a string here ([`render`]) and bail
//! with that string via anyhow. `main` prints the bailed string verbatim, so
//! the graphical report reaches the user intact.
//!
//! Scope is deliberately narrow (DATA-312): only the new build/publish path
//! produces these. Everything else keeps plain anyhow errors.

use std::path::Path;

use miette::{Diagnostic, GraphicalReportHandler, GraphicalTheme, LabeledSpan, NamedSource};
use thiserror::Error;

/// Render a diagnostic to a string using miette's graphical handler, honouring
/// `NO_COLOR` and non-TTY stderr (no ANSI when piped).
pub fn render(diag: &dyn Diagnostic) -> String {
    let handler = if use_color() {
        GraphicalReportHandler::new()
    } else {
        GraphicalReportHandler::new_themed(GraphicalTheme::unicode_nocolor())
    };
    let mut out = String::new();
    // render_report only errors if the writer errors; a String never does.
    let _ = handler.render_report(&mut out, diag);
    out
}

/// Wrap a diagnostic in an `anyhow::Error` whose payload is the fully-rendered
/// graphical report. Leaf code does `return Err(report(diag))` and the existing
/// anyhow-based machinery prints it as-is.
pub fn report(diag: impl Diagnostic + Send + Sync + 'static) -> anyhow::Error {
    anyhow::Error::msg(render(&diag))
}

fn use_color() -> bool {
    use std::io::IsTerminal;
    std::env::var_os("NO_COLOR").is_none() && std::io::stderr().is_terminal()
}

// ============================================================
// CUE evaluation failures
// ============================================================

/// A `cue export` failure, with a source span pointing at the offending line
/// when CUE reported a `file:line:col` location we can map back to bytes.
#[derive(Debug, Error, Diagnostic)]
#[error("manifest does not evaluate")]
#[diagnostic(
    code(forest::cue::eval),
    help("`cue export` rejected the manifest. Fix the reported field and re-run.")
)]
pub struct CueEvalError {
    #[source_code]
    src: NamedSource<String>,
    #[label("{reason}")]
    span: Option<miette::SourceSpan>,
    reason: String,
}

impl CueEvalError {
    /// Build from the raw `cue` stderr. Best-effort: if a `file:line:col` can
    /// be parsed and the file read, the diagnostic underlines that line;
    /// otherwise it falls back to a span-less report carrying the cleaned
    /// message. Either way the user gets something far better than a Debug dump.
    pub fn from_cue_stderr(working_dir: &Path, stderr: &str) -> Self {
        let reason = clean_cue_message(stderr);

        if let Some((file, line, col)) = parse_cue_location(stderr) {
            let path = if file.is_absolute() {
                file.clone()
            } else {
                working_dir.join(&file)
            };
            if let Ok(content) = std::fs::read_to_string(&path) {
                let span = line_col_to_span(&content, line, col);
                return Self {
                    src: NamedSource::new(file.to_string_lossy(), content),
                    span: Some(span),
                    reason,
                };
            }
        }

        Self {
            src: NamedSource::new("manifest", String::new()),
            span: None,
            reason,
        }
    }
}

// ============================================================
// Publish preflight failures
// ============================================================

/// One preflight check failure, rendered as a related sub-diagnostic. The
/// check ID (e.g. `C8`) becomes the diagnostic code so users can grep it.
///
/// `Diagnostic` is hand-implemented (not derived) because the code is dynamic
/// — it carries the check's stable ID, which the derive macro can't express.
#[derive(Debug, Error)]
#[error("{message}")]
pub struct PreflightFailure {
    code: String,
    message: String,
    help: String,
    src: Option<NamedSource<String>>,
    span: Option<miette::SourceSpan>,
}

impl Diagnostic for PreflightFailure {
    fn code<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        Some(Box::new(self.code.clone()))
    }

    fn help<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        Some(Box::new(self.help.clone()))
    }

    fn source_code(&self) -> Option<&dyn miette::SourceCode> {
        self.src.as_ref().map(|s| s as &dyn miette::SourceCode)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        let span = self.span?;
        Some(Box::new(std::iter::once(LabeledSpan::new_with_span(
            Some("here".to_string()),
            span,
        ))))
    }
}

/// Aggregate "publish requirements not met" diagnostic — one related entry per
/// failed check, so the user sees the full punch list in one report.
#[derive(Debug, Error, Diagnostic)]
#[error("publish requirements not met ({} issue{})", failures.len(), if failures.len() == 1 { "" } else { "s" })]
#[diagnostic(
    code(forest::publish::preflight),
    help("Fix the issues below and re-run `forest components publish`.")
)]
pub struct PublishPreflightFailed {
    #[related]
    failures: Vec<PreflightFailure>,
}

impl PublishPreflightFailed {
    /// Build from the preflight failures plus the component's CUE source (used
    /// to underline the offending field where we can locate it).
    pub fn new(
        failures: &[crate::services::preflight::CheckFailure],
        manifest: Option<&CueManifestSource>,
    ) -> Self {
        let failures = failures
            .iter()
            .map(|f| {
                let (src, span) = manifest
                    .and_then(|m| field_for_check(f.id).map(|field| (m, field)))
                    .and_then(|(m, field)| {
                        locate_field(&m.content, field).map(|span| {
                            (Some(NamedSource::new(&m.name, m.content.clone())), Some(span))
                        })
                    })
                    .unwrap_or((None, None));

                PreflightFailure {
                    code: f.id.to_string(),
                    message: f.message.clone(),
                    help: f.hint.clone(),
                    src,
                    span,
                }
            })
            .collect();

        Self { failures }
    }
}

/// The CUE manifest source used to locate spans for preflight failures.
pub struct CueManifestSource {
    pub name: String,
    pub content: String,
}

impl CueManifestSource {
    /// Read the component's manifest for span mapping. Prefers
    /// `forest.component.cue`, falls back to `forest.cue`. Returns `None` if
    /// neither is readable — span mapping is best-effort, so callers degrade
    /// to a span-less report.
    pub fn load(dir: &Path) -> Option<Self> {
        for name in ["forest.component.cue", "forest.cue"] {
            if let Ok(content) = std::fs::read_to_string(dir.join(name)) {
                return Some(Self {
                    name: name.to_string(),
                    content,
                });
            }
        }
        None
    }
}

/// Map a preflight check ID to the manifest field whose line we underline.
/// Checks with no single locatable field (e.g. C6 "no artifact on disk")
/// return `None` and render help-only.
fn field_for_check(id: &str) -> Option<&'static str> {
    match id {
        "C3" => Some("name"),
        "C8" => Some("version"),
        _ => None,
    }
}

// ============================================================
// Missing required tools
// ============================================================

/// One missing tool, rendered as a related sub-diagnostic. The tool name is the
/// diagnostic code; the install hint (if declared) is the help.
#[derive(Debug, Error)]
#[error("`{name}` is not installed or not on PATH")]
pub struct MissingTool {
    name: String,
    hint: Option<String>,
}

impl Diagnostic for MissingTool {
    fn code<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        Some(Box::new(self.name.clone()))
    }

    fn help<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        self.hint
            .as_ref()
            .map(|h| Box::new(h.clone()) as Box<dyn std::fmt::Display + 'a>)
    }
}

/// Aggregate "required tools are missing" diagnostic — one related entry per
/// missing tool. Raised before dispatching to a component that declared
/// `requires.tools`, so the user sees the full list up front instead of a
/// mid-run spawn failure. DATA-312.
#[derive(Debug, Error, Diagnostic)]
#[error("`{component}` needs {} tool{} that {} not installed", tools.len(), if tools.len() == 1 { "" } else { "s" }, if tools.len() == 1 { "is" } else { "are" })]
#[diagnostic(
    code(forest::requires::tools),
    help("Install the tool(s) below, then re-run.")
)]
pub struct MissingTools {
    component: String,
    #[related]
    tools: Vec<MissingTool>,
}

impl MissingTools {
    /// Build from the component name and the tools that failed the PATH check.
    pub fn new(component: impl Into<String>, missing: Vec<crate::tools::which::RequiredTool>) -> Self {
        Self {
            component: component.into(),
            tools: missing
                .into_iter()
                .map(|t| MissingTool {
                    name: t.name,
                    hint: t.hint,
                })
                .collect(),
        }
    }
}

// ============================================================
// Source-mapping helpers
// ============================================================

/// Locate `<field>:` in CUE source and return a span covering the key. Used to
/// underline the offending line in preflight diagnostics. Best-effort: matches
/// the first `field:` (optionally preceded by whitespace) and returns `None` if
/// not found.
fn locate_field(src: &str, field: &str) -> Option<miette::SourceSpan> {
    let mut offset = 0usize;
    for line in src.split_inclusive('\n') {
        let trimmed_start = line.len() - line.trim_start().len();
        let rest = &line[trimmed_start..];
        if rest.starts_with(field)
            && rest[field.len()..]
                .trim_start()
                .starts_with(':')
        {
            let start = offset + trimmed_start;
            return Some((start, field.len()).into());
        }
        offset += line.len();
    }
    None
}

/// Reduce cue's multi-line stderr to a concise one-line reason. cue prints the
/// human message first, then an indented `file:line:col` location line; we keep
/// the message line(s) and drop the location (it's surfaced as the span).
fn clean_cue_message(stderr: &str) -> String {
    let msg = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .find(|l| !l.contains(".cue:"))
        .unwrap_or_else(|| stderr.trim());
    msg.trim_end_matches(':').to_string()
}

/// Parse the first `file.cue:line:col` location out of cue's stderr.
fn parse_cue_location(stderr: &str) -> Option<(std::path::PathBuf, usize, usize)> {
    for token in stderr.split_whitespace() {
        // Trim surrounding punctuation cue sometimes adds (e.g. trailing ':').
        let token = token.trim_matches(|c| c == ':' || c == '(' || c == ')');
        if !token.contains(".cue:") {
            continue;
        }
        let mut parts = token.rsplitn(3, ':');
        let col = parts.next()?.parse::<usize>().ok()?;
        let line = parts.next()?.parse::<usize>().ok()?;
        let file = parts.next()?;
        return Some((std::path::PathBuf::from(file), line, col));
    }
    None
}

/// Convert a 1-based line/column into a byte-offset span covering from the
/// position to the end of that line.
fn line_col_to_span(src: &str, line: usize, col: usize) -> miette::SourceSpan {
    let mut offset = 0usize;
    for (idx, line_str) in src.split_inclusive('\n').enumerate() {
        if idx + 1 == line {
            let col0 = col.saturating_sub(1).min(line_str.len());
            let start = offset + col0;
            let len = line_str.trim_end_matches('\n').len().saturating_sub(col0).max(1);
            return (start, len).into();
        }
        offset += line_str.len();
    }
    (offset.min(src.len()), 1usize).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_field_finds_indented_key() {
        let src = "forest: component: {\n\tname:    \"widget\"\n\tversion: \"0.1.0\"\n}\n";
        let span = locate_field(src, "version").expect("version located");
        let start: usize = span.offset();
        assert_eq!(&src[start..start + 7], "version");
    }

    #[test]
    fn locate_field_absent_returns_none() {
        assert!(locate_field("name: \"x\"\n", "version").is_none());
    }

    #[test]
    fn parse_cue_location_extracts_file_line_col() {
        let stderr = "some constraint failed:\n    ./forest.component.cue:12:5\n";
        let (file, line, col) = parse_cue_location(stderr).expect("location parsed");
        assert_eq!(file.to_string_lossy(), "./forest.component.cue");
        assert_eq!(line, 12);
        assert_eq!(col, 5);
    }

    #[test]
    fn parse_cue_location_none_when_absent() {
        assert!(parse_cue_location("generic failure with no location").is_none());
    }

    #[test]
    fn line_col_to_span_points_at_line() {
        let src = "a\nbb\nccc\n";
        let span = line_col_to_span(src, 2, 1);
        assert_eq!(span.offset(), 2);
    }

    #[test]
    fn preflight_failure_exposes_id_as_code_and_hint_as_help() {
        let manifest = CueManifestSource {
            name: "forest.component.cue".into(),
            content: "forest: component: {\n\tversion: \"latest\"\n}\n".into(),
        };
        let failures = vec![crate::services::preflight::CheckFailure {
            id: "C8",
            message: "version `latest` is not valid semver".into(),
            hint: "Use MAJOR.MINOR.PATCH.".into(),
        }];
        let diag = PublishPreflightFailed::new(&failures, Some(&manifest));
        // Renders without panicking and includes the check id, message, hint,
        // and the underlined source line.
        let rendered = render(&diag);
        assert!(rendered.contains("C8"), "rendered: {rendered}");
        assert!(rendered.contains("not valid semver"), "rendered: {rendered}");
        assert!(rendered.contains("Use MAJOR.MINOR.PATCH."), "rendered: {rendered}");
        assert!(rendered.contains("version: \"latest\""), "rendered: {rendered}");
    }

    #[test]
    fn missing_tools_renders_each_tool_with_hint() {
        let missing = vec![
            crate::tools::which::RequiredTool {
                name: "cargo".into(),
                hint: Some("Install Rust via https://rustup.rs".into()),
            },
            crate::tools::which::RequiredTool {
                name: "docker".into(),
                hint: None,
            },
        ];
        let diag = MissingTools::new("forest-contrib/build-rust", missing);
        let rendered = render(&diag);
        assert!(rendered.contains("build-rust"), "rendered: {rendered}");
        assert!(rendered.contains("cargo"), "rendered: {rendered}");
        assert!(rendered.contains("docker"), "rendered: {rendered}");
        assert!(rendered.contains("rustup.rs"), "rendered: {rendered}");
    }
}
