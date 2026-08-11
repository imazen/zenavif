//! Bridging a borrowed [`Stop`] into APIs that require an owned, `'static` one.
//!
//! # The problem this solves
//!
//! zenavif's decode entry points take `stop: &(impl Stop + ?Sized)` — a
//! borrow, tied to the call. Two of the three AV1 decode backends are happy
//! with that:
//!
//! | backend | how it takes a token | cancellation granularity |
//! |---|---|---|
//! | zenav1-aom (`AomRs`) | borrowed `&dyn Stop` | SB-row / tile / frame, in-flight |
//! | rav1d-safe (`Rav1dSafe`) | **owned `Arc<dyn Stop>`** via `Decoder::set_stop` | superblock-row, in-flight |
//! | rav1d C FFI (`Rav1dFfi`) | takes none | none |
//!
//! rav1d-safe's managed API shares the token with its tile worker threads, so
//! it reasonably demands `Arc<dyn Stop>`, which implies `'static`. A borrowed
//! `&impl Stop` cannot become one. Before this module, zenavif simply never
//! called `set_stop`, so the caller's token was checked only at the *phase*
//! boundaries around the decode (pre-decode, per-tile, per-frame, per-strip)
//! and a single frame decode was uninterruptible — tens to hundreds of
//! milliseconds at 4K, however promptly the caller cancelled.
//!
//! # The bridge
//!
//! [`with_relayed_stop`] runs a closure with an `Arc<dyn Stop>` whose state
//! mirrors the borrowed token, kept in sync by a watcher on a **scoped**
//! thread. Scoped threads are what make this sound without any `'static`
//! requirement or `unsafe`: `Stop: Send + Sync`, so `&S` is `Send`, and
//! `std::thread::scope` guarantees the watcher is joined before the borrow
//! ends.
//!
//! Two properties keep the cost honest:
//!
//! * **Unstoppable callers pay nothing.** If `stop.may_stop()` is false (the
//!   [`Unstoppable`](enough::Unstoppable) case, which is the common one), no
//!   thread is spawned and the closure receives `None` — byte-identical,
//!   allocation-free, exactly the old behavior.
//! * **Shutdown does not wait out a sleep.** The watcher parks on a condvar
//!   with a timeout, so the poll interval bounds *detection* latency while
//!   teardown is immediate when the closure returns.
//!
//! End-to-end cancellation latency is therefore roughly
//! `POLL_INTERVAL + (backend's own check spacing)`, instead of "however long
//! the rest of this frame takes". The measured numbers live in
//! `benches/cancel_latency.rs` — this doc deliberately states the mechanism,
//! not a latency figure, so the two cannot drift apart.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use enough::{Stop, StopReason};

/// How often the watcher samples the caller's token.
///
/// This is the dominant term in cancellation latency for the rav1d-safe
/// backend, so it is deliberately short. The cost of a shorter interval is
/// only wakeups on one otherwise-idle thread for the duration of a decode —
/// at 1 ms that is ~30 wakeups for a 30 ms 4K frame, which does not measurably
/// perturb the decode (see the bench's uncancelled-throughput control).
const POLL_INTERVAL: Duration = Duration::from_millis(1);

/// A `Stop` whose answer is a single atomic flag, set by the watcher thread.
///
/// Deliberately minimal: the hot path here is rav1d-safe polling
/// `should_stop()` once per superblock row on every tile worker, so this must
/// be a plain relaxed load with no locking and no allocation.
#[derive(Debug, Default)]
pub(crate) struct RelayStop {
    stopped: AtomicBool,
}

impl RelayStop {
    fn signal(&self) {
        // Release so that everything the watcher observed before deciding to
        // stop happens-before a worker's acquire-load below.
        self.stopped.store(true, Ordering::Release);
    }
}

