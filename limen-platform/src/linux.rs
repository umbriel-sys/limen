//! Linux / desktop platform adapters.
//!
//! > **Status: stub.** The planned `StdClock` implementation (a `PlatformClock`
//! > backed by `std::time::Instant`) is sketched in commented-out code below.
//! > It will be activated once the `P1` `PlatformBackend` finalisation work lands.
// use limen_core::platform::PlatformClock;
// use limen_core::types::Ticks;
// use std::time::Instant;

// /// A monotonic clock wrapper for Linux/desktop using `Instant`.
// #[derive(Debug, Clone)]
// pub struct StdClock {
//     zero: Instant,
// }

// impl StdClock {
//     /// Create a new clock epoch.
//     pub fn new() -> Self {
//         Self {
//             zero: Instant::now(),
//         }
//     }
// }

// impl PlatformClock for StdClock {
//     fn now_ticks(&self) -> Ticks {
//         let dur = self.zero.elapsed();
//         Ticks(dur.as_nanos() as u64)
//     }
//     fn ticks_to_nanos(&self, ticks: Ticks) -> u64 {
//         ticks.0
//     }
//     fn nanos_to_ticks(&self, ns: u64) -> Ticks {
//         Ticks(ns)
//     }
// }
