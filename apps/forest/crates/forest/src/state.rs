use drop_queue::DropQueue;

use crate::cli::output::OutputFormat;

#[derive(clap::Parser, Clone)]
pub struct Config {
    /// Forest server URL — overrides the active context's server.
    #[arg(long, env = "FOREST_SERVER")]
    pub forest_server: Option<String>,

    /// Use a named context for this invocation, overriding the active one.
    /// See `forest context --help`.
    #[arg(long, env = "FOREST_CONTEXT", global = true)]
    pub context: Option<String>,

    /// Output format for list/show-style commands.
    /// pretty (default) = table, text = TSV, name = first column only,
    /// json = typed JSON array.
    #[arg(long, value_enum, default_value_t, global = true)]
    pub format: OutputFormat,

    /// Increase log verbosity: -v = info, -vv = debug, -vvv = trace. In a
    /// terminal the default is warn-only (the rich UI carries the narration);
    /// this brings the structured logs back. `FOREST_LOG` overrides it.
    #[arg(short = 'v', long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Ceiling on binary downloads in flight at once (DATA-505).
    ///
    /// Downloads ramp up adaptively and settle wherever aggregate throughput
    /// stops improving, so this is an upper bound rather than a target — the
    /// steady state is usually below it. `1` disables concurrency entirely
    /// (the pre-DATA-505 serial behaviour). Unset means the adaptive default.
    #[arg(long, env = "FOREST_DOWNLOAD_CONCURRENCY", global = true, value_parser = clap::value_parser!(u16).range(1..))]
    pub download_concurrency: Option<u16>,
}

impl Config {
    /// In-flight download ceiling, resolved from the flag/env with the
    /// adaptive default and the hard safety clamp applied.
    pub fn max_downloads_in_flight(&self) -> usize {
        crate::download::resolve_max_in_flight(self.download_concurrency.map(usize::from))
    }
}

#[derive(Clone)]
pub struct State {
    pub drop_queue: DropQueue,

    pub config: Config,
}

impl State {
    pub async fn new(config: Config) -> anyhow::Result<Self> {
        Ok(Self {
            drop_queue: DropQueue::new(),
            config,
        })
    }
}