impl Stop for RelayStop {
    #[inline]
    fn check(&self) -> Result<(), StopReason> {
        if self.stopped.load(Ordering::Acquire) {
            // The relay cannot distinguish cancellation from a deadline: the
            // borrowed token's own reason is not carried across, and callers
            // map both to `Error::Cancelled` anyway. `Cancelled` is the
            // accurate reason at this boundary — same mapping
            // `error_from_rav1d` uses for rav1d-safe's own `Cancelled`.
            Err(StopReason::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Whether `ZENAVIF_CANCEL_RELAY=0` asked for the pre-relay behavior.
///
/// Read once and cached: this sits in front of every frame decode, and reading
/// the environment is neither cheap nor thread-safe to do repeatedly.
fn relay_disabled() -> bool {
    use std::sync::OnceLock;
    static DISABLED: OnceLock<bool> = OnceLock::new();
    *DISABLED
        .get_or_init(|| std::env::var("ZENAVIF_CANCEL_RELAY").is_ok_and(|v| v == "0" || v == "off"))
}

/// Parking spot for the watcher: lets the closure wake it immediately on
/// completion instead of waiting out a `POLL_INTERVAL` sleep.
#[derive(Default)]
struct Shutdown {
    done: Mutex<bool>,
    cv: Condvar,
}

impl Shutdown {
    fn finish(&self) {
        // A poisoned mutex here would mean the closure panicked while we held
        // nothing; recover rather than cascade a second panic out of teardown.
        let mut done = self.done.lock().unwrap_or_else(|e| e.into_inner());
        *done = true;
        drop(done);
        self.cv.notify_all();
    }

    /// Sleep up to `POLL_INTERVAL`, returning `true` if work is finished.
    fn wait_or_timeout(&self) -> bool {
        let done = self.done.lock().unwrap_or_else(|e| e.into_inner());
        if *done {
            return true;
        }
        let (done, _) = self
            .cv
            .wait_timeout(done, POLL_INTERVAL)
            .unwrap_or_else(|e| e.into_inner());
        *done
    }
}

/// Run `f` with an owned token that mirrors `stop`.
///
/// `f` receives `None` when `stop` can never fire, and `Some(token)`
/// otherwise. Pass the token straight to `Decoder::set_stop`.
///
/// The relay is dropped, and its watcher joined, before this returns — so the
/// decoder must not outlive the call. That is exactly how the managed decoder
/// is used here (constructed, driven, and finished inside one call), and it is
/// why the token can be a borrow at the public boundary at all.
pub(crate) fn with_relayed_stop<S, R>(stop: &S, f: impl FnOnce(Option<Arc<dyn Stop>>) -> R) -> R
where
    S: Stop + ?Sized,
{
    // Fast path: an `Unstoppable` (or otherwise inert) token needs no relay,
    // no thread, and no atomic in the decoder's inner loop.
    if !stop.may_stop() {
        return f(None);
    }

    // Measurement escape hatch, not a tuning knob: setting ZENAVIF_CANCEL_RELAY=0
    // reproduces the pre-relay behavior exactly (the decoder is never given a
    // token, so the caller is only polled at phase boundaries). It exists so
    // examples/cancel_latency.rs can measure before-and-after in ONE binary
    // rather than quoting a number from a different build. Default is on.
    if relay_disabled() {
        return f(None);
    }

    let relay = Arc::new(RelayStop::default());
    let shutdown = Shutdown::default();

    std::thread::scope(|scope| {
        let watcher_relay = Arc::clone(&relay);
        let watcher_shutdown = &shutdown;
        scope.spawn(move || {
            loop {
                if stop.should_stop() {
                    watcher_relay.signal();
                    // Nothing left to watch: the flag is one-way.
                    return;
                }
                if watcher_shutdown.wait_or_timeout() {
                    return;
                }
            }
        });

        // `Arc<RelayStop>` -> `Arc<dyn Stop>`.
        let token: Arc<dyn Stop> = Arc::clone(&relay) as Arc<dyn Stop>;
        // Run the work even if it unwinds — the scope joins the watcher either
        // way, and `finish()` is what stops it parking for a final interval.
        let out = f(Some(token));
        shutdown.finish();
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use almost_enough::StopSource;
    use std::time::Instant;

    #[test]
    fn unstoppable_callers_get_no_relay_and_no_thread() {
        let seen = with_relayed_stop(&enough::Unstoppable, |token| token.is_some());
        assert!(
            !seen,
            "an inert token must not allocate a relay — the decode inner loop \
             should keep polling nothing"
        );
    }

    #[test]
    fn stoppable_callers_get_a_relay_that_starts_unfired() {
        with_relayed_stop(&StopSource::new(), |token| {
            let token = token.expect("a live token must be relayed");
            assert!(
                token.check().is_ok(),
                "relay must not report a stop before the source fires"
            );
        });
    }

    #[test]
    fn relay_observes_a_cancel_that_happens_during_the_work() {
        let source = StopSource::new();
        with_relayed_stop(&source, |token| {
            let token = token.expect("a live token must be relayed");
            assert!(token.check().is_ok(), "starts unfired");
            source.cancel();
            // Bounded spin: the watcher must pick this up within a few poll
            // intervals. A fixed sleep would either be flaky or slow; this
            // fails loudly instead of silently passing on a slow machine.
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if token.check().is_err() {
                    return;
                }
                std::thread::yield_now();
            }
            panic!("relay never observed the cancel within 5s");
        });
    }

    #[test]
    fn relay_reports_cancelled_as_the_reason() {
        let source = StopSource::new();
        source.cancel();
        with_relayed_stop(&source, |token| {
            let token = token.expect("a live token must be relayed");
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match token.check() {
                    Err(reason) => {
                        assert_eq!(reason, StopReason::Cancelled);
                        return;
                    }
                    Ok(()) if Instant::now() < deadline => std::thread::yield_now(),
                    Ok(()) => panic!("relay never fired for an already-cancelled source"),
                }
            }
        });
    }

    #[test]
    fn work_that_finishes_first_tears_the_watcher_down_promptly() {
        // Regression guard for the "watcher sleeps out a full interval on every
        // decode" shape (the pre-condvar design: sleep POLL_INTERVAL, *then*
        // look at `done`). Teardown here is condvar-driven — `finish()` sets
        // `done` under the lock and notifies, and `wait_or_timeout()` re-checks
        // `done` under the same lock before parking, so the wakeup cannot be
        // lost — and a no-op body must therefore not cost anything like
        // POLL_INTERVAL.
        //
        // What this must NOT charge us for is creating and joining the watcher
        // thread, which is the platform's cost and not the relay's. The
        // previous form of this test — `20 relayed no-op calls <
        // 20 * POLL_INTERVAL` — did exactly that and failed the
        // windows-latest leg at 27.93 ms for 20 calls (CI run 31520483088),
        // i.e. ~1.4 ms per scope+spawn+join round trip, while *reporting* it as
        // "teardown is waiting out the poll interval". That diagnosis was not
        // something the measurement could support.
        //
        // So: measure against a control that spawns and joins a scoped thread
        // doing nothing else, interleaved rep for rep, and compare the two
        // MINIMA. A watcher that sleeps out the interval pays POLL_INTERVAL on
        // *every* iteration, so it lifts the floor by a full interval —
        // something scheduling noise on a loaded shared runner cannot do, and
        // something a slow thread-create cannot hide, because the control pays
        // that too. The failure message prints both numbers so a red CI run
        // says which term grew.
        let source = StopSource::new();
        let reps = 20;
        let mut relay_min = Duration::MAX;
        let mut control_min = Duration::MAX;
        for _ in 0..reps {
            let t = Instant::now();
            with_relayed_stop(&source, |token| {
                // Anti-vacuity: a `None` here would mean we timed the
                // no-thread fast path and proved nothing about teardown.
                assert!(
                    token.is_some(),
                    "a live StopSource must be relayed — otherwise this test \
                     measures the unstoppable fast path"
                );
            });
            relay_min = relay_min.min(t.elapsed());

            let t = Instant::now();
            std::thread::scope(|scope| {
                scope.spawn(|| {});
            });
            control_min = control_min.min(t.elapsed());
        }
        let overhead = relay_min.saturating_sub(control_min);
        assert!(
            relay_min < control_min + POLL_INTERVAL / 2,
            "fastest of {reps} relayed no-op calls was {relay_min:?} vs \
             {control_min:?} for a bare scoped spawn+join — the relay adds \
             {overhead:?} on top of the platform's thread cost, and \
             POLL_INTERVAL is {POLL_INTERVAL:?}. Teardown is waiting out the \
             poll interval instead of being signalled."
        );
    }
}
