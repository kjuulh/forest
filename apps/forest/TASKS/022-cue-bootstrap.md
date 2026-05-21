# 022: Bootstrap `cue` on first use (VSDD Phase 1 Spec)

Status: **DRAFT**. Awaiting Architect + Adversary review. No tests, no implementation.
Method: Verified Spec-Driven Development.

---

## 0. Intent

Forest shells out to the `cue` binary from many code paths (`forest run`, `forest components build|publish|generate`, `forest update`, `services/project.rs`, `services/component_parser.rs`, `global/cue_eval.rs`). If `cue` is not on `PATH`, every one of those paths fails with a low-level `No such file or directory (os error 2)` from `tokio::process::Command::spawn`, which is unhelpful and inconsistent across sites.

This spec lands a single, lazy presence-check + bootstrap path so the first command that needs `cue` either:

1. proceeds normally (cue already installed), or
2. on macOS with `brew` available + an interactive terminal, offers to `brew install cue`, or
3. exits cleanly with a platform-specific install hint pointing at the official cuelang docs.

User narrative:

> A new dev clones the repo and runs `forest run lint` on their Mac. They have `brew` but never installed `cue`. Forest prints `cue is required but not installed. Install with: brew install cue [Y/n]`. They press enter; brew installs cue; the original `forest run lint` invocation continues.
>
> The same dev's teammate is on Ubuntu. They get `cue is required but not installed. See https://cuelang.org/docs/install/ for install instructions.` and a non-zero exit. They install cue themselves and re-run.
>
> CI (no TTY) hits the same code path on a fresh runner image: it errors immediately with the same hint message — no prompt — and the job fails fast instead of hanging.

---

## 1. Behavioral Specification

### 1.1 Where the check fires

A new helper module `crate::tools::cue` exposes:

```rust
pub async fn output<F>(build: F) -> anyhow::Result<std::process::Output>
where
    F: Fn() -> tokio::process::Command;
```

The bootstrap is **reactive, not eager**. Each existing `Command::new("cue").output().await?` site is rewritten as:

```rust
let output = crate::tools::cue::output(|| {
    let mut cmd = tokio::process::Command::new("cue");
    cmd.args(...);
    cmd
}).await?;
```

Inside `output()`:

1. Invoke the closure → spawn cue → `.output().await`.
2. On `Ok` → return.
3. On `Err(NotFound)` → run the bootstrap (`ensure_installed`), then re-invoke the closure → spawn cue → `.output().await`. If install succeeded, this retry succeeds. If install failed, the error from `ensure_installed` bubbles up via `?`.
4. On any other `Err` → propagate.

Net effect: the happy path pays **zero overhead** — no PATH probes, no env reads, no TTY checks. The only time we touch any of that machinery is when cue is *actually missing*.

`ensure_installed` is memoised via `OnceCell` so concurrent or repeat call sites within one process don't re-prompt or repeat the brew install.

Call sites rewritten:

- `crates/forest/src/cli/components/build.rs` — main `BuildCommand::execute`
- `crates/forest/src/cli/components/publish.rs` — main `PublishCommand::execute` + `eval_tool_facet`
- `crates/forest/src/cli/components/generate.rs` — `run_cue_def_openapi_in_dir`, `discover_component_dependencies`
- `crates/forest/src/services/component_parser.rs` — `get_component_spec_from_cue`
- `crates/forest/src/services/project.rs` — both cue branches in `get_project_file`
- `crates/forest/src/global/cue_eval.rs` — `eval_to_json`

Best-effort Option-returning helpers (`cli/update.rs:read_local_component_version`, `cli/run.rs:_extract_input_schema`, `cli/components/generate.rs:detect_codegen_{output,language}`) are intentionally left to swallow NotFound silently. Each is shadowed by a user-facing entry point that *will* trigger the bootstrap when cue is actually needed.

`forest self check`, `forest --help`, `forest context list`, etc. do not invoke cue at all and therefore never trigger the bootstrap, regardless of whether cue is installed.

### 1.2 The `ensure_available` state machine

