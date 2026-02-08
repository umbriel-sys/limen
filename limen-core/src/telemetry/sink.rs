//! Telemetry sink trait and implementations.

use super::*;

/// Error type returned by event writers when writing fails.
///
/// Implementations use this to signal input output failures or buffer
/// exhaustion without tying the core telemetry code to any particular
/// error type.
#[non_exhaustive]
#[derive(Copy, Clone, Debug)]
pub enum TelemetrySinkError {
    /// The event could not be pushed to the underlying sink.
    PushFailed,
}

/// Write-only sink for structured telemetry events.
///
/// Event writers are responsible for transporting events to logs, sockets,
/// buffers, or any other destination. They are deliberately minimal and can
/// be implemented in both no_std and std environments.
pub trait TelemetrySink {
    /// Write a single telemetry event to the underlying sink.
    #[inline]
    fn push_event(&mut self, _event: &TelemetryEvent) -> Result<(), TelemetrySinkError> {
        Ok(())
    }

    /// Write a snapshot of the current graph metrics.
    ///
    /// Implementations that are only interested in events can ignore this
    /// by keeping the default no-op implementation.
    #[inline]
    fn push_metrics<const MAX_NODES: usize, const MAX_EDGES: usize>(
        &mut self,
        _graph: &GraphMetrics<MAX_NODES, MAX_EDGES>,
    ) -> Result<(), TelemetrySinkError> {
        Ok(())
    }

    /// Flush any buffered events to the underlying sink.
    ///
    /// The default implementation is a no operation; implementations that
    /// buffer data should override this.
    #[inline]
    fn flush(&mut self) -> Result<(), TelemetrySinkError> {
        Ok(())
    }
}

/// Convert a watermark state into a stable string representation.
///
/// This helper is used by line based writers to render human readable
/// representations of watermark transitions.
#[inline]
fn wm_str(wm: WatermarkState) -> &'static str {
    match wm {
        WatermarkState::BelowSoft => "BelowSoft",
        WatermarkState::BetweenSoftAndHard => "BetweenSoftAndHard",
        WatermarkState::AtOrAboveHard => "AtOrAboveHard",
    }
}

/// Write an unsigned integer in base ten to a `fmt::Write` target.
///
/// This helper is a small, allocation free formatter used by line based
/// writers in both no_std and std configurations.
#[inline]
pub fn write_u64<W: fmt::Write>(writer: &mut W, mut value: u64) -> fmt::Result {
    if value == 0 {
        return writer.write_str("0");
    }

    let mut buffer = [0u8; 20];
    let mut write_index = buffer.len();

    while value != 0 {
        write_index -= 1;
        let digit = (value % 10) as u8;
        buffer[write_index] = b'0' + digit;
        value /= 10;
    }

    // This cannot fail because we only wrote ASCII digits.
    let string_slice = core::str::from_utf8(&buffer[write_index..]).unwrap();
    writer.write_str(string_slice)
}

