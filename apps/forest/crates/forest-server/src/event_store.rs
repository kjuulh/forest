use std::{future::Future, pin::Pin};

use mire::{Aggregate, AggregateRoot, EventStore};
use sqlx::{Postgres, Transaction};

/// Forest-specific transactional projection update built on Mire's transaction scope.
///
/// The aggregate events and projection mutation commit atomically. An error from the
/// projection closure drops the transaction without committing either side.
pub(crate) trait EventStoreExt {
    async fn save_with<A, F>(
        &self,
        root: &mut AggregateRoot<A>,
        update_projection: F,
    ) -> anyhow::Result<()>
    where
        A: Aggregate,
        for<'tx> F: FnOnce(
                &'tx mut Transaction<'static, Postgres>,
            ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'tx>>
            + Send;
}

impl EventStoreExt for EventStore {
    async fn save_with<A, F>(
        &self,
        root: &mut AggregateRoot<A>,
        update_projection: F,
    ) -> anyhow::Result<()>
    where
        A: Aggregate,
        for<'tx> F: FnOnce(
                &'tx mut Transaction<'static, Postgres>,
            ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'tx>>
            + Send,
    {
        if !root.has_pending() {
            return Ok(());
        }

        let mut scope = self.begin_transaction().await?;
        scope.save(root).await?;
        update_projection(scope.tx()).await?;
        scope.commit().await?;

        Ok(())
    }
}
