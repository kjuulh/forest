use drop_queue::DropQueue;

#[derive(clap::Parser, Clone)]
pub struct Config {
    /// Forest server URL (required for registry operations, optional for local commands)
    #[arg(long, env = "FOREST_SERVER")]
    pub forest_server: Option<String>,
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
