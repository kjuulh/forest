use anyhow::Context;
use nostatus::{CheckInfo, CheckStatus, StatusError, StatusRegistry};
use notmad::{Component, ComponentInfo, MadError};
use tokio_util::sync::CancellationToken;

/// Publishes forage's health state to the global `nostatus` registry, which
/// `build_router` exposes as `/health`, `/health/ready` and `/health/live`.
///
/// Mirrors forest-server's `checks.rs`, and the split between the two endpoints
/// is the whole point of the component:
///
/// - `/health/live` always returns 200 while the HTTP server answers. This is
///   what the ECS container check probes, and failing it replaces the task.
/// - `/health/ready` returns 503 when a registered check is unhealthy. This is
///   what the ALB target group probes, and failing it takes the task out of
///   rotation without killing it.
///
/// Registering a dependency check here therefore affects traffic routing, not
/// task lifetime — which is why Postgres reachability is safe to include.
pub struct Checks {
    /// `None` when forage is running on the file session store, i.e. without a
    /// database. There is nothing to probe in that case, and an empty registry
    /// reports healthy, which is correct: readiness means "can serve", and a
    /// forage with no database dependency can.
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

        // Registered before any component's `run`, so the router that
        // `serve_http` builds picks up this state rather than the empty
        // default — `nostatus::global()` clones at call time.
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
