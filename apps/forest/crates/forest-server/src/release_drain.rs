use std::{
    future::Future,
    panic::AssertUnwindSafe,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use futures::FutureExt;

use notmad::{Component, ComponentInfo, MadError};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

pub const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(90);
pub const INTERRUPT_GRACE: Duration = Duration::from_secs(15);
pub const HANDOVER_GRACE: Duration = Duration::from_secs(5);
pub const EXIT_MARGIN: Duration = Duration::from_secs(5);

pub const INTERRUPTED: &str = "interrupted by forest-server shutdown";

tokio::task_local! {
    static INTERRUPT: CancellationToken;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainOutcome {
    Finished,
    Interrupted,
    HandedOver,
    Stuck,
}

#[derive(Clone)]
pub struct ReleaseDrain {
    inner: Arc<Inner>,
}

struct Inner {
    timeout: Duration,
    interrupt_grace: Duration,
    handover_grace: Duration,
    draining: CancellationToken,
    interrupt: CancellationToken,
    abandon: CancellationToken,
    drained: CancellationToken,
    stopped_every_release: AtomicBool,
    tracker: TaskTracker,
}

impl ReleaseDrain {
    pub fn new(timeout: Duration) -> Self {
        Self::with_graces(timeout, INTERRUPT_GRACE, HANDOVER_GRACE)
    }

    pub fn with_graces(
        timeout: Duration,
        interrupt_grace: Duration,
        handover_grace: Duration,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                timeout,
                interrupt_grace,
                handover_grace,
                draining: CancellationToken::new(),
                interrupt: CancellationToken::new(),
                abandon: CancellationToken::new(),
                drained: CancellationToken::new(),
                stopped_every_release: AtomicBool::new(false),
                tracker: TaskTracker::new(),
            }),
        }
    }

    pub fn shutdown_budget(&self) -> Duration {
        self.inner.timeout + self.inner.interrupt_grace + self.inner.handover_grace + EXIT_MARGIN
    }

    pub fn is_draining(&self) -> bool {
        self.inner.draining.is_cancelled()
    }

    pub fn draining(&self) -> CancellationToken {
        self.inner.draining.clone()
    }

    pub fn drained(&self) -> CancellationToken {
        self.inner.drained.clone()
    }

    pub fn stopped_every_release(&self) -> bool {
        self.inner.drained.is_cancelled() && self.inner.stopped_every_release.load(Ordering::SeqCst)
    }

    pub fn in_flight(&self) -> usize {
        self.inner.tracker.len()
    }

    pub fn spawn<F>(&self, task: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.inner.tracker.spawn(task);
    }

    pub async fn run_release<T>(
        &self,
        work: impl Future<Output = anyhow::Result<T>>,
    ) -> anyhow::Result<T> {
        let work = AssertUnwindSafe(INTERRUPT.scope(self.inner.interrupt.clone(), work))
            .catch_unwind()
            .map(|result| match result {
                Ok(result) => result,
                Err(panic) => {
                    let message = panic
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| panic.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "unknown panic".into());
                    tracing::error!("release task panicked: {message}");
                    Err(anyhow::anyhow!("release task panicked: {message}"))
                }
            });
        tokio::select! {
            biased;
            result = work => result,
            _ = self.inner.abandon.cancelled() => Err(anyhow::anyhow!(
                "{INTERRUPTED}: the release did not finish within the drain window"
            )),
        }
    }

    pub async fn drain(&self) -> DrainOutcome {
        let _drained = self.inner.drained.clone().drop_guard();
        self.inner.draining.cancel();
        self.inner.tracker.close();

        tracing::info!(
            in_flight = self.in_flight(),
            timeout_secs = self.inner.timeout.as_secs(),
            "draining: no new releases are claimed, waiting for in-flight releases"
        );
        if self.wait(self.inner.timeout).await {
            tracing::info!("drained: every in-flight release finished");
            self.mark_stopped();
            return DrainOutcome::Finished;
        }

        tracing::warn!(
            in_flight = self.in_flight(),
            "drain window over, interrupting releases that are still running"
        );
        self.inner.interrupt.cancel();
        if self.wait(self.inner.interrupt_grace).await {
            self.mark_stopped();
            return DrainOutcome::Interrupted;
        }

        tracing::warn!(
            in_flight = self.in_flight(),
            "releases did not stop on interrupt, abandoning them and failing them now"
        );
        self.inner.abandon.cancel();
        if self.wait(self.inner.handover_grace).await {
            self.mark_stopped();
            return DrainOutcome::HandedOver;
        }

        tracing::error!(
            in_flight = self.in_flight(),
            "release tasks still running at shutdown; the release reaper will fail them"
        );
        DrainOutcome::Stuck
    }

    fn mark_stopped(&self) {
        self.inner
            .stopped_every_release
            .store(true, Ordering::SeqCst);
    }

    async fn wait(&self, limit: Duration) -> bool {
        tokio::time::timeout(limit, self.inner.tracker.wait())
            .await
            .is_ok()
    }
}

