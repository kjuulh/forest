//! Bootstrap for the `cue` CLI binary.
//!
//! Forest shells out to `cue` from many code paths. Rather than probing
//! for `cue` at every command entry point (paid even on the happy path),
//! we let the spawn happen normally and react only when `tokio::process`
//! returns `ErrorKind::NotFound`. At that point — and only then — we
//! decide what to do:
//!
//!   * macOS with `brew` on PATH and an interactive terminal → prompt to
//!     `brew install cue`, then retry the spawn.
//!   * macOS without brew, or non-interactive, or Linux → clean error
//!     pointing at <https://cuelang.org/docs/install/>.
//!
//! The result of the first `NotFound` handling is memoised for the rest
//! of the process, so concurrent or repeat call sites don't re-prompt.
//!
//! See `apps/forest/TASKS/022-cue-bootstrap.md` for the spec.

use std::io::{IsTerminal, Write};

use tokio::sync::OnceCell;

static ENSURED: OnceCell<Result<(), String>> = OnceCell::const_new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Linux,
    Other,
}

impl Platform {
    fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(target_os = "linux") {
            Platform::Linux
        } else {
            Platform::Other
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Env {
    pub brew_on_path: bool,
    pub stdin_is_tty: bool,
    pub stderr_is_tty: bool,
    pub ci_set: bool,
    pub no_prompt_set: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallHint {
    Generic,
    BrewSuggested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    PromptBrew,
    Fail(InstallHint),
}

/// Pure decision function for the cue-missing case. `cue` is known to be
/// absent at this point — we only choose between "prompt for brew" and
/// "fail with a hint".
pub fn classify(env: &Env, platform: Platform) -> Action {
    if platform != Platform::MacOs || !env.brew_on_path {
        return Action::Fail(InstallHint::Generic);
    }
    let interactive = env.stdin_is_tty && env.stderr_is_tty && !env.ci_set && !env.no_prompt_set;
    if interactive {
        Action::PromptBrew
    } else {
        Action::Fail(InstallHint::BrewSuggested)
    }
}

pub fn hint_message(hint: InstallHint) -> &'static str {
    match hint {
        InstallHint::Generic => {
            "cue is required but not installed.\n\
             See https://cuelang.org/docs/install/ for install instructions."
        }
        InstallHint::BrewSuggested => {
            "cue is required but not installed.\n\
             Install with: brew install cue\n\
             Or see https://cuelang.org/docs/install/ for other install options."
        }
    }
}

/// Run a `cue` invocation. If the spawn fails with `NotFound`, trigger
/// the bootstrap flow (prompt + brew install on macOS, clean error
/// otherwise) and retry the spawn once.
///
/// The `build` closure is invoked at most twice: once for the initial
/// attempt, once for the post-install retry. It must be a `Fn` (not
/// `FnOnce`) so we can call it on retry.
pub async fn output<F>(build: F) -> anyhow::Result<std::process::Output>
where
    F: Fn() -> tokio::process::Command,
{
    match build().output().await {
        Ok(o) => Ok(o),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            ensure_installed().await?;
            build()
                .output()
                .await
                .map_err(|e| anyhow::anyhow!("running cue after install: {e}"))
        }
        Err(e) => Err(anyhow::anyhow!("spawning cue: {e}")),
    }
}

/// Memoised bootstrap: prompt + `brew install cue` on macOS-with-brew-and-TTY,
/// otherwise a clean error pointing at the cuelang install docs. Called
/// only after a real `Command::new("cue")` spawn returned `NotFound`.
async fn ensure_installed() -> anyhow::Result<()> {
    let result = ENSURED
        .get_or_init(|| async {
            match run_once().await {
                Ok(()) => Ok(()),
                Err(e) => Err(e.to_string()),
            }
        })
        .await;

    match result {
        Ok(()) => Ok(()),
        Err(msg) => Err(anyhow::anyhow!("{msg}")),
    }
}

async fn run_once() -> anyhow::Result<()> {
    let env = gather_env();
    match classify(&env, Platform::current()) {
        Action::Fail(hint) => anyhow::bail!("{}", hint_message(hint)),
        Action::PromptBrew => prompt_and_install_brew().await,
    }
}

fn gather_env() -> Env {
    Env {
        brew_on_path: binary_on_path("brew"),
        stdin_is_tty: std::io::stdin().is_terminal(),
        stderr_is_tty: std::io::stderr().is_terminal(),
        ci_set: env_set("CI"),
        no_prompt_set: env_set("FOREST_NO_PROMPT"),
    }
}

fn env_set(key: &str) -> bool {
    std::env::var(key).map(|v| !v.is_empty()).unwrap_or(false)
}

/// Walk `PATH` and check for an executable file with the given name. No
/// subprocess — sub-millisecond on a warm fs.
fn binary_on_path(name: &str) -> bool {
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path_var).any(|dir| is_executable(&dir.join(name)))
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_file() && (meta.permissions().mode() & 0o111) != 0,
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn is_executable(path: &std::path::Path) -> bool {
    path.is_file()
}

async fn prompt_and_install_brew() -> anyhow::Result<()> {
    // Render on stderr so stdout pipes stay clean.
    {
        let mut stderr = std::io::stderr().lock();
        let _ = writeln!(stderr, "cue is required but not installed.");
        let _ = write!(stderr, "Install with: brew install cue [Y/n] ");
        let _ = stderr.flush();
    }

    // `read_line` is blocking; bounce to a blocking thread so we don't
    // pin a tokio worker while we wait for the user.
    let answer = tokio::task::spawn_blocking(read_yes_no_default_yes)
        .await
        .unwrap_or(false);
    if !answer {
        anyhow::bail!("{}", hint_message(InstallHint::BrewSuggested));
    }

    tracing::info!("running: brew install cue");
    let status = tokio::process::Command::new("brew")
        .args(["install", "cue"])
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .await
        .map_err(|e| anyhow::anyhow!("failed to spawn `brew install cue`: {e}"))?;

    if !status.success() {
        anyhow::bail!(
            "`brew install cue` exited with status {}. \
             See https://cuelang.org/docs/install/ for alternative install options.",
            status.code().unwrap_or(-1)
        );
    }

    if !binary_on_path("cue") {
        anyhow::bail!(
            "`brew install cue` reported success but `cue` is still not on PATH. \
             Try running `brew doctor` or restarting your shell."
        );
    }

    Ok(())
}

fn read_yes_no_default_yes() -> bool {
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) => false, // EOF — treat as NO.
        Ok(_) => parse_yes_no_default_yes(&line),
        Err(_) => false,
    }
}

