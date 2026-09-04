//! `forest auth login --web` — RFC 8628 device authorization grant client.
//!
//! See `apps/forest/TASKS/022-device-login.md` §1.2 for the user-visible
//! flow. The handler is intentionally short — the heavy lifting is on the
//! server (`grpc::users::initiate_device_login` / `poll_device_login`).

use std::time::Duration;

use anyhow::Context;
use forest_grpc_interface::DeviceLoginStatus;

use crate::{
    contexts::ContextStore,
    grpc::GrpcClientState,
    state::State,
    user_state::{UserState, UserStateLoaderState, compute_refresh_after},
};

const CLIENT_NAME: &str = "forest-cli";

/// Hard cap on how long we'll loop polling. The server returns its own
/// `expires_in_seconds` (default 900s); we trust it but defensively bound
/// ourselves at 1h to avoid runaway processes on a misconfigured server.
const MAX_TOTAL_WAIT: Duration = Duration::from_secs(3600);

pub async fn run(state: &State, show_qr: bool) -> anyhow::Result<()> {
    // Resolve the web URL for the active context — without one, we
    // don't know where to send the browser and there's no point starting
    // the flow.
    let store = ContextStore::from_env()?;
    let want_ctx = state.config.context.as_deref();
    let entry = store.resolve(want_ctx).or_else(|_| store.active())?;
    let web_url = entry.resolve_web_url().ok_or_else(|| {
        anyhow::anyhow!(
            "context '{}' has no web URL set and the server URL ({}) \
             doesn't have a leading `api.` label. \
             Run `forest context set-web-url {} <web-url>` or pass \
             FOREST_WEB_URL.",
            entry.name,
            entry.server,
            entry.name,
        )
    })?;
    let _ = web_url; // used implicitly via the server's response

    let client_version = env!("CARGO_PKG_VERSION");

    let init = state
        .grpc_client()
        .initiate_device_login(CLIENT_NAME, client_version)
        .await
        .context("the server may not support web login (try --password)")?;

    // Print the one-time code prominently, then auto-open the browser.
    // No "Press Enter" prompt — most users have already seen the prompt
    // they just clicked through, and waiting for input here means
    // headless / scripted flows hang. The browser opens via the
    // `webbrowser` crate which uses xdg-open / open / start as
    // appropriate for the platform.
    eprintln!();
    eprintln!("! First copy your one-time code: {}", init.user_code);

    // The QR goes BEFORE the browser attempt on purpose. On a headless
    // box — a dashboard pi over ssh, a server, a container — the open
    // below fails and its error would otherwise be the last thing on
    // screen, burying the one instruction that still works. The URL it
    // encodes is `verification_uri_complete`, which already carries the
    // code, so scanning it approves without typing anything.
    if show_qr && std::io::IsTerminal::is_terminal(&std::io::stderr()) {
        if let Some(qr) = render_qr(&init.verification_uri_complete) {
            eprintln!();
            eprintln!("Or scan this with a phone — the code is already in it:");
            eprintln!();
            eprint!("{qr}");
            eprintln!();
        }
    }

    eprintln!(
        "Opening {} in your browser…",
        format_terminal_hyperlink(&init.verification_uri, &init.verification_uri)
    );
    let _ = std::io::Write::flush(&mut std::io::stderr());

    // Best-effort browser open. On headless boxes this fails and we
    // fall through to the "open this URL manually" hint — the polling
    // loop still works, the user just has to do the navigation by hand
    // (perhaps on another device).
    if let Err(e) = webbrowser::open(&init.verification_uri_complete) {
        eprintln!(
            "(couldn't open a browser automatically: {e}. \
             Visit the URL above on another device and enter the code.)"
        );
    }

    // Poll loop.
    let interval = Duration::from_secs(init.interval_seconds.max(1) as u64);
    let server_expiry = Duration::from_secs(init.expires_in_seconds.max(1) as u64);
    let deadline = std::time::Instant::now() + server_expiry.min(MAX_TOTAL_WAIT);

    let mut current_interval = interval;
    eprintln!("Waiting for approval… (press Ctrl-C to cancel)");

    loop {
        if std::time::Instant::now() >= deadline {
            anyhow::bail!("device login expired before approval — run `forest auth login` again");
        }

        // Race the sleep against SIGINT so Ctrl-C exits the loop instead
        // of hanging up to interval_seconds. Without this, the user sees
        // an unresponsive terminal until the next poll boundary.
        tokio::select! {
            _ = tokio::time::sleep(current_interval) => {}
            _ = tokio::signal::ctrl_c() => {
                eprintln!();
                anyhow::bail!("cancelled");
            }
        }

        let resp = state
            .grpc_client()
            .poll_device_login(&init.device_code)
            .await?;

        let status =
            DeviceLoginStatus::try_from(resp.status).unwrap_or(DeviceLoginStatus::Unspecified);
        match status {
            DeviceLoginStatus::Approved => {
                let user = resp
                    .user
                    .context("server reported APPROVED but sent no user")?;
                let tokens = resp
                    .tokens
                    .context("server reported APPROVED but sent no tokens")?;
                let now = chrono::Utc::now().timestamp();
                let refresh_after = compute_refresh_after(now, tokens.expires_in_seconds);
                state
                    .user_state()
                    .set_state(&UserState {
                        user_id: user.user_id.clone(),
                        username: user.username.clone(),
                        emails: user.emails.into_iter().map(|e| e.email).collect(),
                        access_token: tokens.access_token,
                        refresh_access: tokens.refresh_token,
                        refresh_after: Some(refresh_after),
                    })
                    .await?;
                eprintln!();
                eprintln!(
                    "✓ Authentication complete. Logged in to context '{}' as {}.",
                    entry.name, user.username
                );
                return Ok(());
            }
            DeviceLoginStatus::Pending => {
                // Quiet — common case during the wait.
            }
            DeviceLoginStatus::SlowDown => {
                // Server asked us to back off. Add 5s and cap at 30s.
                current_interval =
                    (current_interval + Duration::from_secs(5)).min(Duration::from_secs(30));
            }
            DeviceLoginStatus::Denied => {
                anyhow::bail!("device login was denied in the browser");
            }
            DeviceLoginStatus::Expired => {
                anyhow::bail!(
                    "device login expired before approval — run `forest auth login` again"
                );
            }
            DeviceLoginStatus::Unspecified => {
                anyhow::bail!("server returned an unspecified device login status — try again");
            }
        }
    }
}