pub fn interrupt_signal() -> Option<CancellationToken> {
    INTERRUPT.try_with(CancellationToken::clone).ok()
}

pub fn ensure_not_interrupted() -> anyhow::Result<()> {
    match interrupt_signal() {
        Some(interrupt) if interrupt.is_cancelled() => anyhow::bail!(INTERRUPTED),
        _ => Ok(()),
    }
}

pub async fn wait_interruptible(
    child: &mut tokio::process::Child,
) -> anyhow::Result<std::process::ExitStatus> {
    let Some(interrupt) = interrupt_signal() else {
        return Ok(child.wait().await?);
    };

    tokio::select! {
        biased;
        exit = child.wait() => Ok(exit?),
        _ = interrupt.cancelled() => {
            if let Some(pid) = child.id().and_then(|pid| i32::try_from(pid).ok()) {
                unsafe {
                    libc::kill(pid, libc::SIGINT);
                }
            }
            let exit = child.wait().await?;
            if exit.success() {
                Ok(exit)
            } else {
                anyhow::bail!("{INTERRUPTED}: the process stopped on interrupt ({exit})")
            }
        }
    }
}

pub struct AfterDrain<C> {
    inner: C,
    drained: CancellationToken,
}

impl<C> AfterDrain<C> {
    pub fn new(drain: &ReleaseDrain, inner: C) -> Self {
        Self {
            inner,
            drained: drain.drained(),
        }
    }
}

impl<C: Component> Component for AfterDrain<C> {
    fn info(&self) -> ComponentInfo {
        self.inner.info()
    }

    async fn setup(&self) -> Result<(), MadError> {
        self.inner.setup().await
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        let stop = CancellationToken::new();
        let relay = tokio::spawn({
            let stop = stop.clone();
            let drained = self.drained.clone();
            async move {
                cancellation_token.cancelled().await;
                drained.cancelled().await;
                stop.cancel();
            }
        });

        let stopped = stop.clone();
        let result = tokio::select! {
            result = self.inner.run(stop) => result,
            _ = async {
                stopped.cancelled().await;
                tokio::time::sleep(EXIT_MARGIN).await;
            } => {
                tracing::warn!(
                    component = %self.inner.info(),
                    "component still closing connections after the drain, exiting without it"
                );
                Ok(())
            }
        };
        relay.abort();
        result
    }

    async fn close(&self) -> Result<(), MadError> {
        self.inner.close().await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use super::*;

    fn short_drain() -> ReleaseDrain {
        ReleaseDrain::with_graces(
            Duration::from_millis(300),
            Duration::from_millis(300),
            Duration::from_millis(300),
        )
    }

    fn release_task(
        drain: &ReleaseDrain,
        work: impl Future<Output = anyhow::Result<()>> + Send + 'static,
    ) -> tokio::sync::oneshot::Receiver<anyhow::Result<()>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let runner = drain.clone();
        drain.spawn(async move {
            let result = runner.run_release(work).await;
            let _ = tx.send(result);
        });
        rx
    }

    #[tokio::test]
    async fn a_release_that_finishes_within_the_drain_window_completes() {
        let drain = short_drain();
        let outcome = release_task(&drain, async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok(())
        });

