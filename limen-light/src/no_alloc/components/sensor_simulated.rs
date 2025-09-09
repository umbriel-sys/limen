#![cfg_attr(not(feature = "std"), no_std)]
use limen_core::errors::SensorError;
use limen_core::types::{SequenceNumber, TimestampNanosecondsSinceUnixEpoch};
use crate::no_alloc::traits::SensorSourceBorrowed;
use crate::no_alloc::types::SensorFrameMeta;

pub enum SimulatedPattern { Zeros, Ramp, Constant(u8) }

pub struct SimulatedSensorNoAlloc {
    is_open: bool,
    frame_length_bytes: usize,
    total_frames: u64,
    emitted: u64,
    pattern: SimulatedPattern,
    timestamp_next: TimestampNanosecondsSinceUnixEpoch,
    timestamp_step_nanoseconds: u128,
    sequence_next: SequenceNumber,
}
impl SimulatedSensorNoAlloc {
    pub fn new(frame_length_bytes: usize, total_frames: u64, pattern: SimulatedPattern) -> Self {
        Self {
            is_open: false, frame_length_bytes, total_frames, emitted: 0, pattern,
            timestamp_next: TimestampNanosecondsSinceUnixEpoch(0), timestamp_step_nanoseconds: 1_000_000, sequence_next: SequenceNumber(0),
        }
    }
}
impl SensorSourceBorrowed for SimulatedSensorNoAlloc {
    fn open(&mut self) -> Result<(), SensorError> { self.is_open = true; Ok(()) }
    fn read_next_into<'a>(&'a mut self, destination_buffer: &'a mut [u8]) -> Result<Option<(usize, SensorFrameMeta)>, SensorError> {
        if !self.is_open { return Err(SensorError::OpenFailed); }
        if self.emitted >= self.total_frames { return Err(SensorError::EndOfStream); }
        if destination_buffer.len() < self.frame_length_bytes { return Ok(None); }
        match self.pattern {
            SimulatedPattern::Zeros => { for b in &mut destination_buffer[..self.frame_length_bytes] { *b = 0; } }
            SimulatedPattern::Ramp => { for (i, b) in destination_buffer[..self.frame_length_bytes].iter_mut().enumerate() { *b = (i % 256) as u8; } }
            SimulatedPattern::Constant(v) => { for b in &mut destination_buffer[..self.frame_length_bytes] { *b = v; } }
        }
        let meta = SensorFrameMeta { timestamp: self.timestamp_next, sequence_number: self.sequence_next };
        self.emitted += 1;
        self.sequence_next = SequenceNumber(self.sequence_next.0.saturating_add(1));
        self.timestamp_next = TimestampNanosecondsSinceUnixEpoch(self.timestamp_next.0.saturating_add(self.timestamp_step_nanoseconds));
        Ok(Some((self.frame_length_bytes, meta)))
    }
    fn close(&mut self) -> Result<(), SensorError> { self.is_open = false; Ok(()) }
}
