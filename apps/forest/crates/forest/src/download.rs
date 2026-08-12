//! Bounded, adaptive concurrency for registry downloads (DATA-505).
//!
//! # Why adaptive rather than a fixed cap
//!
//! Every gRPC client in the CLI shares **one** `tonic::transport::Channel`
//! (see [`crate::grpc::GrpcClient::channel`]) and therefore one TCP
//! connection. Running N downloads concurrently opens N HTTP/2 streams over
//! that single connection — they share one congestion window, so extra
//! streams buy almost no additional *bandwidth*. What they do buy is
//! *latency hiding*: each download has a serial prologue (the server's S3
//! GET before the first byte, our sha256 hashing and disk writes after it)
//! and overlapping those prologues is where the speedup comes from.
//!
//! That means the useful in-flight count is small and it saturates quickly.
//! A big fixed cap would only add server load, file descriptors and one
//! HTTP/2 flow-control window worth of buffering per stream for no gain. So
//! instead of asserting a number we ramp: start low, grow while aggregate
//! throughput actually improves, stop at the plateau, and back off when the
//! source pushes back.
//!
//! # Shape
//!
//! [`RampState`] is the whole policy and it is a pure state machine — no
//! clock, no I/O — so the interesting behaviour is unit-testable without
//! timing flake. [`Limiter`] wraps it in a `Semaphore` whose permit count is
//! reconciled to the state machine's current limit, and [`map_bounded`]
//! drives a collection of futures through it.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use futures::StreamExt;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Where the ramp starts. Two is enough to overlap one download's transfer
/// with the next one's server-side S3 fetch, which is the first and largest
/// win; anything beyond that has to earn its place by measurement.
pub const INITIAL_IN_FLIGHT: usize = 2;

/// Default ceiling for the adaptive ramp.
///
/// Chosen for the single-shared-connection reality described above: by the
/// time 8 transfers are in flight, per-download prologues are fully
/// overlapped and the one TCP flow is the limit, so more streams cannot help.
/// It is also low enough that 8 × the HTTP/2 stream window stays a modest
/// amount of client-side buffering. The ramp usually settles well below this.
pub const DEFAULT_MAX_IN_FLIGHT: usize = 8;

/// Hard ceiling, whatever the user or config asks for. Guards against a typo
/// (`--download-concurrency 10000`) exhausting file descriptors or memory.
pub const MAX_IN_FLIGHT_CEILING: usize = 32;

/// Aggregate throughput has to improve by at least this fraction for the ramp
/// to keep opening slots. Below it we treat the extra concurrency as noise and
/// give the slot back.
const GROWTH_THRESHOLD: f64 = 0.10;

/// How many recent completions feed the aggregate-throughput estimate.
const RATE_WINDOW: usize = 16;

/// Resolve the in-flight ceiling from the CLI flag, falling back to
/// `FOREST_DOWNLOAD_CONCURRENCY` and then to [`DEFAULT_MAX_IN_FLIGHT`].
///
/// `1` is honoured and means "no concurrency" — the fully serial behaviour
/// that predates DATA-505, which is the escape hatch if a registry ever
/// misbehaves under parallel streams. `0` is meaningless rather than
/// dangerous, so it is treated as unset.
pub fn resolve_max_in_flight(flag: Option<usize>) -> usize {
    let from_env = || {
        std::env::var("FOREST_DOWNLOAD_CONCURRENCY")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
    };
    let requested = flag.or_else(from_env).unwrap_or(DEFAULT_MAX_IN_FLIGHT);
    requested.clamp(1, MAX_IN_FLIGHT_CEILING)
}

/// What a finished (or failed) transfer tells the controller.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Signal {
    /// A transfer completed. `aggregate_rate` is bytes/sec across all recent
    /// transfers, not this one's own rate — the ramp is climbing total
    /// throughput, and a single stream getting slower while the total gets
    /// faster is exactly the trade we want to accept.
    Completed { aggregate_rate: Option<f64> },
    /// The source pushed back (`UNAVAILABLE`, `RESOURCE_EXHAUSTED`, a
    /// timeout, a connection reset). Concurrency is plausibly the cause, so
    /// halve it.
    Backpressure,
    /// A transfer failed for a reason unrelated to load (not found, sha
    /// mismatch, permission denied). Carries no information about how many
    /// streams the source can take, so the ramp must not react to it.
    NeutralFailure,
}