        assert_eq!(drain.drain().await, DrainOutcome::Finished);
        assert!(
            outcome.await.unwrap().is_ok(),
            "the release ran to completion instead of being cut off"
        );
        assert!(drain.drained().is_cancelled());
    }

    #[tokio::test]
    async fn a_release_that_stops_on_interrupt_reports_the_interruption_itself() {
        let drain = short_drain();
        let outcome = release_task(&drain, async {
            let interrupt = interrupt_signal().expect("releases run with an interrupt signal");
            interrupt.cancelled().await;
            anyhow::bail!("{INTERRUPTED}: stopped cleanly")
        });

        assert_eq!(drain.drain().await, DrainOutcome::Interrupted);
        let error = outcome.await.unwrap().unwrap_err().to_string();
        assert!(error.contains(INTERRUPTED), "got {error:?}");
    }

    #[tokio::test]
    async fn a_release_still_running_after_the_interrupt_is_failed_before_shutdown() {
        let drain = short_drain();
        let outcome = release_task(&drain, std::future::pending());

        assert_eq!(drain.drain().await, DrainOutcome::HandedOver);
        let error = outcome
            .await
            .expect("the task reported its outcome before the drain returned")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(INTERRUPTED),
            "the release fails as interrupted rather than waiting for the reaper, got {error:?}"
        );
    }

    #[tokio::test]
    async fn a_panicking_release_fails_instead_of_unwinding_through_the_scheduler() {
        let drain = short_drain();
        let outcome = release_task(&drain, async {
            if interrupt_signal().is_some() {
                panic!("provider exploded");
            }
            Ok(())
        });

        assert_eq!(drain.drain().await, DrainOutcome::Finished);
        let error = outcome
            .await
            .expect("the task reported its outcome")
            .unwrap_err()
            .to_string();
        assert_eq!(error, "release task panicked: provider exploded");
    }

    #[tokio::test]
    async fn a_drain_that_stopped_every_release_says_so_and_a_stuck_one_does_not() {
        let clean = short_drain();
        assert!(!clean.stopped_every_release(), "not before the drain");
        assert_eq!(clean.drain().await, DrainOutcome::Finished);
        assert!(clean.stopped_every_release());

        let stuck = short_drain();
        stuck.spawn(std::future::pending());
        assert_eq!(stuck.drain().await, DrainOutcome::Stuck);
        assert!(!stuck.stopped_every_release());
    }

    #[tokio::test]
    async fn the_drain_marks_the_server_as_draining_before_waiting() {
        let drain = short_drain();
        assert!(!drain.is_draining());

        let observed = Arc::new(AtomicBool::new(false));
        let watcher = drain.clone();
        let seen = observed.clone();
        drain.spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            seen.store(watcher.is_draining(), Ordering::SeqCst);
        });

        drain.drain().await;
        assert!(
            observed.load(Ordering::SeqCst),
            "work that checks the gate during the drain sees it closed"
        );
        assert!(drain.is_draining());
    }

    #[tokio::test]
    async fn an_interrupted_child_process_gets_sigint_and_may_stop_cleanly() {
        let interrupt = CancellationToken::new();
        let mut child = tokio::process::Command::new("sh")
            .args(["-c", "trap 'exit 3' INT; while true; do sleep 0.05; done"])
            .kill_on_drop(true)
            .spawn()
            .unwrap();

        let trigger = interrupt.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            trigger.cancel();
        });

        let error = INTERRUPT
            .scope(interrupt, wait_interruptible(&mut child))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(INTERRUPTED) && error.contains('3'),
            "the child ran its SIGINT handler (exit 3) instead of being killed, got {error:?}"
        );
    }

    #[tokio::test]
    async fn no_new_process_starts_once_interrupted() {
        let interrupt = CancellationToken::new();
        interrupt.cancel();
        let result = INTERRUPT
            .scope(interrupt, async { ensure_not_interrupted() })
            .await;
        assert!(result.is_err());
        assert!(
            ensure_not_interrupted().is_ok(),
            "outside a release nothing is interrupted"
        );
    }

    struct Probe {
        stopped: Arc<AtomicBool>,
    }

    impl Component for Probe {
        async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
            cancellation_token.cancelled().await;
            self.stopped.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_component_after_the_drain_keeps_serving_until_releases_are_drained() {
        let drain = short_drain();
        let stopped = Arc::new(AtomicBool::new(false));
        let component = Arc::new(AfterDrain::new(
            &drain,
            Probe {
                stopped: stopped.clone(),
            },
        ));
        let shutdown = CancellationToken::new();
        let running = tokio::spawn({
            let component = component.clone();
            let shutdown = shutdown.clone();
            async move { component.run(shutdown).await }
        });

        shutdown.cancel();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !stopped.load(Ordering::SeqCst),
            "shutdown alone does not stop what in-flight releases depend on"
        );

        drain.drain().await;
        running.await.unwrap().unwrap();
        assert!(stopped.load(Ordering::SeqCst));
    }
}
