//! `birta review --wait`: a sleep loop with no runtime. It holds no lock between ticks,
//! so it never stalls the browser's writes.

use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How often a waiter looks again, whether or not the file changed.
const TICK: Duration = Duration::from_millis(250);
/// Matches the claim-release threshold, so an orphaned waiter can't hold a hand-back longer.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(900);

static SIGNAL: AtomicI32 = AtomicI32::new(0);

/// How long to wait: 900 s unless `--timeout` says otherwise; `None` is forever.
pub fn timeout(secs: Option<u64>) -> Option<Duration> {
    match secs {
        None => Some(DEFAULT_TIMEOUT),
        Some(0) => None,
        Some(secs) => Some(Duration::from_secs(secs)),
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Calls `poll` with the time every tick until it returns something, or `None` once
/// `limit` has passed. SIGINT and SIGTERM end the process at once, silently: a waiter
/// writes nothing while it waits, so there is nothing to clean up. A poll blocked on a
/// lock is interrupted too, and only a poll that found nothing ends this way.
pub fn until<T>(
    limit: Option<Duration>,
    mut poll: impl FnMut(u64) -> anyhow::Result<Option<T>>,
) -> anyhow::Result<Option<T>> {
    catch_signals();
    let started = Instant::now();
    loop {
        exit_if_signalled();
        match poll(now_ms()) {
            Ok(Some(done)) => return Ok(Some(done)),
            Ok(None) => exit_if_signalled(),
            Err(e) => {
                exit_if_signalled();
                return Err(e);
            }
        }
        let left = limit.map(|limit| limit.saturating_sub(started.elapsed()));
        if left == Some(Duration::ZERO) {
            return Ok(None);
        }
        std::thread::sleep(left.map_or(TICK, |left| left.min(TICK)));
    }
}

fn exit_if_signalled() {
    let signal = SIGNAL.load(Ordering::Relaxed);
    if signal != 0 {
        std::process::exit(128 + signal);
    }
}

extern "C" fn on_signal(signal: libc::c_int) {
    SIGNAL.store(signal, Ordering::Relaxed);
}

/// Installs the handlers without `SA_RESTART`, so a blocked `flock` returns `EINTR`
/// instead of waiting on.
fn catch_signals() {
    let handler = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
    for signal in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: `action` is plain data, for which all zeroes is a valid value; the
        // handler only stores to an atomic, which is async-signal-safe.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = handler;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(signal, &action, std::ptr::null_mut());
        }
    }
}
