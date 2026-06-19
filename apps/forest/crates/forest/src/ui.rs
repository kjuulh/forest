//! Interactive terminal UI for stderr.
//!
//! forest's stderr serves two audiences: a human at a TTY, and machines / CI
//! reading logs. This renders a minimal, forest-themed experience for the
//! former — colored status lines, spinners, byte progress bars — and degrades
//! to plain lines (with `tracing` carrying the structured detail) for the
//! latter. stdout is reserved for machine output (`--format`); this is
//! stderr-only.
//!
//! Theme: minimalist and green-forward. A small set of glyphs (✓ → ! ✗), one
//! accent colour (forest green), dim for secondary text. No boxes or banners.

use std::io::IsTerminal;
use std::sync::OnceLock;
use std::time::Duration;

use console::Style;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

/// True only at an interactive stderr TTY that hasn't opted out — the one
/// place that decides whether we render the rich UI. Mirrors the checks that
/// were scattered across the codebase (`NO_COLOR`, `CI`, `FOREST_NO_PROMPT`).
pub fn interactive() -> bool {
    static INTERACTIVE: OnceLock<bool> = OnceLock::new();
    *INTERACTIVE.get_or_init(|| {
        std::io::stderr().is_terminal()
            && std::env::var_os("NO_COLOR").is_none()
            && !env_set("CI")
            && !env_set("FOREST_NO_PROMPT")
    })
}

fn env_set(k: &str) -> bool {
    std::env::var(k).map(|v| !v.is_empty()).unwrap_or(false)
}

/// Shared draw target so concurrent bars/spinners and status lines don't
/// clobber each other on the terminal.
fn multi() -> &'static MultiProgress {
    static MP: OnceLock<MultiProgress> = OnceLock::new();
    MP.get_or_init(MultiProgress::new)
}

// ── Forest theme ───────────────────────────────────────────────
fn accent() -> Style {
    Style::new().green()
}
fn dim() -> Style {
    Style::new().dim()
}
fn warn_style() -> Style {
    Style::new().yellow()
}
fn error_style() -> Style {
    Style::new().red().bold()
}

/// Emit a status line that coexists with any active progress bars. Styled at
/// an interactive TTY; plain (no ANSI) otherwise so piped/CI logs stay clean.
fn emit(glyph: &str, style: &Style, msg: &str) {
    if interactive() {
        let _ = multi().println(format!("{} {}", style.apply_to(glyph), msg));
    } else {
        eprintln!("{glyph} {msg}");
    }
}

/// A finished step / success: `✓ <msg>` (forest green).
pub fn success(msg: impl AsRef<str>) {
    emit("✓", &accent(), msg.as_ref());
}

/// A neutral, in-progress status: `→ <msg>` (dim).
pub fn status(msg: impl AsRef<str>) {
    let m = msg.as_ref();
    if interactive() {
        let _ = multi().println(format!("{} {}", dim().apply_to("→"), dim().apply_to(m)));
    } else {
        eprintln!("→ {m}");
    }
}

/// A non-fatal warning: `! <msg>` (yellow). Distinct from `tracing::warn!`,
/// which is for log capture; this is for the human following along.
pub fn warn(msg: impl AsRef<str>) {
    emit("!", &warn_style(), msg.as_ref());
}

/// A failure line: `✗ <msg>` (red). Errors still propagate as `Result`s; this
/// is for inline narration before/around them.
pub fn error(msg: impl AsRef<str>) {
    emit("✗", &error_style(), msg.as_ref());
}

const TICK: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", "✓"];

/// Start an indeterminate spinner for a step with no measurable progress (an
/// RPC, a validation). Call [`Step::done`] / [`Step::done_with`] when finished.
/// In non-interactive mode it logs a single line and the handle is inert.
pub fn step(msg: impl Into<String>) -> Step {
    let msg = msg.into();
    if interactive() {
        let pb = multi().add(ProgressBar::new_spinner());
        if let Ok(style) = ProgressStyle::with_template("{spinner:.green} {msg}") {
            pb.set_style(style.tick_strings(TICK));
        }
        pb.set_message(msg.clone());
        pb.enable_steady_tick(Duration::from_millis(90));
        Step { pb: Some(pb), msg }
    } else {
        eprintln!("→ {msg}");
        Step { pb: None, msg }
    }
}

/// Handle for an in-progress [`step`].
pub struct Step {
    pb: Option<ProgressBar>,
    msg: String,
}

impl Step {
    /// Finish the step, leaving a green `✓ <original message>`.
    pub fn done(self) {
        let msg = self.msg.clone();
        self.done_with(msg);
    }

    /// Finish the step with a different final message.
    pub fn done_with(self, msg: impl AsRef<str>) {
        match self.pb {
            Some(pb) => {
                pb.finish_and_clear();
                success(msg);
            }
            None => success(msg),
        }
    }
}

/// A byte-progress bar for uploads/downloads. Inert (single log line) when not
/// interactive. Hand `inc`/`finish` the transferred byte counts.
pub fn bytes_bar(label: impl Into<String>, total: u64) -> Bar {
    let label = label.into();
    if interactive() {
        let pb = multi().add(ProgressBar::new(total));
        if let Ok(style) = ProgressStyle::with_template(
            "{msg} {bar:24.green/dim} {bytes}/{total_bytes} ({eta})",
        ) {
            pb.set_style(style.progress_chars("█▉▊▋▌▍▎▏ "));
        }
        pb.set_message(label);
        Bar { pb: Some(pb) }
    } else {
        eprintln!("→ {label} ({total} bytes)");
        Bar { pb: None }
    }
}

/// Handle for a [`bytes_bar`].
pub struct Bar {
    pb: Option<ProgressBar>,
}

impl Bar {
    pub fn inc(&self, delta: u64) {
        if let Some(pb) = &self.pb {
            pb.inc(delta);
        }
    }

    pub fn finish_with(self, msg: impl AsRef<str>) {
        if let Some(pb) = &self.pb {
            pb.finish_and_clear();
        }
        success(msg);
    }
}

/// Configure `tracing` for the chosen audience: warn-only at an interactive
/// terminal (the rich UI carries the narration), info otherwise (CI/prod/piped
/// keep the structured log). `-v/-vv/-vvv` raise the floor; `FOREST_LOG`
/// overrides everything (forest-specific so it doesn't collide with a parent
/// process's `RUST_LOG`).
pub fn init_logging(verbose: u8) {
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::filter::LevelFilter;

    let default = match verbose {
        0 if interactive() => LevelFilter::WARN,
        0 => LevelFilter::INFO,
        1 => LevelFilter::INFO,
        2 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    };

    let filter = EnvFilter::builder()
        .with_default_directive(default.into())
        .with_env_var("FOREST_LOG")
        .from_env_lossy()
        .add_directive("notmad=warn".parse().expect("valid directive"));

    tracing_subscriber::fmt()
        .pretty()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .init();
}
