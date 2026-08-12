//! `forest admin status` — what is the server, and is it running my fix?
//!
//! The CLI-side counterpart to the forage footer: same `StatusService.Status`
//! fields, same question. Also reports the round trip, which is the cheapest
//! way to tell "the registry is slow" apart from "my link is slow".

use crate::grpc::GrpcClientState;
use crate::state::State;

#[derive(clap::Args)]
pub struct StatusCommand {}

#[derive(serde::Serialize, tabled::Tabled)]
struct StatusRow {
    #[tabled(rename = "FIELD")]
    field: String,
    #[tabled(rename = "VALUE")]
    value: String,
}

impl StatusCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let started = std::time::Instant::now();
        let status = state.grpc_client().server_status().await?;
        let round_trip = started.elapsed();

        // An older server leaves the provenance fields empty. Say so plainly
        // instead of rendering blanks that look like a rendering bug.
        let unknown = |v: String| {
            if v.trim().is_empty() {
                "unknown (server predates build stamping)".to_string()
            } else {
                v
            }
        };

        let rows = vec![
            StatusRow {
                field: "client version".into(),
                value: env!("CARGO_PKG_VERSION").to_string(),
            },
            StatusRow {
                // The server crate's own version, not the released CLI
                // version — they are separate and only the CLI's is managed by
                // release-please. The commit below is the real identity.
                field: "server version (crate)".into(),
                value: unknown(status.version),
            },
            StatusRow {
                field: "server commit".into(),
                value: unknown(status.commit),
            },
            StatusRow {
                field: "server built".into(),
                value: unknown(status.build_time),
            },
            StatusRow {
                field: "round trip".into(),
                value: format!("{:.1}ms", round_trip.as_secs_f64() * 1000.0),
            },
        ];

        print!(
            "{}",
            crate::cli::output::render(&state.config.format, &rows)
        );
        Ok(())
    }
}
