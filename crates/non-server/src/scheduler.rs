use anyhow::Context;
use non_models::ReleaseStatus;
use notmad::{Component, MadError};
use tokio_util::sync::CancellationToken;

use crate::{
    State,
    destination_services::{DestinationServices, DestinationServicesState},
    services::{
        destination_registry::{DestinationRegistry, DestinationRegistryState},
        release_registry::{ReleaseItem, ReleaseRegistry, ReleaseRegistryState},
    },
};

pub struct Scheduler {
    release_registry: ReleaseRegistry,
    destination_registry: DestinationRegistry,
    destinations: DestinationServices,
}

impl Scheduler {
    pub async fn handle(&self, _cancellation: &CancellationToken) -> anyhow::Result<()> {
        let Some((staged_release, tx)) = self.release_registry.get_staged_release().await? else {
            return Ok(());
        };

        tracing::info!(id =% staged_release.id, "begin processing release");

        let res = self.schedule_destination(&staged_release).await;
        match res {
            Ok(_) => {
                self.release_registry
                    .commit_release_status(&staged_release, tx, ReleaseStatus::Success)
                    .await?;
            }
            Err(e) => {
                tracing::warn!("failed to handle release: {e:#}");

                self.release_registry
                    .commit_release_status(&staged_release, tx, ReleaseStatus::Failure)
                    .await?;
            }
        }

        Ok(())
    }

    async fn schedule_destination(&self, staged_release: &ReleaseItem) -> anyhow::Result<()> {
        let dest = self
            .destination_registry
            .get(&staged_release.destination_id)
            .await?
            .context("failed to find a destination")?;

        let dest_svc = self
            .destinations
            .get_destination(
                &dest.destination_type.organisation,
                &dest.destination_type.name,
                dest.destination_type.version,
            )
            .context(anyhow::anyhow!(
                "no implementation of: {} exists",
                dest.destination_type
            ))?;

        dest_svc.prepare(staged_release, &dest).await?;
        dest_svc.release(staged_release, &dest).await?;

        tracing::info!("got this far");

        Ok(())
    }
}

#[async_trait::async_trait]
impl Component for Scheduler {
    fn name(&self) -> Option<String> {
        Some("non-server/scheduler".into())
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    break;
                }
                _ = interval.tick() => {
                    self.handle(&cancellation_token).await?;
                }
            }
        }

        Ok(())
    }
}

pub trait SchedulerState {
    fn scheduler(&self) -> Scheduler;
}

impl SchedulerState for State {
    fn scheduler(&self) -> Scheduler {
        Scheduler {
            release_registry: self.release_registry(),
            destinations: self.destination_services(),
            destination_registry: self.destination_registry(),
        }
    }
}