/// Wrap `text` as an OSC 8 terminal hyperlink pointing at `url` when
/// stderr is a TTY likely to support it. Falls back to the plain URL
/// otherwise — never lets the escape bytes leak into a log file or a
/// CI pipe.
///
/// OSC 8 is supported by iTerm2, kitty, alacritty, WezTerm, modern
/// GNOME Terminal, VS Code's terminal, etc. Terminals that don't
/// understand the sequence silently drop it on most implementations,
/// but a few mangle the output, so we gate on TTY + a TERM allowlist.
fn format_terminal_hyperlink(url: &str, text: &str) -> String {
    if !supports_hyperlinks() {
        return text.to_string();
    }
    // The closing sequence `\x1b]8;;\x1b\\` resets the link state.
    // `\x1b\\` is the ST (String Terminator).
    format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
}

fn supports_hyperlinks() -> bool {
    use std::io::IsTerminal;
    if !std::io::stderr().is_terminal() {
        return false;
    }
    // Respect the de-facto disable switch.
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    // Explicit opt-in / opt-out wins. Many terminals advertise via
    // FORCE_HYPERLINK or set TERM_PROGRAM.
    if std::env::var_os("FORCE_HYPERLINK").is_some() {
        return true;
    }
    if let Ok(prog) = std::env::var("TERM_PROGRAM") {
        // Known-good terminals on macOS / cross-platform.
        if matches!(
            prog.as_str(),
            "iTerm.app" | "WezTerm" | "vscode" | "Hyper" | "ghostty" | "Apple_Terminal"
        ) {
            return true;
        }
    }
    if let Ok(term) = std::env::var("TERM") {
        if term.contains("kitty") || term.contains("alacritty") || term == "xterm-ghostty" {
            return true;
        }
    }
    // Default to off — better a plain URL than mangled output.
    false
}

/// Render `url` as a QR code made of unicode half-blocks.
///
/// Polarity is the whole difficulty, and it is not theme-independent by
/// default. The unicode renderer draws dark modules as filled blocks and
/// light ones as spaces, taking its actual colours from the terminal —
/// so on a dark theme the result is an INVERTED code, which a good
/// number of phone scanners simply refuse. Every line is therefore
/// wrapped in explicit black-on-white (`ESC[30;47m`), which makes the
/// filled blocks dark and the gaps light whichever theme is in use.
///
/// `Dense1x2` packs two module rows into one text row, so a version-4
/// code is about 19 lines rather than 37 — the difference between
/// fitting an ssh window and scrolling out of it.
///
/// Returns `None` if the URL will not fit in a QR code at all, which is
/// not worth failing a login over: the URL is printed above regardless.
fn render_qr(url: &str) -> Option<String> {
    use qrcode::render::unicode::Dense1x2;

    let code = qrcode::QrCode::new(url.as_bytes()).ok()?;
    let rendered = code.render::<Dense1x2>().quiet_zone(true).build();

    Some(
        rendered
            .lines()
            .map(|line| format!("\x1b[30;47m{line}\x1b[0m\n"))
            .collect(),
    )
}

#[cfg(test)]
mod qr_tests {
    use super::render_qr;

    #[test]
    fn renders_a_url_as_wrapped_lines() {
        let qr = render_qr("https://forest.understory.sh/device?code=ABCD-1234")
            .expect("a URL of this length encodes");
        let lines: Vec<&str> = qr.lines().collect();

        assert!(
            lines.len() > 8,
            "suspiciously short QR: {} lines",
            lines.len()
        );
        for line in &lines {
            // Without this the code renders inverted on a dark terminal.
            assert!(
                line.starts_with("\x1b[30;47m"),
                "line not colour-wrapped: {line:?}"
            );
            assert!(line.ends_with("\x1b[0m"), "line not reset: {line:?}");
        }
    }

    #[test]
    fn a_quiet_zone_is_present() {
        // Scanners need the light margin; without it the finder patterns
        // sit flush against whatever else is on the terminal.
        let qr = render_qr("https://example.com/d?code=AAAA-0000").expect("encodes");
        let first = qr.lines().next().expect("at least one line");
        let payload = first
            .trim_start_matches("\x1b[30;47m")
            .trim_end_matches("\x1b[0m");
        assert!(
            payload.chars().all(|c| c == ' '),
            "first row should be all quiet zone, got {payload:?}"
        );
    }

    #[test]
    fn oversized_input_is_declined_rather_than_panicking() {
        // A QR code tops out around 3kB of binary data. Returning None
        // keeps a login working — the URL is printed either way.
        let huge = "h".repeat(8000);
        assert!(render_qr(&huge).is_none());
    }
}
