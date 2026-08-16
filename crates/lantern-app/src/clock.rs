use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use tokio::sync::Notify;

/// Boxed future returned by the application-owned monotonic clock.
pub type ClockFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

/// The single monotonic-time port used by transport, simulation, and planners.
///
/// Wall-clock time is intentionally excluded. Deadlines and protocol delays must
/// never jump when the system clock is adjusted.
pub trait MonotonicClock: Send + Sync {
    /// Returns the current monotonic instant.
    fn now(&self) -> Instant;

    /// Sleeps until the supplied monotonic deadline.
    fn sleep_until(&self, deadline: Instant) -> ClockFuture<'_>;

    /// Sleeps for a monotonic duration.
    fn sleep(&self, duration: Duration) -> ClockFuture<'_> {
        self.sleep_until(self.now() + duration)
    }
}

/// Production monotonic clock backed by Tokio timers.
#[derive(Clone, Copy, Debug, Default)]
pub struct TokioMonotonicClock;

impl MonotonicClock for TokioMonotonicClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sleep_until(&self, deadline: Instant) -> ClockFuture<'_> {
        Box::pin(tokio::time::sleep_until(tokio::time::Instant::from_std(
            deadline,
        )))
    }
}

/// Deterministic monotonic clock for scenario and scheduler tests.
///
/// Time advances only through [`Self::advance`]. Sleeping tasks are woken after
/// each advancement and re-check their deadline, avoiding a second test-only
/// timing model outside the application layer.
#[derive(Clone, Debug)]
pub struct ManualMonotonicClock {
    origin: Instant,
    state: Arc<ManualClockState>,
}

#[derive(Debug)]
struct ManualClockState {
    elapsed: Mutex<Duration>,
    changed: Notify,
}

impl Default for ManualMonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

impl ManualMonotonicClock {
    /// Creates a clock at an arbitrary monotonic origin.
    #[must_use]
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
            state: Arc::new(ManualClockState {
                elapsed: Mutex::new(Duration::ZERO),
                changed: Notify::new(),
            }),
        }
    }

    /// Advances time and wakes all tasks waiting on monotonic deadlines.
    pub fn advance(&self, duration: Duration) {
        let mut elapsed = self
            .state
            .elapsed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *elapsed = elapsed.saturating_add(duration);
        drop(elapsed);
        self.state.changed.notify_waiters();
    }

    /// Returns elapsed deterministic time since this clock was created.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        *self
            .state
            .elapsed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl MonotonicClock for ManualMonotonicClock {
    fn now(&self) -> Instant {
        self.origin + self.elapsed()
    }

    fn sleep_until(&self, deadline: Instant) -> ClockFuture<'_> {
        Box::pin(async move {
            loop {
                if self.now() >= deadline {
                    return;
                }
                let changed = self.state.changed.notified();
                if self.now() >= deadline {
                    return;
                }
                changed.await;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{ManualMonotonicClock, MonotonicClock};

    #[tokio::test]
    async fn manual_clock_wakes_only_after_the_deadline() {
        let clock = ManualMonotonicClock::new();
        let waiter_clock = clock.clone();
        let task = tokio::spawn(async move {
            waiter_clock.sleep(Duration::from_millis(10)).await;
        });

        tokio::task::yield_now().await;
        clock.advance(Duration::from_millis(9));
        tokio::task::yield_now().await;
        assert!(!task.is_finished());

        clock.advance(Duration::from_millis(1));
        task.await.expect("clock waiter");
    }
}
