use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use futures::StreamExt;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub const INITIAL_IN_FLIGHT: usize = 2;

pub const DEFAULT_MAX_IN_FLIGHT: usize = 8;

pub const MAX_IN_FLIGHT_CEILING: usize = 32;

const GROWTH_THRESHOLD: f64 = 0.10;

const RATE_WINDOW: usize = 16;

pub fn resolve_max_in_flight(flag: Option<usize>) -> usize {
    let from_env = || {
        std::env::var("FOREST_DOWNLOAD_CONCURRENCY")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
    };
    let requested = flag.or_else(from_env).unwrap_or(DEFAULT_MAX_IN_FLIGHT);
    requested.clamp(1, MAX_IN_FLIGHT_CEILING)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Signal {
    Completed { aggregate_rate: Option<f64> },
    Backpressure,
    NeutralFailure,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RampState {
    limit: usize,
    max: usize,
    probing: bool,
    baseline: Option<f64>,
}

impl RampState {
    pub fn new(max: usize) -> Self {
        let max = max.clamp(1, MAX_IN_FLIGHT_CEILING);
        Self {
            limit: INITIAL_IN_FLIGHT.min(max),
            probing: max > 1,
            max,
            baseline: None,
        }
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    pub fn max(&self) -> usize {
        self.max
    }

    pub fn probing(&self) -> bool {
        self.probing
    }

    pub fn observe(&mut self, signal: Signal) -> usize {
        match signal {
            Signal::Backpressure => {
                self.limit = (self.limit / 2).max(1);
                self.probing = false;
                self.baseline = None;
            }
            Signal::NeutralFailure => {}
            Signal::Completed { aggregate_rate } => {
                if self.probing {
                    self.step(aggregate_rate);
                }
            }
        }
        self.limit
    }

    fn step(&mut self, aggregate_rate: Option<f64>) {
        if self.limit >= self.max {
            self.probing = false;
            return;
        }
        let Some(rate) = aggregate_rate else {
            self.limit += 1;
            return;
        };
        match self.baseline {
            None => {
                self.baseline = Some(rate);
                self.limit += 1;
            }
            Some(previous) if rate >= previous * (1.0 + GROWTH_THRESHOLD) => {
                self.baseline = Some(rate);
                self.limit += 1;
            }
            Some(_) => {
                self.limit = (self.limit - 1).max(1);
                self.probing = false;
            }
        }
    }
}

#[derive(Debug, Default)]
struct RateWindow {
    samples: VecDeque<(Instant, u64)>,
}

impl RateWindow {
    fn record(&mut self, at: Instant, bytes: u64) {
        if self.samples.len() == RATE_WINDOW {
            self.samples.pop_front();
        }
        self.samples.push_back((at, bytes));
    }

    fn rate(&self) -> Option<f64> {
        let first = self.samples.front()?.0;
        let last = self.samples.back()?.0;
        if self.samples.len() < 2 {
            return None;
        }
        let span = last.duration_since(first).as_secs_f64();
        if span <= 0.0 {
            return None;
        }
        let bytes: u64 = self.samples.iter().skip(1).map(|(_, b)| *b).sum();
        Some(bytes as f64 / span)
    }
}

pub struct Limiter {
    semaphore: Arc<Semaphore>,
    inner: Mutex<Inner>,
    in_flight: AtomicUsize,
    peak_in_flight: AtomicUsize,
}

struct Inner {
    state: RampState,
    rates: RateWindow,
    issued: usize,
    debt: usize,
}

impl Limiter {
    pub fn new(max_in_flight: usize) -> Arc<Self> {
        let state = RampState::new(max_in_flight);
        let issued = state.limit();
        Arc::new(Self {
            semaphore: Arc::new(Semaphore::new(issued)),
            inner: Mutex::new(Inner {
                state,
                rates: RateWindow::default(),
                issued,
                debt: 0,
            }),
            in_flight: AtomicUsize::new(0),
            peak_in_flight: AtomicUsize::new(0),
        })
    }

    pub async fn acquire(self: &Arc<Self>) -> Slot {
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("download limiter semaphore is never closed");
        let now = self.in_flight.fetch_add(1, Ordering::AcqRel) + 1;
        self.peak_in_flight.fetch_max(now, Ordering::AcqRel);
        Slot {
            limiter: Arc::clone(self),
            permit: Some(permit),
        }
    }

    pub fn add_bytes(&self, bytes: u64) {
        let mut inner = self.inner.lock().expect("limiter mutex poisoned");
        inner.rates.record(Instant::now(), bytes);
    }

    pub fn note_completion(&self) {
        let rate = {
            let inner = self.inner.lock().expect("limiter mutex poisoned");
            inner.rates.rate()
        };
        self.record(Signal::Completed {
            aggregate_rate: rate,
        });
    }

    pub fn record(&self, signal: Signal) {
        let mut inner = self.inner.lock().expect("limiter mutex poisoned");
        let target = inner.state.observe(signal);
        self.reconcile(&mut inner, target);
    }

    fn reconcile(&self, inner: &mut Inner, target: usize) {
        let effective = inner.issued.saturating_sub(inner.debt);
        if target > effective {
            let grow = target - effective;
            let from_debt = grow.min(inner.debt);
            inner.debt -= from_debt;
            let mint = grow - from_debt;
            if mint > 0 {
                inner.issued += mint;
                self.semaphore.add_permits(mint);
            }
        } else if target < effective {
            let mut owe = effective - target;
            while owe > 0 {
                match self.semaphore.try_acquire() {
                    Ok(permit) => {
                        permit.forget();
                        inner.issued -= 1;
                        owe -= 1;
                    }
                    Err(_) => break,
                }
            }
            inner.debt += owe;
        }
    }

    pub fn current_limit(&self) -> usize {
        let inner = self.inner.lock().expect("limiter mutex poisoned");
        inner.state.limit()
    }

    pub fn peak_in_flight(&self) -> usize {
        self.peak_in_flight.load(Ordering::Acquire)
    }

    fn release(&self, permit: OwnedSemaphorePermit) {
        self.in_flight.fetch_sub(1, Ordering::AcqRel);
        let mut inner = self.inner.lock().expect("limiter mutex poisoned");
        if inner.debt > 0 {
            inner.debt -= 1;
            inner.issued -= 1;
            permit.forget();
        } else {
            drop(permit);
        }
    }
}

pub struct Slot {
    limiter: Arc<Limiter>,
    permit: Option<OwnedSemaphorePermit>,
}

impl Drop for Slot {
    fn drop(&mut self) {
        if let Some(permit) = self.permit.take() {
            self.limiter.release(permit);
        }
    }
}

pub fn classify(err: &anyhow::Error) -> Signal {
    if let Some(status) = err.downcast_ref::<tonic::Status>() {
        return match status.code() {
            tonic::Code::Unavailable
            | tonic::Code::ResourceExhausted
            | tonic::Code::DeadlineExceeded
            | tonic::Code::Aborted => Signal::Backpressure,
            _ => Signal::NeutralFailure,
        };
    }
    let msg = err.to_string().to_ascii_lowercase();
    let load_shaped = [
        "unavailable",
        "resource exhausted",
        "resourceexhausted",
        "deadline exceeded",
        "timed out",
        "timeout",
        "connection reset",
        "broken pipe",
        "too many requests",
        "429",
        "503",
    ];
    if load_shaped.iter().any(|needle| msg.contains(needle)) {
        Signal::Backpressure
    } else {
        Signal::NeutralFailure
    }
}

pub async fn map_bounded<T, R, F, Fut>(
    items: Vec<T>,
    limiter: Arc<Limiter>,
    f: F,
) -> Vec<anyhow::Result<R>>
where
    F: Fn(T, Arc<Limiter>) -> Fut,
    Fut: Future<Output = anyhow::Result<R>>,
{
    let mut results: Vec<(usize, anyhow::Result<R>)> =
        futures::stream::iter(items.into_iter().enumerate().map(|(index, item)| {
            let limiter = Arc::clone(&limiter);
            let fut = f(item, Arc::clone(&limiter));
            async move {
                let slot = limiter.acquire().await;
                let out = fut.await;
                match &out {
                    Ok(_) => limiter.note_completion(),
                    Err(e) => limiter.record(classify(e)),
                }
                drop(slot);
                (index, out)
            }
        }))
        .buffer_unordered(MAX_IN_FLIGHT_CEILING)
        .collect()
        .await;

    results.sort_by_key(|(index, _)| *index);
    results.into_iter().map(|(_, out)| out).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::time::Duration;

    #[test]
    fn flag_wins_and_defaults_apply() {
        assert_eq!(resolve_max_in_flight(Some(4)), 4);
        assert_eq!(resolve_max_in_flight(None), DEFAULT_MAX_IN_FLIGHT);
    }

    #[test]
    fn concurrency_is_clamped_to_the_ceiling() {
        assert_eq!(resolve_max_in_flight(Some(10_000)), MAX_IN_FLIGHT_CEILING);
    }

    #[test]
    fn one_is_honoured_as_serial_and_zero_is_treated_as_unset() {
        assert_eq!(resolve_max_in_flight(Some(1)), 1);
        assert_eq!(resolve_max_in_flight(Some(0)), 1);
    }

    #[test]
    fn ramp_starts_low_and_below_the_ceiling() {
        let s = RampState::new(8);
        assert_eq!(s.limit(), INITIAL_IN_FLIGHT);
        assert!(s.probing());
    }

    #[test]
    fn a_ceiling_of_one_never_probes_or_grows() {
        let mut s = RampState::new(1);
        assert_eq!(s.limit(), 1);
        assert!(!s.probing());
        for _ in 0..10 {
            s.observe(Signal::Completed {
                aggregate_rate: Some(1e9),
            });
        }
        assert_eq!(s.limit(), 1, "an explicit serial cap must stay serial");
    }

    #[test]
    fn grows_while_throughput_improves_then_holds_at_the_plateau() {
        let mut s = RampState::new(8);
        assert_eq!(
            s.observe(Signal::Completed {
                aggregate_rate: None
            }),
            3
        );
        assert_eq!(
            s.observe(Signal::Completed {
                aggregate_rate: Some(100.0)
            }),
            4
        );
        assert_eq!(
            s.observe(Signal::Completed {
                aggregate_rate: Some(150.0)
            }),
            5
        );
        assert_eq!(
            s.observe(Signal::Completed {
                aggregate_rate: Some(153.0)
            }),
            4
        );
        assert!(!s.probing(), "should stop probing once it plateaus");
        for rate in [400.0, 10.0, 1000.0] {
            assert_eq!(
                s.observe(Signal::Completed {
                    aggregate_rate: Some(rate)
                }),
                4
            );
        }
    }

    #[test]
    fn ramp_never_exceeds_its_ceiling() {
        let mut s = RampState::new(4);
        let mut rate = 100.0;
        for _ in 0..50 {
            s.observe(Signal::Completed {
                aggregate_rate: Some(rate),
            });
            rate *= 2.0;
            assert!(s.limit() <= 4, "limit escaped the ceiling: {}", s.limit());
        }
        assert_eq!(s.limit(), 4);
        assert!(!s.probing());
    }

    #[test]
    fn backpressure_halves_the_limit_and_stops_probing() {
        let mut s = RampState::new(16);
        let mut rate = 100.0;
        for _ in 0..6 {
            s.observe(Signal::Completed {
                aggregate_rate: Some(rate),
            });
            rate *= 2.0;
        }
        let before = s.limit();
        assert!(before >= 4, "expected the ramp to have grown, got {before}");

        assert_eq!(s.observe(Signal::Backpressure), before / 2);
        assert!(
            !s.probing(),
            "must not keep pushing a source that pushed back"
        );

        for _ in 0..10 {
            s.observe(Signal::Backpressure);
        }
        assert_eq!(s.limit(), 1);
    }

    #[test]
    fn neutral_failures_do_not_move_the_limit() {
        let mut s = RampState::new(8);
        s.observe(Signal::Completed {
            aggregate_rate: Some(100.0),
        });
        let before = s.limit();
        for _ in 0..5 {
            assert_eq!(s.observe(Signal::NeutralFailure), before);
        }
        assert!(
            s.probing(),
            "a 404 says nothing about load and must not end probing"
        );
    }

    #[test]
    fn rate_needs_two_samples_and_measures_bytes_over_wall_time() {
        let mut w = RateWindow::default();
        let t0 = Instant::now();
        assert_eq!(w.rate(), None, "no samples");
        w.record(t0, 1_000);
        assert_eq!(w.rate(), None, "a single completion has no duration");
        w.record(t0 + Duration::from_secs(2), 4_000);
        let rate = w.rate().expect("two samples give a rate");
        assert!(
            (rate - 2_000.0).abs() < 1.0,
            "expected ~2000 B/s, got {rate}"
        );
    }

    #[test]
    fn rate_window_is_bounded() {
        let mut w = RateWindow::default();
        let t0 = Instant::now();
        for i in 0..(RATE_WINDOW * 3) {
            w.record(t0 + Duration::from_millis(i as u64 * 10), 100);
        }
        assert_eq!(w.samples.len(), RATE_WINDOW);
    }

    #[tokio::test]
    async fn limiter_never_exceeds_its_ceiling_under_load() {
        let limiter = Limiter::new(4);
        let items: Vec<u64> = (0..64).collect();

        let out = map_bounded(items, Arc::clone(&limiter), |i, lim| async move {
            tokio::time::sleep(Duration::from_millis(2)).await;
            lim.add_bytes(1_024 * 1_024);
            Ok::<_, anyhow::Error>(i)
        })
        .await;

        assert_eq!(out.len(), 64);
        assert!(out.iter().all(|r| r.is_ok()));
        assert!(
            limiter.peak_in_flight() <= 4,
            "cap breached: peak was {}",
            limiter.peak_in_flight()
        );
    }

    #[tokio::test]
    async fn a_ceiling_of_one_runs_strictly_serially() {
        let limiter = Limiter::new(1);
        let concurrent = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let out = map_bounded(
            (0..16).collect::<Vec<u32>>(),
            Arc::clone(&limiter),
            |i, _lim| {
                let concurrent = Arc::clone(&concurrent);
                let peak = Arc::clone(&peak);
                async move {
                    let now = concurrent.fetch_add(1, Ordering::AcqRel) + 1;
                    peak.fetch_max(now, Ordering::AcqRel);
                    tokio::time::sleep(Duration::from_millis(1)).await;
                    concurrent.fetch_sub(1, Ordering::AcqRel);
                    Ok::<_, anyhow::Error>(i)
                }
            },
        )
        .await;

        assert_eq!(out.len(), 16);
        assert_eq!(peak.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn one_failure_does_not_abort_the_rest() {
        let limiter = Limiter::new(4);
        let out = map_bounded(
            (0..20).collect::<Vec<u32>>(),
            Arc::clone(&limiter),
            |i, lim| async move {
                if i % 5 == 0 {
                    anyhow::bail!("component not found: {i}");
                }
                lim.add_bytes(4_096);
                Ok(i)
            },
        )
        .await;

        assert_eq!(out.len(), 20, "every input must produce an outcome");
        let failures = out.iter().filter(|r| r.is_err()).count();
        assert_eq!(failures, 4);
        for (i, r) in out.iter().enumerate() {
            let i = i as u32;
            match r {
                Ok(v) => assert_eq!(*v, i, "results must stay in input order"),
                Err(e) => {
                    assert!(i % 5 == 0);
                    assert!(e.to_string().contains(&format!("not found: {i}")));
                }
            }
        }
    }

    #[tokio::test]
    async fn results_are_returned_in_input_order_despite_out_of_order_completion() {
        let limiter = Limiter::new(8);
        let out = map_bounded(
            (0..8u64).collect::<Vec<u64>>(),
            Arc::clone(&limiter),
            |i, _lim| async move {
                tokio::time::sleep(Duration::from_millis(8 - i)).await;
                Ok::<_, anyhow::Error>(i)
            },
        )
        .await;

        let values: Vec<u64> = out.into_iter().map(|r| r.unwrap()).collect();
        assert_eq!(values, (0..8).collect::<Vec<u64>>());
    }

    #[tokio::test]
    async fn backpressure_errors_shrink_the_live_limiter() {
        let limiter = Limiter::new(16);
        for _ in 0..4 {
            limiter.add_bytes(1_024 * 1_024);
            limiter.note_completion();
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let grown = limiter.current_limit();
        assert!(grown > INITIAL_IN_FLIGHT, "expected growth, got {grown}");

        limiter.record(Signal::Backpressure);
        assert_eq!(limiter.current_limit(), grown / 2);

        let target = limiter.current_limit();
        let items: Vec<u32> = (0..40).collect();
        let fresh_peak = Arc::new(AtomicUsize::new(0));
        let live = Arc::new(AtomicUsize::new(0));
        map_bounded(items, Arc::clone(&limiter), |_i, _lim| {
            let fresh_peak = Arc::clone(&fresh_peak);
            let live = Arc::clone(&live);
            async move {
                let now = live.fetch_add(1, Ordering::AcqRel) + 1;
                fresh_peak.fetch_max(now, Ordering::AcqRel);
                tokio::time::sleep(Duration::from_millis(2)).await;
                live.fetch_sub(1, Ordering::AcqRel);
                Ok::<_, anyhow::Error>(())
            }
        })
        .await;
        assert!(
            fresh_peak.load(Ordering::Acquire) <= target.max(1),
            "shrunk limiter admitted {} concurrent, target {target}",
            fresh_peak.load(Ordering::Acquire)
        );
    }

    #[tokio::test]
    async fn shrinking_while_idle_takes_effect_immediately() {
        let limiter = Limiter::new(8);
        limiter.record(Signal::Backpressure);
        assert_eq!(limiter.current_limit(), 1);

        let items: Vec<u32> = (0..24).collect();
        let peak = Arc::new(AtomicUsize::new(0));
        let live = Arc::new(AtomicUsize::new(0));
        map_bounded(items, Arc::clone(&limiter), |_i, lim| {
            let peak = Arc::clone(&peak);
            let live = Arc::clone(&live);
            async move {
                let now = live.fetch_add(1, Ordering::AcqRel) + 1;
                peak.fetch_max(now, Ordering::AcqRel);
                tokio::time::sleep(Duration::from_millis(1)).await;
                live.fetch_sub(1, Ordering::AcqRel);
                lim.add_bytes(1_024 * 1_024);
                Ok::<_, anyhow::Error>(())
            }
        })
        .await;
        assert_eq!(
            peak.load(Ordering::Acquire),
            1,
            "backpressure shrank the limit to 1, so work must be serial"
        );
    }

    #[tokio::test]
    async fn shrinking_mid_flight_waits_for_running_transfers() {
        let limiter = Limiter::new(8);
        let gate = Arc::new(tokio::sync::Notify::new());

        let a = limiter.acquire().await;
        let b = limiter.acquire().await;
        assert_eq!(limiter.peak_in_flight(), 2);

        limiter.record(Signal::Backpressure);
        assert_eq!(limiter.current_limit(), 1);

        let waiter = {
            let limiter = Arc::clone(&limiter);
            let gate = Arc::clone(&gate);
            tokio::spawn(async move {
                let slot = limiter.acquire().await;
                gate.notify_one();
                drop(slot);
            })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiter.is_finished(), "a third slot must not be handed out");

        drop(a);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            !waiter.is_finished(),
            "cap is 1 and a transfer is still running"
        );

        drop(b);
        tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("waiter should be admitted once the limiter drains")
            .expect("waiter task panicked");
    }

    #[tokio::test]
    async fn retries_inside_a_slot_are_preserved() {
        let limiter = Limiter::new(4);
        let attempts = Arc::new(AtomicU64::new(0));

        let out = map_bounded(
            (0..8u32).collect::<Vec<u32>>(),
            Arc::clone(&limiter),
            |i, lim| {
                let attempts = Arc::clone(&attempts);
                async move {
                    let mut last_err = None;
                    for attempt in 0..3 {
                        attempts.fetch_add(1, Ordering::AcqRel);
                        if attempt < 2 {
                            last_err = Some(anyhow::anyhow!("transient"));
                            continue;
                        }
                        lim.add_bytes(1_024);
                        return Ok(i);
                    }
                    Err(last_err.unwrap())
                }
            },
        )
        .await;

        assert!(out.iter().all(|r| r.is_ok()), "retries should have won");
        assert_eq!(
            attempts.load(Ordering::Acquire),
            24,
            "every item should have made all 3 attempts"
        );
        assert!(limiter.peak_in_flight() <= 4);
    }

    #[tokio::test]
    async fn byteless_work_ramps_to_the_ceiling() {
        let limiter = Limiter::new(6);
        let peak = Arc::new(AtomicUsize::new(0));
        let live = Arc::new(AtomicUsize::new(0));

        map_bounded(
            (0..60).collect::<Vec<u32>>(),
            Arc::clone(&limiter),
            |i, _| {
                let peak = Arc::clone(&peak);
                let live = Arc::clone(&live);
                async move {
                    let now = live.fetch_add(1, Ordering::AcqRel) + 1;
                    peak.fetch_max(now, Ordering::AcqRel);
                    tokio::time::sleep(Duration::from_millis(3)).await;
                    live.fetch_sub(1, Ordering::AcqRel);
                    Ok::<_, anyhow::Error>(i)
                }
            },
        )
        .await;

        assert_eq!(
            limiter.current_limit(),
            6,
            "byte-less work should ramp all the way to the ceiling"
        );
        assert!(
            peak.load(Ordering::Acquire) > INITIAL_IN_FLIGHT,
            "concurrency should have grown past the start, peaked at {}",
            peak.load(Ordering::Acquire)
        );
        assert!(
            peak.load(Ordering::Acquire) <= 6,
            "but never past the ceiling, peaked at {}",
            peak.load(Ordering::Acquire)
        );
    }

    #[tokio::test]
    async fn the_driver_signals_completion_so_closures_only_report_bytes() {
        let limiter = Limiter::new(8);
        let before = limiter.current_limit();
        map_bounded(
            (0..4).collect::<Vec<u32>>(),
            Arc::clone(&limiter),
            |i, _lim| async move { Ok::<_, anyhow::Error>(i) },
        )
        .await;
        assert!(
            limiter.current_limit() > before,
            "the driver's completion signals should have moved the limit"
        );
    }

    #[tokio::test]
    async fn empty_input_is_a_no_op() {
        let limiter = Limiter::new(4);
        let out = map_bounded(Vec::<u32>::new(), limiter, |i, _| async move { Ok(i) }).await;
        assert!(out.is_empty());
    }

    #[test]
    fn load_shaped_errors_are_backpressure() {
        for msg in [
            "registry unavailable — is the forest server running?",
            "deadline exceeded",
            "connection reset by peer",
            "HTTP status 503",
            "too many requests",
        ] {
            assert_eq!(
                classify(&anyhow::anyhow!("{msg}")),
                Signal::Backpressure,
                "{msg:?} should read as backpressure"
            );
        }
    }

    #[test]
    fn artifact_level_errors_are_neutral() {
        for msg in [
            "binary not found: org/tool@1.0.0 (darwin/arm64)",
            "sha mismatch — refusing to write to cache",
            "permission denied — your account may not be a member",
        ] {
            assert_eq!(
                classify(&anyhow::anyhow!("{msg}")),
                Signal::NeutralFailure,
                "{msg:?} should not move the ramp"
            );
        }
    }

    #[test]
    fn tonic_statuses_are_classified_by_code() {
        assert_eq!(
            classify(&anyhow::Error::new(tonic::Status::unavailable("down"))),
            Signal::Backpressure
        );
        assert_eq!(
            classify(&anyhow::Error::new(tonic::Status::resource_exhausted(
                "slow down"
            ))),
            Signal::Backpressure
        );
        assert_eq!(
            classify(&anyhow::Error::new(tonic::Status::not_found("nope"))),
            Signal::NeutralFailure
        );
    }
}
