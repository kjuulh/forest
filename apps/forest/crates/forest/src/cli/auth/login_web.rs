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

pub async fn run(state: &State) -> anyhow::Result<()> {
    // Resolve the web URL for the active context — without one, we
    // don't know where to send the browser and there's no point starting
    // the flow.
    let store = ContextStore::from_env()?;
    let want_ctx = state.config.context.as_deref();
    let entry = store.resolve(want_ctx).or_else(|_| store.active())?;
    let web_url = entry.resolve_web_url().ok_or_else(|| {
        anyhow::anyhow!(
            "context '{}' has no web URL set and the server URL ({}) \
             doesn't follow the forest. → forage. convention. \
             Run `forest context set-web-url {} <forage-url>` or pass \
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
    eprintln!("Opening {} in your browser…", init.verification_uri);
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

        let status = DeviceLoginStatus::try_from(resp.status).unwrap_or(DeviceLoginStatus::Unspecified);
        match status {
            DeviceLoginStatus::Approved => {
                let user = resp.user.context("server reported APPROVED but sent no user")?;
                let tokens = resp.tokens.context("server reported APPROVED but sent no tokens")?;
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
                current_interval = (current_interval + Duration::from_secs(5))
                    .min(Duration::from_secs(30));
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
