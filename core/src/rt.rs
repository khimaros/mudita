//! what the core needs of an async runtime, and nothing more: time, timers,
//! tasks, and a flag to wait on. all of it goes through veilid-tools, which
//! runs on tokio natively and on the browser's event loop as wasm, so the
//! core names no runtime of its own.

use crate::Result;
use event_listener::Event;
use futures_util::future::select;
pub use futures_util::future::Either;
use futures_util::pin_mut;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use veilid_core::tools::{get_raw_timestamp, random_bytes};

const TASK_NAME: &str = "mudita";
const MICROS_PER_MILLI: u64 = 1000;
const MILLIS_PER_SECOND: u64 = 1000;
const RETRY_ATTEMPTS: u32 = 6;
pub const RETRY_DELAY: Duration = Duration::from_secs(1);
pub const MAX_RETRY_DELAY_FACTOR: u32 = 5;

/// milliseconds since the unix epoch. the system clock is all a browser
/// offers, so durations are measured with it too.
pub fn now_ms() -> u64 {
    get_raw_timestamp() / MICROS_PER_MILLI
}

pub fn unix_now() -> u64 {
    now_ms() / MILLIS_PER_SECOND
}

fn millis(duration: Duration) -> u32 {
    duration.as_millis().try_into().unwrap_or(u32::MAX)
}

pub async fn sleep(duration: Duration) {
    veilid_core::tools::sleep(millis(duration)).await;
}

/// what `future` gives, or None if it takes longer than `duration`.
pub async fn timeout<T>(duration: Duration, future: impl Future<Output = T>) -> Option<T> {
    veilid_core::tools::timeout(millis(duration), future)
        .await
        .ok()
}

/// run a future in the background, unattended.
pub fn spawn(future: impl Future<Output = ()> + Send + 'static) {
    veilid_core::tools::spawn_detached(TASK_NAME, future);
}

/// run two futures side by side until one is done, and drop the other.
pub async fn race<A, B>(a: impl Future<Output = A>, b: impl Future<Output = B>) -> Either<A, B> {
    pin_mut!(a, b);
    match select(a, b).await {
        Either::Left((a, _)) => Either::Left(a),
        Either::Right((b, _)) => Either::Right(b),
    }
}

/// random bytes: enough to pick a slot or tell downloads apart.
pub fn unique_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    random_bytes(&mut bytes);
    bytes
}

/// run f until it succeeds or has failed `attempts` times, backing off
/// between attempts by a delay that grows up to a cap.
pub async fn retry_times<T, F, Fut>(attempts: u32, mut f: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let mut attempt = 1;
    loop {
        match f().await {
            Err(_) if attempt < attempts => {
                sleep(RETRY_DELAY * attempt.min(MAX_RETRY_DELAY_FACTOR)).await
            }
            outcome => return outcome,
        }
        attempt += 1;
    }
}

pub async fn retry<T, F, Fut>(f: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    retry_times(RETRY_ATTEMPTS, f).await
}

/// something that happens once and stays so: whoever waits on it after it
/// was raised does not wait at all.
#[derive(Default)]
pub struct Flag {
    raised: AtomicBool,
    event: Event,
}

impl Flag {
    pub fn raise(&self) {
        self.raised.store(true, Ordering::SeqCst);
        self.event.notify(usize::MAX);
    }

    pub fn is_raised(&self) -> bool {
        self.raised.load(Ordering::SeqCst)
    }

    pub async fn wait(&self) {
        while !self.is_raised() {
            let raised = self.event.listen();
            // raised between the check and the listen: nobody would wake us.
            if self.is_raised() {
                return;
            }
            raised.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;
    use std::sync::Arc;

    const WAITERS: usize = 8;
    const RACES: usize = 200;
    // far longer than a wakeup takes, and short enough to fail a test by.
    const WOKEN_WITHIN: Duration = Duration::from_secs(10);

    struct Unpark(std::thread::Thread);

    impl std::task::Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }

    // run a future to its end on this thread, sleeping between wakeups.
    fn block_on<T>(future: impl Future<Output = T>) -> T {
        let waker = Arc::new(Unpark(std::thread::current())).into();
        let mut context = std::task::Context::from_waker(&waker);
        pin_mut!(future);
        loop {
            match future.as_mut().poll(&mut context) {
                std::task::Poll::Ready(output) => return output,
                std::task::Poll::Pending => std::thread::park(),
            }
        }
    }

    #[test]
    fn a_raised_flag_is_not_waited_on() {
        let flag = Flag::default();
        assert!(flag.wait().now_or_never().is_none());
        flag.raise();
        assert!(flag.is_raised());
        assert!(flag.wait().now_or_never().is_some());
    }

    #[test]
    fn a_flag_raised_while_it_is_waited_on_wakes_the_waiter() {
        let flag = Flag::default();
        let waiting = flag.wait();
        pin_mut!(waiting);
        assert!(waiting.as_mut().now_or_never().is_none());
        flag.raise();
        assert!(waiting.now_or_never().is_some());
    }

    // however a raise and the waits fall against each other, every waiter
    // comes back: a wakeup missed between the check and the listen would
    // leave one parked for good.
    #[test]
    fn every_waiter_is_woken_whenever_the_flag_is_raised() {
        for _ in 0..RACES {
            let flag = Arc::new(Flag::default());
            let (woken_tx, woken) = std::sync::mpsc::channel();
            for _ in 0..WAITERS {
                let (flag, woken_tx) = (flag.clone(), woken_tx.clone());
                std::thread::spawn(move || {
                    block_on(flag.wait());
                    let _ = woken_tx.send(());
                });
            }
            flag.raise();
            for _ in 0..WAITERS {
                assert!(woken.recv_timeout(WOKEN_WITHIN).is_ok());
            }
        }
    }

    #[test]
    fn unique_bytes_differ_from_call_to_call() {
        assert_ne!(unique_bytes::<8>(), unique_bytes::<8>());
    }
}
