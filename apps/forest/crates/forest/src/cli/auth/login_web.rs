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

const MAX_TOTAL_WAIT: Duration = Duration::from_secs(3600);

pub async fn run(state: &State, show_qr: bool) -> anyhow::Result<()> {
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
    let _ = web_url;

    let client_version = env!("CARGO_PKG_VERSION");

    let init = state
        .grpc_client()
        .initiate_device_login(CLIENT_NAME, client_version)
        .await
        .context("the server may not support web login (try --password)")?;

    eprintln!();
    eprintln!("! First copy your one-time code: {}", init.user_code);

    if show_qr
        && std::io::IsTerminal::is_terminal(&std::io::stderr())
        && let Some(qr) = render_qr(&init.verification_uri_complete)
    {
        eprintln!();
        eprintln!("Or scan this with a phone — the code is already in it:");
        eprintln!();
        eprint!("{qr}");
        eprintln!();
    }

    eprintln!(
        "Opening {} in your browser…",
        format_terminal_hyperlink(&init.verification_uri, &init.verification_uri)
    );
    let _ = std::io::Write::flush(&mut std::io::stderr());

    if let Err(e) = webbrowser::open(&init.verification_uri_complete) {
        eprintln!(
            "(couldn't open a browser automatically: {e}. \
             Visit the URL above on another device and enter the code.)"
        );
    }

    let interval = Duration::from_secs(init.interval_seconds.max(1) as u64);
    let server_expiry = Duration::from_secs(init.expires_in_seconds.max(1) as u64);
    let deadline = std::time::Instant::now() + server_expiry.min(MAX_TOTAL_WAIT);

    let mut current_interval = interval;
    eprintln!("Waiting for approval… (press Ctrl-C to cancel)");

    loop {
        if std::time::Instant::now() >= deadline {
            anyhow::bail!("device login expired before approval — run `forest auth login` again");
        }

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
            DeviceLoginStatus::Pending => {}
            DeviceLoginStatus::SlowDown => {
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

fn format_terminal_hyperlink(url: &str, text: &str) -> String {
    if !supports_hyperlinks() {
        return text.to_string();
    }
    format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
}

fn supports_hyperlinks() -> bool {
    use std::io::IsTerminal;
    if !std::io::stderr().is_terminal() {
        return false;
    }
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if std::env::var_os("FORCE_HYPERLINK").is_some() {
        return true;
    }
    if let Ok(prog) = std::env::var("TERM_PROGRAM") {
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
    false
}

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
            assert!(
                line.starts_with("\x1b[30;47m"),
                "line not colour-wrapped: {line:?}"
            );
            assert!(line.ends_with("\x1b[0m"), "line not reset: {line:?}");
        }
    }

    #[test]
    fn a_quiet_zone_is_present() {
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
        let huge = "h".repeat(8000);
        assert!(render_qr(&huge).is_none());
    }
}
