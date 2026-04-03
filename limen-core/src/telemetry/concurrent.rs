//! Concurrent enabled wrapper for telemetry Implementations.

use crate::prelude::{Telemetry, TelemetryEvent, TelemetryKey};

use std::sync::mpsc;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// Internal message type sent from `TelemetrySender` instances to the
/// single-threaded `TelemetryCore`.
enum TelemetryMsg {
    IncrCounter(TelemetryKey, u64),
    SetGauge(TelemetryKey, u64),
    RecordLatency(TelemetryKey, u64),
    PushMetrics,
    PushEvent(TelemetryEvent),
    Flush,
    Shutdown,
}

/// Single-threaded core that owns the underlying telemetry implementation `T`.
///
/// All metric and event updates are serialized through this core, which runs
/// in a dedicated thread or is driven by the host runtime.
pub struct TelemetryCore<T: Telemetry> {
    inner: T,
    rx: mpsc::Receiver<TelemetryMsg>,
    events_flag: Arc<AtomicBool>,
}

impl<T: Telemetry> TelemetryCore<T> {
    /// Construct a new `TelemetryCore` around `inner`, using the given receiver.
    ///
    /// Returns the core and an `Arc<AtomicBool>` that mirrors
    /// `inner.events_enabled()` for cheap access on the sender side.
    fn new(inner: T, rx: mpsc::Receiver<TelemetryMsg>) -> (Self, Arc<AtomicBool>) {
        let events_flag = Arc::new(AtomicBool::new(inner.events_enabled()));
        (
            Self {
                inner,
                rx,
                events_flag: events_flag.clone(),
            },
            events_flag,
        )
    }

    /// Drive the telemetry core loop until all senders are dropped or a
    /// `Shutdown` message is received.
    pub fn run(mut self) {
        while let Ok(message) = self.rx.recv() {
            let shutdown_requested = match message {
                TelemetryMsg::IncrCounter(telemetry_key, counter_delta) if T::METRICS_ENABLED => {
                    self.inner.incr_counter(telemetry_key, counter_delta);
                    false
                }
                TelemetryMsg::SetGauge(telemetry_key, gauge_value) if T::METRICS_ENABLED => {
                    self.inner.set_gauge(telemetry_key, gauge_value);
                    false
                }
                TelemetryMsg::RecordLatency(telemetry_key, latency_value_ns)
                    if T::METRICS_ENABLED =>
                {
                    self.inner
                        .record_latency_ns(telemetry_key, latency_value_ns);
                    false
                }
                TelemetryMsg::PushMetrics => {
                    self.inner.push_metrics();
                    false
                }
                TelemetryMsg::PushEvent(telemetry_event) if T::EVENTS_STATICALLY_ENABLED => {
                    self.inner.push_event(telemetry_event);
                    false
                }
                TelemetryMsg::Flush => {
                    self.inner.flush();
                    false
                }
                TelemetryMsg::Shutdown => true,
                _ => false,
            };

            self.events_flag
                .store(self.inner.events_enabled(), Ordering::Relaxed);

            if shutdown_requested {
                while let Ok(queued_message) = self.rx.try_recv() {
                    match queued_message {
                        TelemetryMsg::IncrCounter(telemetry_key, counter_delta)
                            if T::METRICS_ENABLED =>
                        {
                            self.inner.incr_counter(telemetry_key, counter_delta);
                        }
                        TelemetryMsg::SetGauge(telemetry_key, gauge_value)
                            if T::METRICS_ENABLED =>
                        {
                            self.inner.set_gauge(telemetry_key, gauge_value);
                        }
                        TelemetryMsg::RecordLatency(telemetry_key, latency_value_ns)
                            if T::METRICS_ENABLED =>
                        {
                            self.inner
                                .record_latency_ns(telemetry_key, latency_value_ns);
                        }
                        TelemetryMsg::PushMetrics => {
                            self.inner.push_metrics();
                        }
                        TelemetryMsg::PushEvent(telemetry_event)
                            if T::EVENTS_STATICALLY_ENABLED =>
                        {
                            self.inner.push_event(telemetry_event);
                        }
                        TelemetryMsg::Flush => {
                            self.inner.flush();
                        }
                        TelemetryMsg::Shutdown => {}
                        _ => {}
                    }

                    self.events_flag
                        .store(self.inner.events_enabled(), Ordering::Relaxed);
                }

                self.inner.flush();
                return;
            }
        }

        self.inner.flush();
    }
}