```
                    ┌────────────────────────────┐
                    │ which("cue") on PATH?      │
                    └────────────┬───────────────┘
                       found     │     not found
                  ┌──────────────┘             │
                  ▼                            ▼
              Ok(())               ┌──────── platform? ────────┐
                                   │                            │
                                  macOS                     not macOS
                                   │                            │
                          ┌────────┴──────────┐                 ▼
                       brew on PATH?         no              Err(install_hint)
                          │                  │
                       yes│                  ▼
                          ▼            Err(install_hint)
                  ┌────── TTY? ───────┐
                  │                   │
                 yes                  no
                  │                   │
                  ▼                   ▼
        prompt "Install cue via Err(install_hint:
        brew? [Y/n]"            brew-flavoured)
                  │
            ┌─────┴─────┐
           yes          no
            │            │
            ▼            ▼
     run brew install   Err(install_hint:
     cue (streaming)    brew-flavoured)
            │
       ┌────┴───────┐
     success       failure
       │             │
       ▼             ▼
     re-check     Err(brew_install_failed)
     PATH; Ok    
     if found    
```

### 1.3 TTY detection

"Interactive terminal" means **stderr is a TTY** AND **stdin is a TTY** AND `CI` env var is unset or empty AND `FOREST_NO_PROMPT` env var is unset or empty. Both stdin and stderr must be TTYs because we read the answer from stdin and render the prompt on stderr (so it doesn't pollute stdout-piped output). The `CI` opt-out matches the convention forest already uses for the update-nag (`FOREST_NO_UPDATE_CHECK`/`CI` in the README). `FOREST_NO_PROMPT=1` is a new escape hatch for users who want the install hint without any interactive prompts even in a local terminal.

### 1.4 The brew prompt

When the prompt fires, render to **stderr**:

```
cue is required but not installed.
Install with: brew install cue [Y/n]
```

Read a single line from stdin. Accept `y`, `Y`, `yes`, empty (default-yes), `\n` as YES. Treat anything else as NO. On NO, return the install-hint error (§1.6, brew variant). On YES, spawn `brew install cue` with stdin/stdout/stderr inherited so the user sees brew's full output; on non-zero exit, return a `BrewInstallFailed` error (§1.6).

After a successful `brew install cue`, **re-resolve `cue` on `PATH`** (do not assume `/opt/homebrew/bin/cue` or `/usr/local/bin/cue` — brew prefix varies). If the re-resolve fails, return `BrewInstallSucceededButCueNotOnPath` with a hint about `brew doctor` and shell rehash. If it succeeds, return `Ok(())` and the original caller proceeds with its `cue` invocation as if nothing happened.

### 1.5 Install hint messages

Three variants, surfaced via `anyhow::Error` (printed by the CLI's top-level `?`):

- **macOS, brew not on PATH**:
  ```
  cue is required but not installed.
  See https://cuelang.org/docs/install/ for install instructions.
  ```

- **Linux** (any distro):
  ```
  cue is required but not installed.
  See https://cuelang.org/docs/install/ for install instructions.
  ```

- **macOS, brew on PATH, prompt declined or non-TTY**:
  ```
  cue is required but not installed.
  Install with: brew install cue
  Or see https://cuelang.org/docs/install/ for other install options.
  ```

The first two are identical text by design — the spec deliberately does not branch on Linux distro. Linux package managers vary too much (apt/dnf/pacman/apk/nix), and the official cuelang install page already covers all of them.

Windows is **not in scope**. If `cfg!(target_os = "windows")` ever evaluates true in this code path, return the platform-agnostic hint (same text as Linux) — but the rest of forest doesn't currently target Windows, so this is a defensive fallback, not a supported flow.

### 1.6 Error taxonomy

```rust
pub enum CueBootstrapError {
    NotInstalled { hint: InstallHint },     // PATH miss, no prompt fired (non-TTY, non-mac, or brew-missing)
    UserDeclined { hint: InstallHint },     // mac+brew+TTY, user said no
    BrewInstallFailed { exit_code: i32, stderr_tail: String },
    BrewInstallSucceededButCueNotOnPath,    // brew exited 0 but `which cue` still misses
}

pub enum InstallHint {
    Generic,    // §1.5 first/second variant
    BrewSuggested, // §1.5 third variant
}
```

The error is converted to `anyhow::Error` with a top-level message matching the §1.5 hint, so the CLI surface is the same single user-facing line plus a clean non-zero exit.

### 1.7 Interaction with `CUE_REGISTRY` and other env

`ensure_available` does **not** read or modify `CUE_REGISTRY`. It only checks for the binary. Each existing call site already manages its own env (most propagate `CUE_REGISTRY` from the active context). This spec does not change that wiring.

### 1.8 Concurrency

`ensure_available` is safe to call from multiple tasks concurrently within one process. The `OnceCell` guarantees the prompt fires at most once. If two tasks race into the helper before init completes, the second awaits the first's result — it does not get a second prompt.

---

## 2. Edge Case Catalog

| # | Case | Expected behaviour |
|---|------|--------------------|
| E1 | `cue` is on PATH and executable | `Ok(())`, no I/O, no prompt. Memoised. |
| E2 | `cue` is on PATH but not executable (perms) | Treat as "not installed". `which::which` already filters for executability on Unix. |
| E3 | `cue` exists at a path that is a directory | Same as E2 — `which` won't return it. |
| E4 | macOS, brew on PATH, user is in a TTY, answers `y` | Prompt, brew installs, re-resolve, `Ok(())`. |
| E5 | macOS, brew on PATH, user is in a TTY, answers `n` | `Err(UserDeclined { BrewSuggested })`. |
| E6 | macOS, brew on PATH, user is in a TTY, answers empty (just hits enter) | Treat as YES (default). |
| E7 | macOS, brew on PATH, no TTY (CI, redirected stdin) | No prompt. `Err(NotInstalled { BrewSuggested })`. |
| E8 | macOS, brew on PATH, `CI=true` set but a TTY exists | No prompt (CI opt-out wins). `Err(NotInstalled { BrewSuggested })`. |
| E9 | macOS, brew on PATH, `FOREST_NO_PROMPT=1` | No prompt. `Err(NotInstalled { BrewSuggested })`. |
| E10 | macOS, brew not on PATH | `Err(NotInstalled { Generic })`. |
| E11 | Linux, any state | `Err(NotInstalled { Generic })`. |
| E12 | macOS, prompt fires, user sends EOF (Ctrl-D, redirected from `/dev/null`) | Treat as NO. `Err(UserDeclined { BrewSuggested })`. |
| E13 | macOS, prompt accepted, `brew install cue` exits non-zero | `Err(BrewInstallFailed)` with last ~20 lines of brew stderr. Do not retry. |
| E14 | macOS, prompt accepted, `brew install cue` exits zero but `which cue` still misses | `Err(BrewInstallSucceededButCueNotOnPath)`. Hint: "run `brew doctor` or restart your shell". |
| E15 | Two concurrent forest tasks each hit a `cue` call site at the same instant | OnceCell serialises; prompt fires at most once; both tasks receive the same result. |
| E16 | `ensure_available` succeeded earlier in the process, then the user uninstalls cue mid-run (unlikely, but) | Memoised `Ok(())` is returned. The downstream `spawn` will fail with the original `ENOENT`. Acceptable — memoisation is per-process, and uninstalls during a single command run are out of scope. |
| E17 | User has `cue` shadowed by an alias / shell function | `which::which` finds the on-disk binary, not the shell alias. If the binary works, we proceed. |
| E18 | User has multiple cues on PATH (e.g. asdf shim + brew binary) | We use the first one `which` returns, identical to what `Command::new("cue")` would do. |
| E19 | `brew` is on PATH but broken (e.g. `brew install` exits non-zero immediately) | Reported as E13. |
| E20 | `brew install cue` succeeds and the user already had a stale `cue` binary at a different PATH entry | `which::which` returns the *first* match. If brew installed to a later PATH entry, the user keeps the stale cue. Acceptable — we documented "install with brew" and brew succeeded; PATH ordering is a user-config concern. |
| E21 | `FOREST_NO_PROMPT` set to empty string `""` vs `"1"` | Per §1.3, "unset or empty" disables the opt-out. Any non-empty value enables it. |
| E22 | Prompt rendered to stderr while stdout is being piped (`forest project publish \| jq`) | Prompt is on stderr, so stdout pipe is unaffected. Stdin is still a TTY from the user's terminal, so the prompt is answerable. |

---

## 3. Non-Functional Requirements

- **NFR1**: Happy-path overhead is **zero** — `output()` just invokes the closure and spawns cue. No probes, no env reads, no TTY checks unless cue is actually missing. Empirically measured: ~10ms steady-state for `forest build` (which is forest's own boot cost, not the wrapper).
- **NFR2**: No network calls. No filesystem writes outside whatever `brew install` itself does.
- **NFR3**: No new dependencies. PATH walk uses `std::env::split_paths` + `std::fs::metadata`. TTY check via `std::io::IsTerminal` (stable since Rust 1.70).
- **NFR4**: Prompt latency from "cue spawn fails NotFound" to "prompt visible" must be < 50ms on a warm shell.

---

## 4. Verification Strategy

### 4.1 Provable / strongly-testable properties

- **P1**: For all inputs where `which("cue")` returns Ok, `ensure_available()` returns Ok with no side effects (no prompt, no brew spawn). *Test: mock `which` and assert no other syscalls.*
- **P2**: For all non-macOS targets, `ensure_available()` never spawns `brew`. *Test: cfg-gated; assert at compile time via `#[cfg(target_os = "linux")]` module-level test.*
- **P3**: When `CI=true` or `FOREST_NO_PROMPT` non-empty, no prompt is rendered. *Test: inject env, capture stderr, assert no prompt string present.*
- **P4**: When stdin is not a TTY, no prompt is rendered, regardless of platform or brew presence. *Test: hook the TTY-check via a trait, inject `false`.*
- **P5**: `ensure_available` is idempotent within a process — calling it N times after a successful first call performs zero additional `which` lookups. *Test: counter inside a mocked `which` impl.*
- **P6**: The error returned on Linux is structurally identical to the error returned on macOS-without-brew (§1.5 first/second variants). *Test: string equality on the hint.*

### 4.2 Purity boundary

The `cue::ensure_available` module has two layers:

- **Pure core** (`fn classify(env: &Env, platform: Platform) -> Action`): given a snapshot of `{path_has_cue, path_has_brew, is_tty, ci_set, no_prompt_set, target_os}`, returns one of `Action::Proceed | Action::Prompt | Action::Fail(InstallHint)`. No I/O, fully unit-testable, table-driven tests cover the full state space.
- **Effectful shell** (`async fn ensure_available()`): gathers the env snapshot (PATH lookups, TTY check, env var reads), calls `classify`, and on `Prompt` performs the stdin read + brew spawn. Only this layer is non-deterministic.

This split is the load-bearing architectural decision: the matrix in §1.2 has ~12 reachable states, and table-driven tests on `classify` cover all of them without any process spawning or fd manipulation.

### 4.3 What we explicitly do NOT formally verify

- The behaviour of `brew install cue` itself. We treat brew's exit code as ground truth.
- The behaviour of `cue` post-install. If `cue` is installed but broken, downstream call sites surface that, not us.
- Cross-shell prompt rendering (zsh vs bash vs whatever). Stderr writes are POSIX; we don't probe `$TERM`.

---

## 5. Open Architect Decisions

These need a call before Phase 2:

- **Q1**: Crate placement — is `crate::tools::cue` the right module path? Alternatives: `crate::cue_bootstrap`, fold into `crate::requirements`. The `requirements.rs` module exists but is currently a `todo!()` stub for a different feature.
- **Q2**: Should the prompt also fire on `forest run`-spawned **hook scripts** that themselves call `cue`? Probably yes (consistent UX), but the hook process is a separate `bash` process; our memoisation doesn't reach it. Recommend: out of scope — hooks managing their own deps is the hook's problem.
- **Q3**: Do we want a `forest self install-deps` subcommand as an explicit, non-lazy bootstrap path? Recommend: out of scope for this spec, possible follow-up.
- **Q4**: Should `forest self check` *report* whether cue is installed (without prompting)? Recommend: yes, but as a follow-up — keeps this spec focused on the bootstrap-on-use flow.
- **Q5**: For E13 (brew install fails), do we log brew's full stderr or a tail? Tail is friendlier; full might be needed for debugging. Recommend: tail of 20 lines, plus a hint to re-run with `RUST_LOG=debug` for the full output.

---

## 6. Files to Change (Phase 2 preview, not binding)

- `crates/forest/src/main.rs` — register new module.
- `crates/forest/src/tools/cue.rs` *(new)* — `ensure_available` + pure `classify`.
- `crates/forest/src/tools/mod.rs` *(new)* — module index.
- `crates/forest/src/cli/update.rs`, `cli/run.rs`, `cli/components/{build,generate,publish}.rs`, `services/component_parser.rs`, `services/project.rs`, `global/cue_eval.rs` — insert `tools::cue::ensure_available().await?` immediately before each `Command::new("cue")`.
- `crates/forest/Cargo.toml` — add `which` if not already pulled in transitively.
- `crates/forest/tests/` — table-driven tests for `classify`; an integration test that runs `forest run` with a doctored `PATH` (no cue) on Linux and asserts the exact hint message + exit code.

---

## 7. Convergence Criteria

Spec is LOCKED when:

- Architect has resolved Q1–Q5.
- Adversary cannot identify a reachable case in §1.2 that isn't covered by §2 (Edge Case Catalog).
- Adversary cannot find a property in §1 that isn't captured by P1–P6 in §4.1.
- Wording in §1.5 (install hints) is the exact string that will ship — no placeholders.
