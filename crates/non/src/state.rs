#[derive(Clone)]
pub struct State {}

impl State {
    pub async fn new() -> anyhow::Result<Self> {
        Ok(Self {})
    }
}
