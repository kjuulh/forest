use drop_queue::DropQueue;

#[derive(Clone)]
pub struct State {
    pub drop_queue: DropQueue,
}

impl State {
    pub async fn new() -> anyhow::Result<Self> {
        Ok(Self {
            drop_queue: DropQueue::new(),
        })
    }
}
