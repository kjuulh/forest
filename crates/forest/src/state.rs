use drop_queue::DropQueue;

#[derive(clap::Parser, Clone)]
pub struct Config {
    #[arg(long, env = "FOREST_SERVER")]
    pub forest_server: String,
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
