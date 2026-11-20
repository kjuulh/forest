use anyhow::Context;
use nostatus::{CheckInfo, CheckStatus, StatusError};
use notmad::{Component, ComponentInfo, MadError};
use tokio_util::sync::CancellationToken;

use crate::{State, release_drain::ReleaseDrain};

pub struct Checks {
    pub state: State,
    pub drain: ReleaseDrain,
}

impl Component for Checks {
    fn info(&self) -> ComponentInfo {
        "forest-server/checks".into()
    }

    async fn setup(&self) -> Result<(), MadError> {
        let status = nostatus::StatusRegistry::builder()
            .add_fn(CheckInfo::new("db").severity(nostatus::Severity::Major), {
                let db = self.state.db.clone();
                move || {
                    let db = db.clone();
                    async move {
                        sqlx::query("SELECT 1;")
                            .fetch_one(&db)
                            .await
                            .context("failed to query database")?;

                        Ok::<_, StatusError>(CheckStatus::Healthy)
                    }
                }
            })
            .add_fn(
                CheckInfo::new("accepting-releases").severity(nostatus::Severity::Critical),
                {
                    let drain = self.drain.clone();
                    move || {
                        let draining = drain.is_draining();
                        async move {
                            if draining {
                                return Err(StatusError::Inner(anyhow::anyhow!(
                                    "draining for shutdown: no new releases are claimed"
                                )));
                            }
                            Ok::<_, StatusError>(CheckStatus::Healthy)
                        }
                    }
                },
            )
            .build();
        nostatus::set_global(status);

        Ok(())
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        let draining = self.drain.draining();
        tokio::spawn(async move {
            draining.cancelled().await;
            nostatus::global().run_checks().await;
        });

        nostatus::global()
            .run(cancellation_token.child_token())
            .await;

        Ok(())
    }
}
