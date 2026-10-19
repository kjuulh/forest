use std::io::IsTerminal;
use std::sync::OnceLock;
use std::time::Duration;

use console::Style;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

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

static QUIET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_quiet(quiet: bool) {
    QUIET.store(quiet, std::sync::atomic::Ordering::Relaxed);
}

fn muted() -> bool {
    QUIET.load(std::sync::atomic::Ordering::Relaxed)
}

fn multi() -> &'static MultiProgress {
    static MP: OnceLock<MultiProgress> = OnceLock::new();
    MP.get_or_init(MultiProgress::new)
}

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

fn emit(glyph: &str, style: &Style, msg: &str) {
    if muted() {
        return;
    }
    if interactive() {
        let _ = multi().println(format!("{} {}", style.apply_to(glyph), msg));
    } else {
        eprintln!("{glyph} {msg}");
    }
}

pub fn success(msg: impl AsRef<str>) {
    emit("✓", &accent(), msg.as_ref());
}

pub fn status(msg: impl AsRef<str>) {
    if muted() {
        return;
    }
    let m = msg.as_ref();
    if interactive() {
        let _ = multi().println(format!("{} {}", dim().apply_to("→"), dim().apply_to(m)));
    } else {
        eprintln!("→ {m}");
    }
}

pub fn warn(msg: impl AsRef<str>) {
    emit("!", &warn_style(), msg.as_ref());
}

pub fn error(msg: impl AsRef<str>) {
    emit("✗", &error_style(), msg.as_ref());
}

const TICK: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", "✓"];

pub fn step(msg: impl Into<String>) -> Step {
    let msg = msg.into();
    if muted() {
        return Step { pb: None, msg };
    }
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

pub struct Step {
    pb: Option<ProgressBar>,
    msg: String,
}

impl Step {
    pub fn done(self) {
        let msg = self.msg.clone();
        self.done_with(msg);
    }

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

pub fn bytes_bar(label: impl Into<String>, total: u64) -> Bar {
    let label = label.into();
    if muted() {
        return Bar { pb: None };
    }
    if interactive() {
        let pb = multi().add(ProgressBar::new(total));
        if let Ok(style) =
            ProgressStyle::with_template("{msg} {bar:24.green/dim} {bytes}/{total_bytes} ({eta})")
        {
            pb.set_style(style.progress_chars("█▉▊▋▌▍▎▏ "));
        }
        pb.set_message(label);
        Bar { pb: Some(pb) }
    } else {
        eprintln!("→ {label} ({total} bytes)");
        Bar { pb: None }
    }
}

pub fn transfer(label: impl Into<String>) -> Bar {
    let label = label.into();
    if muted() {
        return Bar { pb: None };
    }
    if interactive() {
        let pb = multi().add(ProgressBar::new_spinner());
        if let Ok(style) = ProgressStyle::with_template("{spinner:.green} {msg} {bytes}") {
            pb.set_style(style.tick_strings(TICK));
        }
        pb.set_message(label);
        pb.enable_steady_tick(Duration::from_millis(90));
        Bar { pb: Some(pb) }
    } else {
        eprintln!("→ {label}");
        Bar { pb: None }
    }
}

pub struct Bar {
    pb: Option<ProgressBar>,
}

impl Bar {
    pub fn inc(&self, delta: u64) {
        if let Some(pb) = &self.pb {
            pb.inc(delta);
        }
    }

    pub fn handle(&self) -> Option<ProgressBar> {
        self.pb.clone()
    }

    pub fn clear(self) {
        if let Some(pb) = &self.pb {
            pb.finish_and_clear();
        }
    }

    pub fn finish_with(self, msg: impl AsRef<str>) {
        if let Some(pb) = &self.pb {
            pb.finish_and_clear();
        }
        success(msg);
    }
}

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