/// The ramp policy: a single-step hill climb over aggregate throughput.
///
/// Grow while each step measurably improves the total rate; on the first step
/// that does not, give the slot back and stop probing. Back off hard on
/// backpressure and do not resume probing afterwards — a source that has
/// already refused work is not somewhere to keep pushing.
#[derive(Debug, Clone, PartialEq)]
pub struct RampState {
    limit: usize,
    max: usize,
    probing: bool,
    /// Aggregate rate measured at the limit *below* the current one — the
    /// number the next step has to beat.
    baseline: Option<f64>,
}

impl RampState {
    pub fn new(max: usize) -> Self {
        let max = max.clamp(1, MAX_IN_FLIGHT_CEILING);
        Self {
            limit: INITIAL_IN_FLIGHT.min(max),
            // A ceiling of 1 is an explicit "stay serial", not a starting
            // point to grow from.
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

    /// Fold one signal in and return the new limit.
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
            // Nowhere left to grow; stop measuring for growth.
            self.probing = false;
            return;
        }
        let Some(rate) = aggregate_rate else {
            // Not enough completions yet to estimate a rate. Open a slot
            // anyway: the first couple of steps are near-certain wins
            // (they overlap serial prologues) and waiting for a measurement
            // we cannot take yet would leave short dependency sets serial.
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
                // The extra slot did not pay for itself — hand it back and
                // settle here.
                self.limit = (self.limit - 1).max(1);
                self.probing = false;
            }
        }
    }
}

/// Sliding-window aggregate throughput estimator.
///
/// Deliberately measures *completions over wall time* rather than summing
/// per-transfer rates: with N streams in flight the per-transfer rates each
/// fall while the total rises, and the total is what we are optimising.
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

    /// Bytes/sec across the window, or `None` until there is a span to divide
    /// by (a single completion has no duration).
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
        // Skip the first sample's bytes: those moved before the window
        // opened, so counting them against this span overstates the rate.
        let bytes: u64 = self.samples.iter().skip(1).map(|(_, b)| *b).sum();
        Some(bytes as f64 / span)
    }
}

/// Adaptive in-flight limiter.
///
/// Permits live in a `Semaphore` and are reconciled to [`RampState::limit`]
/// as signals arrive. Shrinking cannot revoke a permit that is currently held,
/// so a shrink records *debt* and the next slot to be released is retired
/// instead of returned — the cap tightens as work drains, never by
/// interrupting a transfer that is already running.
pub struct Limiter {
    semaphore: Arc<Semaphore>,
    inner: Mutex<Inner>,
    in_flight: AtomicUsize,
    peak_in_flight: AtomicUsize,
}

struct Inner {
    state: RampState,
    rates: RateWindow,
    /// Permits currently owned by the semaphore (held or available).
    issued: usize,
    /// Permits owed back — retired as slots are released.
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

