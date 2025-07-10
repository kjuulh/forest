use std::sync::{Arc, atomic::AtomicBool};

use tokio::sync::{
    Mutex,
    mpsc::{self, UnboundedReceiver, UnboundedSender},
};

#[derive(Clone)]
pub struct DropQueue {
    draining: Arc<AtomicBool>,
    input: UnboundedSender<Arc<dyn QueueItem + Send + Sync + 'static>>,
    receiver: Arc<Mutex<UnboundedReceiver<Arc<dyn QueueItem + Send + Sync + 'static>>>>,
}

impl DropQueue {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();

        Self {
            draining: Arc::new(AtomicBool::new(false)),
            input: tx,
            receiver: Arc::new(Mutex::new(rx)),
        }
    }

    pub fn assign<F, Fut>(&self, f: F) -> anyhow::Result<()>
    where
        F: FnOnce() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        if self.draining.load(std::sync::atomic::Ordering::Relaxed) {
            panic!("trying to put an item on a draining queue. This is not allowed");
        }

        self.input
            .send(Arc::new(ClosureComponent { inner: Box::new(f) }))
            .expect("unbounded channel should never be full");

        Ok(())
    }

    pub async fn process_next(&self) -> anyhow::Result<()> {
        let item = {
            let mut queue = self.receiver.lock().await;
            queue.recv().await
        };

        if let Some(item) = item {
            item.execute().await?;
        }

        Ok(())
    }

    pub async fn process(&self) -> anyhow::Result<()> {
        loop {
            if self.draining.load(std::sync::atomic::Ordering::Relaxed) {
                return Ok(());
            }

            self.process_next().await?;
        }
    }

    pub async fn try_process_next(&self) -> anyhow::Result<Option<()>> {
        let item = {
            let mut queue = self.receiver.lock().await;
            match queue.try_recv() {
                Ok(o) => o,
                Err(e) => return Ok(None),
            }
        };

        item.execute().await?;

        Ok(Some(()))
    }

    pub async fn drain(&self) -> anyhow::Result<()> {
        self.draining
            .store(true, std::sync::atomic::Ordering::Release);

        while self.try_process_next().await?.is_some() {}

        Ok(())
    }
}

struct ClosureComponent<F, Fut>
where
    F: FnOnce() -> Fut + Send + Sync + 'static,
    Fut: futures::Future<Output = Result<(), anyhow::Error>> + Send + 'static,
{
    inner: Box<F>,
}

#[async_trait::async_trait]
trait QueueItem {
    async fn execute(&self) -> anyhow::Result<()>;
}

#[async_trait::async_trait]
impl<F, Fut> QueueItem for ClosureComponent<F, Fut>
where
    F: FnOnce() -> Fut + Send + Sync + 'static,
    Fut: futures::Future<Output = Result<(), anyhow::Error>> + Send + 'static,
{
    async fn execute(&self) -> Result<(), anyhow::Error> {
        Ok(())
    }
}
