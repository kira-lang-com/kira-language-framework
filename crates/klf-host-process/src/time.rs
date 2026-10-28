//! The host side of the `klf.time` kernel services.

use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static ORIGIN: OnceLock<Instant> = OnceLock::new();

/// Nanoseconds since the first clock read in this process. It never
/// decreases, so differences between two reads are durations.
#[must_use]
pub fn monotonic_nanos() -> i64 {
    let origin = *ORIGIN.get_or_init(Instant::now);
    i64::try_from(origin.elapsed().as_nanos()).unwrap_or(i64::MAX)
}

/// Nanoseconds since the Unix epoch, negative before it.
#[must_use]
pub fn wall_nanos() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_nanos()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_nanos()).map_or(i64::MIN, |nanos| -nanos),
    }
}

/// Pauses the calling thread for at least `nanos` nanoseconds; a count that
/// is not positive returns at once.
pub fn sleep_nanos(nanos: i64) {
    if let Ok(nanos) = u64::try_from(nanos) {
        std::thread::sleep(Duration::from_nanos(nanos));
    }
}
