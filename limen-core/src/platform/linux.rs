//! Platform impls for linux systems.

use crate::platform::PlatformClock;
use crate::types::Ticks;

/// Number of nanoseconds in one second.
const NANOSECONDS_PER_SECOND: u128 = 1_000_000_000;

/// Linux monotonic clock implementation backed by `clock_gettime(CLOCK_MONOTONIC)`.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoStdLinuxMonotonicClock;

impl NoStdLinuxMonotonicClock {
    /// Create a new `NoStdLinuxMonotonicClock` instance.
    #[inline]
    pub const fn new() -> NoStdLinuxMonotonicClock {
        NoStdLinuxMonotonicClock
    }

    /// Read the current monotonic time in nanoseconds since an unspecified epoch.
    ///
    /// This is a best-effort implementation. On any detected error or invalid
    /// `timespec` values, it returns `0` rather than invoking undefined behavior.
    #[allow(unsafe_code)]
    #[inline]
    fn read_monotonic_nanoseconds() -> u64 {
        // Initialize to a known state so we do not need MaybeUninit or assume_init.
        let mut timespec_value = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };

        // The only unsafe in this module: the FFI call itself.
        let return_code =
            unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut timespec_value) };

        // On error, fall back to 0. The trait does not allow us to return a Result.
        if return_code != 0 {
            return 0;
        }

        // Defensive validation of the returned timespec.
        if timespec_value.tv_sec < 0 as libc::time_t {
            return 0;
        }

        if timespec_value.tv_nsec < 0 as libc::c_long
            || timespec_value.tv_nsec >= 1_000_000_000 as libc::c_long
        {
            return 0;
        }

        let seconds_as_u64 = timespec_value.tv_sec as u64;
        let nanoseconds_as_u64 = timespec_value.tv_nsec as u64;

        // Compute total nanoseconds in u128, then saturate down to u64.
        let total_nanoseconds_as_u128 = (seconds_as_u64 as u128)
            .saturating_mul(NANOSECONDS_PER_SECOND)
            .saturating_add(nanoseconds_as_u64 as u128);

        if total_nanoseconds_as_u128 > u64::MAX as u128 {
            u64::MAX
        } else {
            total_nanoseconds_as_u128 as u64
        }
    }
}

impl PlatformClock for NoStdLinuxMonotonicClock {
    #[inline]
    fn now_ticks(&self) -> Ticks {
        // Define ticks as nanoseconds of CLOCK_MONOTONIC.
        Ticks::new(Self::read_monotonic_nanoseconds())
    }

    #[inline]
    fn ticks_to_nanos(&self, ticks: Ticks) -> u64 {
        // Identity mapping: ticks are nanoseconds.
        ticks.as_u64()
    }

    #[inline]
    fn nanos_to_ticks(&self, nanoseconds: u64) -> Ticks {
        // Identity mapping: ticks are nanoseconds.
        Ticks::new(nanoseconds)
    }
}

/// Monotonic clock backed by `std::time::Instant`.
///
/// Ticks are defined as nanoseconds elapsed since the clock was constructed.
/// This is strictly monotonic (subject to `Instant`'s guarantees).
#[cfg(feature = "std")]
#[derive(Debug, Clone)]
pub struct StdLinuxMonotonicClock {
    origin: std::time::Instant,
}

#[cfg(feature = "std")]
impl StdLinuxMonotonicClock {
    /// Create a new `StdLinuxMonotonicClock` with the current instant as origin.
    #[inline]
    pub fn new() -> StdLinuxMonotonicClock {
        StdLinuxMonotonicClock {
            origin: std::time::Instant::now(),
        }
    }
}

#[cfg(feature = "std")]
impl Default for StdLinuxMonotonicClock {
    #[inline]
    fn default() -> StdLinuxMonotonicClock {
        StdLinuxMonotonicClock::new()
    }
}

#[cfg(feature = "std")]
impl PlatformClock for StdLinuxMonotonicClock {
    #[inline]
    fn now_ticks(&self) -> Ticks {
        let duration_since_origin = std::time::Instant::now().duration_since(self.origin);

        // Duration::as_nanos returns u128.
        let total_nanoseconds_as_u128 = duration_since_origin.as_nanos();

        // Saturate down to u64 if it ever exceeds the range.
        if total_nanoseconds_as_u128 > u64::MAX as u128 {
            Ticks::new(u64::MAX)
        } else {
            Ticks::new(total_nanoseconds_as_u128 as u64)
        }
    }

    #[inline]
    fn ticks_to_nanos(&self, ticks: Ticks) -> u64 {
        // Identity mapping: ticks are nanoseconds.
        ticks.as_u64()
    }

    #[inline]
    fn nanos_to_ticks(&self, nanoseconds: u64) -> Ticks {
        // Identity mapping: ticks are nanoseconds.
        Ticks::new(nanoseconds)
    }
}