fn parse_yes_no_default_yes(input: &str) -> bool {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return true;
    }
    matches!(trimmed.to_ascii_lowercase().as_str(), "y" | "yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(brew: bool, stdin_tty: bool, stderr_tty: bool, ci: bool, no_prompt: bool) -> Env {
        Env {
            brew_on_path: brew,
            stdin_is_tty: stdin_tty,
            stderr_is_tty: stderr_tty,
            ci_set: ci,
            no_prompt_set: no_prompt,
        }
    }

    #[test]
    fn non_macos_always_generic_hint() {
        for &platform in &[Platform::Linux, Platform::Other] {
            for &brew in &[true, false] {
                let e = env(brew, true, true, false, false);
                assert_eq!(classify(&e, platform), Action::Fail(InstallHint::Generic));
            }
        }
    }

    #[test]
    fn macos_without_brew_generic_hint() {
        let e = env(false, true, true, false, false);
        assert_eq!(
            classify(&e, Platform::MacOs),
            Action::Fail(InstallHint::Generic)
        );
    }

    #[test]
    fn macos_with_brew_and_tty_prompts() {
        let e = env(true, true, true, false, false);
        assert_eq!(classify(&e, Platform::MacOs), Action::PromptBrew);
    }

    #[test]
    fn macos_brew_no_stdin_tty_no_prompt() {
        let e = env(true, false, true, false, false);
        assert_eq!(
            classify(&e, Platform::MacOs),
            Action::Fail(InstallHint::BrewSuggested)
        );
    }

    #[test]
    fn macos_brew_no_stderr_tty_no_prompt() {
        let e = env(true, true, false, false, false);
        assert_eq!(
            classify(&e, Platform::MacOs),
            Action::Fail(InstallHint::BrewSuggested)
        );
    }

    #[test]
    fn ci_suppresses_prompt() {
        let e = env(true, true, true, true, false);
        assert_eq!(
            classify(&e, Platform::MacOs),
            Action::Fail(InstallHint::BrewSuggested)
        );
    }

    #[test]
    fn no_prompt_env_suppresses_prompt() {
        let e = env(true, true, true, false, true);
        assert_eq!(
            classify(&e, Platform::MacOs),
            Action::Fail(InstallHint::BrewSuggested)
        );
    }

    #[test]
    fn generic_hint_text() {
        let msg = hint_message(InstallHint::Generic);
        assert!(msg.contains("cue is required but not installed"));
        assert!(msg.contains("https://cuelang.org/docs/install/"));
        assert!(!msg.contains("brew install"));
    }

    #[test]
    fn brew_hint_text() {
        let msg = hint_message(InstallHint::BrewSuggested);
        assert!(msg.contains("cue is required but not installed"));
        assert!(msg.contains("brew install cue"));
        assert!(msg.contains("https://cuelang.org/docs/install/"));
    }

    #[test]
    fn parse_yes_no_accepts_yes_variants() {
        for input in ["y\n", "Y\n", "yes\n", "YES\n", "Yes\n", "  y  \n"] {
            assert!(parse_yes_no_default_yes(input), "expected YES for {input:?}");
        }
    }

    #[test]
    fn parse_yes_no_defaults_yes_on_empty() {
        for input in ["\n", "", "   \n", "\t\n"] {
            assert!(parse_yes_no_default_yes(input), "expected default-YES for {input:?}");
        }
    }

    #[test]
    fn parse_yes_no_rejects_no_variants() {
        for input in ["n\n", "N\n", "no\n", "NO\n", "nope\n", "garbage\n", "0\n"] {
            assert!(!parse_yes_no_default_yes(input), "expected NO for {input:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn binary_on_path_finds_executable_and_rejects_non_executable() {
        let dir = std::env::temp_dir().join(format!("forest-cue-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("fakebin");
        let nonexe = dir.join("fakelib");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        std::fs::write(&nonexe, "data").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&nonexe, std::fs::Permissions::from_mode(0o644)).unwrap();

        let orig = std::env::var_os("PATH");
        // SAFETY: not thread-safe with concurrent PATH readers; no other
        // test in this module reads PATH at the same time.
        unsafe {
            std::env::set_var("PATH", &dir);
        }
        assert!(binary_on_path("fakebin"));
        assert!(!binary_on_path("fakelib"));
        assert!(!binary_on_path("does-not-exist-anywhere"));
        unsafe {
            match orig {
                Some(v) => std::env::set_var("PATH", v),
                None => std::env::remove_var("PATH"),
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