/// Sender-side handle that implements `Telemetry` and forwards all calls
/// to the core via a multi-producer, single-consumer channel.
pub struct TelemetrySender<T: Telemetry> {
    tx: mpsc::Sender<TelemetryMsg>,
    events_flag: Arc<AtomicBool>,
    _marker: core::marker::PhantomData<T>,
}

impl<T: Telemetry> TelemetrySender<T> {
    /// Send an explicit shutdown message to the telemetry core.
    ///
    /// After this call, the core will exit its run loop once it processes
    /// the message and perform a final flush.
    pub fn send_shutdown(&self) {
        let _ = self.tx.send(TelemetryMsg::Shutdown);
    }
}

impl<T: Telemetry> Clone for TelemetrySender<T> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            events_flag: self.events_flag.clone(),
            _marker: core::marker::PhantomData,
        }
    }
}

impl<T: Telemetry> Telemetry for TelemetrySender<T> {
    const METRICS_ENABLED: bool = T::METRICS_ENABLED;
    const EVENTS_STATICALLY_ENABLED: bool = T::EVENTS_STATICALLY_ENABLED;

    #[inline]
    fn incr_counter(&mut self, key: TelemetryKey, delta: u64) {
        if !Self::METRICS_ENABLED {
            return;
        }
        // For very hot paths consider mpsc::Sender::send vs try_send on a bounded channel.
        let _ = self.tx.send(TelemetryMsg::IncrCounter(key, delta));
    }

    #[inline]
    fn set_gauge(&mut self, key: TelemetryKey, value: u64) {
        if !Self::METRICS_ENABLED {
            return;
        }
        let _ = self.tx.send(TelemetryMsg::SetGauge(key, value));
    }

    #[inline]
    fn record_latency_ns(&mut self, key: TelemetryKey, value_ns: u64) {
        if !Self::METRICS_ENABLED {
            return;
        }
        let _ = self.tx.send(TelemetryMsg::RecordLatency(key, value_ns));
    }

    #[inline]
    fn push_metrics(&mut self) {
        let _ = self.tx.send(TelemetryMsg::PushMetrics);
    }

    #[inline]
    fn events_enabled(&self) -> bool {
        Self::EVENTS_STATICALLY_ENABLED && self.events_flag.load(Ordering::Relaxed)
    }

    #[inline]
    fn push_event(&mut self, event: TelemetryEvent) {
        if !self.events_enabled() {
            return;
        }
        let _ = self.tx.send(TelemetryMsg::PushEvent(event));
    }

    #[inline]
    fn flush(&mut self) {
        let _ = self.tx.send(TelemetryMsg::Flush);
    }
}

/// Handle returned by `spawn_telemetry_core`, owning the core thread and
/// providing a clonable sender for worker threads.
pub struct TelemetryCoreHandle<T: Telemetry> {
    sender: TelemetrySender<T>,
    join_handle: Option<std::thread::JoinHandle<()>>,
}

/// Construct a telemetry core and its corresponding sender without spawning
/// a dedicated thread.
///
/// This is useful when the host runtime wants to own the core and drive
/// `TelemetryCore::run()` or a custom loop itself.
pub fn new_telemetry_pair<T: Telemetry>(inner: T) -> (TelemetryCore<T>, TelemetrySender<T>) {
    let (tx, rx) = mpsc::channel::<TelemetryMsg>();
    let (core, events_flag) = TelemetryCore::new(inner, rx);
    let sender = TelemetrySender {
        tx,
        events_flag,
        _marker: core::marker::PhantomData,
    };
    (core, sender)
}

/// Spawn a dedicated thread that owns `inner` and processes telemetry messages,
/// returning a handle that can be used to obtain senders and shut the core down.
pub fn spawn_telemetry_core<T>(inner: T) -> TelemetryCoreHandle<T>
where
    T: Telemetry + Send + 'static,
{
    let (core, sender) = new_telemetry_pair(inner);
    let join_handle = std::thread::spawn(move || core.run());
    TelemetryCoreHandle {
        sender,
        join_handle: Some(join_handle),
    }
}

impl<T: Telemetry> TelemetryCoreHandle<T> {
    /// Return a clonable `TelemetrySender<T>` that can be passed to nodes.
    #[inline]
    pub fn sender(&self) -> TelemetrySender<T> {
        self.sender.clone()
    }

    /// Send a shutdown signal to the core and join the thread.
    ///
    /// This should be called during runtime shutdown to ensure that all
    /// telemetry is flushed before exit.
    pub fn shutdown_and_join(mut self) {
        self.sender.send_shutdown();
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}