/// Format a telemetry event as a single line of text.
///
/// The exact format is stable and intended for machine parsing as well as
/// human reading. It does not allocate and only uses `fmt::Write`.
pub fn fmt_event<W: fmt::Write>(w: &mut W, e: &TelemetryEvent) -> fmt::Result {
    match e {
        TelemetryEvent::Runtime(ev) => {
            w.write_str("runtime id=")?;
            write_u64(w, ev.graph_id() as u64)?;
            w.write_str(" ts=")?;
            write_u64(w, ev.timestamp_ns())?;
            w.write_str(" kind=")?;
            w.write_str(match ev.event_kind() {
                RuntimeTelemetryEventKind::GraphStarted => "GraphStarted",
                RuntimeTelemetryEventKind::GraphStopped => "GraphStopped",
                RuntimeTelemetryEventKind::GraphPanicked => "GraphPanicked",
                RuntimeTelemetryEventKind::SensorDisconnected => "SensorDisconnected",
                RuntimeTelemetryEventKind::SensorRecovered => "SensorRecovered",
                RuntimeTelemetryEventKind::ModelLoadFailed => "ModelLoadFailed",
                RuntimeTelemetryEventKind::ModelRecovered => "ModelRecovered",
                RuntimeTelemetryEventKind::MqttDisconnected => "MqttDisconnected",
                RuntimeTelemetryEventKind::MqttRecovered => "MqttRecovered",
                RuntimeTelemetryEventKind::DataGapDetected => "DataGapDetected",
                RuntimeTelemetryEventKind::InvalidDataSeen => "InvalidDataSeen",
            })?;
            w.write_str(" msg=")?;
            if let Some(msg) = ev.message() {
                w.write_str(msg.as_str())?;
            } else {
                w.write_str("-")?;
            }
            w.write_str("\n")
        }
        TelemetryEvent::NodeStep(ev) => {
            w.write_str("node-step gid=")?;
            write_u64(w, ev.graph_id() as u64)?;
            w.write_str(" nin=")?;
            write_u64(w, ev.node_index().as_usize() as u64)?;
            w.write_str(" ts_start=")?;
            write_u64(w, ev.timestamp_start_ns())?;
            w.write_str(" ts_end=")?;
            write_u64(w, ev.timestamp_end_ns())?;
            w.write_str(" dur=")?;
            write_u64(w, ev.duration_ns())?;
            w.write_str(" dl=")?;
            if let Some(d) = ev.deadline_ns() {
                write_u64(w, d)?;
            } else {
                w.write_str("-")?;
            }
            w.write_str(" miss=")?;
            w.write_str(if ev.deadline_missed() { "1" } else { "0" })?;
            w.write_str(" err=")?;
            if let Some(k) = ev.error_kind() {
                w.write_str(match k {
                    NodeStepError::NoInput => "NoInput",
                    NodeStepError::Backpressured => "BackPressured",
                    NodeStepError::OverBudget => "OverBudget",
                    NodeStepError::ExternalUnavailable => "ExternalUnavailable",
                    NodeStepError::ExecutionFailed => "ExecutionFailed",
                })?;
            } else {
                w.write_str("-")?;
            }
            w.write_str("\n")
        }
        TelemetryEvent::EdgeSnapshot(ev) => {
            w.write_str("edge-snap gid=")?;
            write_u64(w, ev.graph_id() as u64)?;
            w.write_str(" eid=")?;
            write_u64(w, ev.edge_index().as_usize() as u64)?;
            w.write_str(" ts=")?;
            write_u64(w, ev.timestamp_ns())?;
            w.write_str(" occ=")?;
            write_u64(w, ev.current_occupancy() as u64)?;
            w.write_str(" wm=")?;
            w.write_str(wm_str(ev.watermark_state()))?;
            w.write_str("\n")
        }
    }
}

// ---- no_std sink ----

/// Event writer that formats events as lines of text using `core::fmt::Write`.
///
/// This is a no_std friendly sink that can write to any type that implements
/// `fmt::Write`, such as a ring buffer or fixed size string.
pub struct FmtLineWriter<W: fmt::Write> {
    /// Inner writer that receives formatted event lines.
    inner: W,
}

impl<W: fmt::Write> FmtLineWriter<W> {
    /// Create a new line based event writer around the given writer.
    pub fn new(writer: W) -> Self {
        Self { inner: writer }
    }

    /// Access the inner `fmt::Write` target.
    #[inline]
    pub fn inner(&self) -> &W {
        &self.inner
    }
}

impl<W: fmt::Write> TelemetrySink for FmtLineWriter<W> {
    fn push_event(&mut self, e: &TelemetryEvent) -> Result<(), TelemetrySinkError> {
        fmt_event(&mut self.inner, e).map_err(|_| TelemetrySinkError::PushFailed)
    }

    fn push_metrics<const MAX_NODES: usize, const MAX_EDGES: usize>(
        &mut self,
        graph: &GraphMetrics<MAX_NODES, MAX_EDGES>,
    ) -> Result<(), TelemetrySinkError> {
        graph
            .fmt(&mut self.inner)
            .map_err(|_| TelemetrySinkError::PushFailed)
    }
}

impl<W: fmt::Write + Clone> Clone for FmtLineWriter<W> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Fixed-size, owned buffer implementing `core::fmt::Write`.
///
/// - Completely no_std and allocation-free.
/// - Capacity is a const generic `N`.
/// - On overflow, `write_str` returns `Err(fmt::Error)` (no partial writes).
#[derive(Clone, Copy)]
pub struct FixedBuffer<const N: usize> {
    buffer: [u8; N],
    length: usize,
}

impl<const N: usize> Default for FixedBuffer<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> FixedBuffer<N> {
    /// Create a new empty buffer.
    pub const fn new() -> Self {
        Self {
            buffer: [0u8; N],
            length: 0,
        }
    }

    /// Maximum capacity in bytes.
    #[inline]
    pub const fn capacity(&self) -> usize {
        N
    }

    /// Number of bytes currently written.
    #[inline]
    pub fn len(&self) -> usize {
        self.length
    }

