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
