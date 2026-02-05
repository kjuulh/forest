use anyhow::Context;
use nostatus::{CheckInfo, CheckStatus, StatusError};
use notmad::{Component, ComponentInfo, MadError};
use tokio_util::sync::CancellationToken;

use crate::State;

pub struct Checks {
    pub state: State,
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
            .build();
        nostatus::set_global(status);

        Ok(())
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        nostatus::global()
            .run(cancellation_token.child_token())
            .await;

        Ok(())
    }
}