    /// Returns true if the buffer is empty.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Access the written portion as bytes.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.buffer[..self.length]
    }

    /// Access the written portion as `&str`.
    ///
    /// Panics if the contents are not valid UTF-8, which is fine for telemetry
    /// because we only ever write ASCII.
    #[inline]
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(self.as_bytes()).unwrap()
    }

    /// Clear the buffer without reallocating.
    #[inline]
    pub fn clear(&mut self) {
        self.length = 0;
    }
}

impl<const N: usize> fmt::Write for FixedBuffer<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        if self.length + bytes.len() > N {
            return Err(fmt::Error);
        }
        let start = self.length;
        let end = start + bytes.len();
        self.buffer[start..end].copy_from_slice(bytes);
        self.length = end;
        Ok(())
    }
}

/// Convenience constructor for a line writer over a fixed owned buffer.
///
/// This is pure no_std: no heap, no std::io.
pub fn fixed_buffer_line_writer<const N: usize>() -> FmtLineWriter<FixedBuffer<N>> {
    FmtLineWriter::new(FixedBuffer::<N>::new())
}

// ---- std sink ----

#[cfg(feature = "std")]
struct BufWriter<'a> {
    data: &'a mut [u8],
    len: usize,
}

#[cfg(feature = "std")]
impl<'a> BufWriter<'a> {
    fn new(storage: &'a mut [u8]) -> Self {
        Self {
            data: storage,
            len: 0,
        }
    }

    fn as_slice(&self) -> &[u8] {
        &self.data[..self.len]
    }
}

#[cfg(feature = "std")]
impl<'a> fmt::Write for BufWriter<'a> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        if self.len + bytes.len() > self.data.len() {
            return Err(fmt::Error);
        }
        self.data[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
        Ok(())
    }
}

/// Event writer that formats events as lines and writes them to a `std::io::Write`
/// target.
///
/// This is a std only sink suitable for writing to files, sockets, or standard
/// output without any heap allocation in the formatting path.
#[cfg(feature = "std")]
pub struct IoLineWriter<W: std::io::Write> {
    /// Inner writer that receives encoded event bytes.
    inner: W,
}

#[cfg(feature = "std")]
impl<W: std::io::Write> IoLineWriter<W> {
    /// Create a new input output backed event writer around the given writer.
    pub fn new(writer: W) -> Self {
        Self { inner: writer }
    }

    /// Create an `IoLineWriter` that writes telemetry lines to `stdout`.
    pub fn stdout_writer() -> IoLineWriter<std::io::Stdout> {
        IoLineWriter::new(std::io::stdout())
    }

    /// Create an `IoLineWriter` that writes telemetry lines to a file at `path`.
    pub fn file_writer(path: &str) -> std::io::Result<IoLineWriter<std::fs::File>> {
        let file = std::fs::File::create(path)?;
        Ok(IoLineWriter::new(file))
    }
}

#[cfg(feature = "std")]
impl<W: std::io::Write> TelemetrySink for IoLineWriter<W> {
    fn push_event(&mut self, event: &TelemetryEvent) -> Result<(), TelemetrySinkError> {
        let mut buffer = [0u8; 256];
        let mut writer = BufWriter::new(&mut buffer);

        if fmt_event(&mut writer, event).is_err() {
            let mut heap_buffer = String::new();
            fmt_event(&mut heap_buffer, event).map_err(|_| TelemetrySinkError::PushFailed)?;
            self.inner
                .write_all(heap_buffer.as_bytes())
                .map_err(|_| TelemetrySinkError::PushFailed)
        } else {
            self.inner
                .write_all(writer.as_slice())
                .map_err(|_| TelemetrySinkError::PushFailed)
        }
    }

    fn push_metrics<const MAX_NODES: usize, const MAX_EDGES: usize>(
        &mut self,
        graph: &GraphMetrics<MAX_NODES, MAX_EDGES>,
    ) -> Result<(), TelemetrySinkError> {
        let mut buffer = [0u8; 4096];
        let mut writer = BufWriter::new(&mut buffer);

        if graph.fmt(&mut writer).is_err() {
            let mut heap_buffer = String::new();
            graph
                .fmt(&mut heap_buffer)
                .map_err(|_| TelemetrySinkError::PushFailed)?;
            self.inner
                .write_all(heap_buffer.as_bytes())
                .map_err(|_| TelemetrySinkError::PushFailed)
        } else {
            self.inner
                .write_all(writer.as_slice())
                .map_err(|_| TelemetrySinkError::PushFailed)
        }
    }

    fn flush(&mut self) -> Result<(), TelemetrySinkError> {
        self.inner
            .flush()
            .map_err(|_| TelemetrySinkError::PushFailed)
    }
}

#[cfg(feature = "std")]
impl<W: std::io::Write + Clone> Clone for IoLineWriter<W> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl TelemetrySink for () {}