    /// Wait for a slot. Held for as long as the returned [`Slot`] lives.
    pub async fn acquire(self: &Arc<Self>) -> Slot {
        // `Semaphore` is never closed, so acquisition cannot fail.
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

    /// Add `bytes` to the throughput estimate. Call this from work that moves
    /// data; the driver turns it into a ramp signal when the unit finishes.
    ///
    /// Deliberately does *not* signal on its own — a unit of work may report
    /// bytes several times (or not at all) and only the driver knows when it is
    /// actually done.
    pub fn add_bytes(&self, bytes: u64) {
        let mut inner = self.inner.lock().expect("limiter mutex poisoned");
        inner.rates.record(Instant::now(), bytes);
    }

    /// Signal that a unit of work finished successfully.
    ///
    /// Work that reported no bytes (a metadata RPC — a manifest fetch, a
    /// version lookup) leaves the throughput window empty, so the ramp has no
    /// rate to compare and grows on each completion until it hits the ceiling.
    /// That is the behaviour we want there: extra concurrent round trips are
    /// nearly free and the win is linear in how many are in flight. Byte
    /// transfers do have a rate, so they hill-climb it instead.
    pub fn note_completion(&self) {
        let rate = {
            let inner = self.inner.lock().expect("limiter mutex poisoned");
            inner.rates.rate()
        };
        self.record(Signal::Completed {
            aggregate_rate: rate,
        });
    }

    /// Report a signal directly. Public so callers can classify their own
    /// errors (only they know whether a failure was load-related).
    pub fn record(&self, signal: Signal) {
        let mut inner = self.inner.lock().expect("limiter mutex poisoned");
        let target = inner.state.observe(signal);
        self.reconcile(&mut inner, target);
    }

    fn reconcile(&self, inner: &mut Inner, target: usize) {
        // Outstanding debt has not been paid yet, so effective capacity is
        // `issued - debt`. Compare against that, not against `issued`.
        let effective = inner.issued.saturating_sub(inner.debt);
        if target > effective {
            let grow = target - effective;
            // Cancel debt before minting new permits.
            let from_debt = grow.min(inner.debt);
            inner.debt -= from_debt;
            let mint = grow - from_debt;
            if mint > 0 {
                inner.issued += mint;
                self.semaphore.add_permits(mint);
            }
        } else if target < effective {
            let mut owe = effective - target;
            // Retire permits that are sitting idle right now. Only permits
            // currently *held* by a running transfer have to be settled later
            // (as debt, on release) — waiting on a release that may never
            // come would leave a shrink silently unapplied while the limiter
            // is quiet.
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

    /// Current cap.
    pub fn current_limit(&self) -> usize {
        let inner = self.inner.lock().expect("limiter mutex poisoned");
        inner.state.limit()
    }

    /// Highest number of slots ever held simultaneously. The assertion hook
    /// for "the cap was respected".
    pub fn peak_in_flight(&self) -> usize {
        self.peak_in_flight.load(Ordering::Acquire)
    }

    fn release(&self, permit: OwnedSemaphorePermit) {
        self.in_flight.fetch_sub(1, Ordering::AcqRel);
        let mut inner = self.inner.lock().expect("limiter mutex poisoned");
        if inner.debt > 0 {
            inner.debt -= 1;
            inner.issued -= 1;
            // Retiring the permit shrinks the semaphore for good.
            permit.forget();
        } else {
            drop(permit);
        }
    }
}

/// A held slot. Dropping it releases (or retires) the underlying permit.
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

/// Classify a download error for the ramp.
///
/// Only load-shaped failures move the limit. A 404 or a sha mismatch says
/// nothing about how many streams the registry can serve, and reacting to it
/// would let one bad artifact throttle every other download in the batch.
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
    // `grpc_err` flattens tonic statuses into plain anyhow errors before most
    // of the codebase sees them, and reqwest errors arrive with their own
    // types, so fall back to the message for the load-shaped cases.
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

/// Run `f` over `items` with adaptive bounded concurrency, preserving input
/// order in the results.
///
/// Every item is driven to its own completion: one failure never cancels a
/// sibling, and the returned `Vec` has exactly one entry per input so callers
/// can report per-item outcomes.
///
/// The driver owns the ramp signals — it calls [`Limiter::note_completion`] on
/// success and classifies failures itself — so `f` only has to report bytes it
/// moved via [`Limiter::add_bytes`], and work that moves none reports nothing.
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
        // The semaphore is the real limit; this only has to be wide enough not to
        // become one itself.
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

    // --- resolve_max_in_flight ---

    #[test]
    fn flag_wins_and_defaults_apply() {
        assert_eq!(resolve_max_in_flight(Some(4)), 4);
        // Env is not set in this test process, so None falls to the default.
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

    // --- RampState (pure policy) ---

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
        // No measurement yet — grow on faith.
        assert_eq!(
            s.observe(Signal::Completed {
                aggregate_rate: None
            }),
            3
        );
        // First real measurement becomes the baseline and earns a slot.
        assert_eq!(
            s.observe(Signal::Completed {
                aggregate_rate: Some(100.0)
            }),
            4
        );
        // +50%: clear win, keep going.
        assert_eq!(
            s.observe(Signal::Completed {
                aggregate_rate: Some(150.0)
            }),
            5
        );
        // +2%: noise, not a win. Hand the slot back and settle.
        assert_eq!(
            s.observe(Signal::Completed {
                aggregate_rate: Some(153.0)
            }),
            4
        );
        assert!(!s.probing(), "should stop probing once it plateaus");
        // Further completions must not move a settled limit.
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
        // Improving forever: the ceiling, not the measurement, must stop it.
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

        // Repeated backpressure decays toward serial but never below 1.
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

    // --- RateWindow ---

    #[test]
    fn rate_needs_two_samples_and_measures_bytes_over_wall_time() {
        let mut w = RateWindow::default();
        let t0 = Instant::now();
        assert_eq!(w.rate(), None, "no samples");
        w.record(t0, 1_000);
        assert_eq!(w.rate(), None, "a single completion has no duration");
        w.record(t0 + Duration::from_secs(2), 4_000);
        // Only the bytes that moved *inside* the span count.
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

    // --- Limiter (async, cap enforcement) ---

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
        // Successes are intact and still correlated with their inputs.
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
        // Later items finish first.
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
        // Grow first so there is something to shrink.
        for _ in 0..4 {
            limiter.add_bytes(1_024 * 1_024);
            limiter.note_completion();
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let grown = limiter.current_limit();
        assert!(grown > INITIAL_IN_FLIGHT, "expected growth, got {grown}");

        limiter.record(Signal::Backpressure);
        assert_eq!(limiter.current_limit(), grown / 2);

        // The tightened cap is actually enforced on subsequent work.
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
        // Regression: a shrink used to be recorded purely as debt, and debt is
        // only settled when a slot is *released*. With nothing in flight there
        // was no release to settle it against, so idle permits stayed
        // available and the tightened cap was silently ignored.
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
        // The other half of the shrink story: permits that are *held* cannot
        // be revoked, so the cap has to tighten as work drains rather than by
        // interrupting a transfer.
        let limiter = Limiter::new(8);
        let gate = Arc::new(tokio::sync::Notify::new());

        // Occupy both starting slots.
        let a = limiter.acquire().await;
        let b = limiter.acquire().await;
        assert_eq!(limiter.peak_in_flight(), 2);

        limiter.record(Signal::Backpressure);
        assert_eq!(limiter.current_limit(), 1);

        // Nothing new may start while both slots are still held.
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

        // Draining one slot pays the debt; it must NOT admit the waiter yet,
        // because the cap is 1 and one slot is still held.
        drop(a);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            !waiter.is_finished(),
            "cap is 1 and a transfer is still running"
        );

        // Draining the last one lets the waiter through.
        drop(b);
        tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("waiter should be admitted once the limiter drains")
            .expect("waiter task panicked");
    }

    #[tokio::test]
    async fn retries_inside_a_slot_are_preserved() {
        // A caller that retries internally must still see its retries run,
        // and must hold exactly one slot for the whole retry sequence.
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
                        // Fail the first two attempts of every item.
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
        // Metadata RPCs (manifest fetches, version lookups) move no bytes, so
        // there is no throughput to hill-climb. Extra concurrent round trips
        // are nearly free there and the win is linear in how many are in
        // flight, so the ramp should open up to the ceiling rather than stall
        // at its starting value.
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
        // Guards the contract that makes the above work: `map_bounded` is what
        // turns a successful unit into a ramp signal. If a closure had to
        // remember to signal, byte-less fan-outs would silently never ramp.
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

    // --- classify ---

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
