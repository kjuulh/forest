use anyhow::Context;
use nostatus::{CheckInfo, CheckStatus, StatusError, StatusRegistry};
use notmad::{Component, ComponentInfo, MadError};
use tokio_util::sync::CancellationToken;

pub struct Checks {
    pub db: Option<sqlx::PgPool>,
}

impl Component for Checks {
    fn info(&self) -> ComponentInfo {
        "forage/checks".into()
    }

    async fn setup(&self) -> Result<(), MadError> {
        let mut builder = StatusRegistry::builder();

        if let Some(db) = self.db.clone() {
            builder.add_fn(CheckInfo::new("db").severity(nostatus::Severity::Major), {
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
            });
        }

        nostatus::set_global(builder.build());

        Ok(())
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        nostatus::global()
            .run(cancellation_token.child_token())
            .await;

        Ok(())
    }
}
