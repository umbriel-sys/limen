use limen_core::errors::SensorError;
use limen_core::traits::configuration::SensorStreamConfiguration;
use limen_core::traits::{SensorStream, SensorStreamFactory};
use limen_core::types::{
    SensorData, SensorSampleMetadata, SequenceNumber, TimestampNanosecondsSinceUnixEpoch,
};

#[cfg(feature = "alloc")]
use alloc::{boxed::Box, string::String, vec, vec::Vec};

#[cfg(feature = "alloc")]
use alloc::borrow::Cow;

#[derive(Clone, Debug)]
pub enum SimulatedPattern {
    Zeros,
    Ramp,
    Constant(u8),
}

#[derive(Clone, Debug)]
pub struct SimulatedSensor {
    is_open: bool,
    frame_length_bytes: usize,
    total_frames_to_emit: u64,
    frames_emitted: u64,
    pattern: SimulatedPattern,
    timestamp_next: TimestampNanosecondsSinceUnixEpoch,
    timestamp_step_nanoseconds: u128,
    sequence_next: SequenceNumber,
    sample_rate_hz: Option<u32>,
    channel_count: Option<u16>,
}

impl SimulatedSensor {
    fn build_payload(&self) -> Vec<u8> {
        match self.pattern {
            SimulatedPattern::Zeros => vec![0u8; self.frame_length_bytes],
            SimulatedPattern::Ramp => {
                let mut buffer = Vec::with_capacity(self.frame_length_bytes);
                for i in 0..self.frame_length_bytes {
                    buffer.push((i % 256) as u8);
                }
                buffer
            }
            SimulatedPattern::Constant(value) => vec![value; self.frame_length_bytes],
        }
    }
    fn metadata(&self) -> Option<SensorSampleMetadata> {
        if self.sample_rate_hz.is_none() && self.channel_count.is_none() {
            return None;
        }
        Some(SensorSampleMetadata {
            channel_count: self.channel_count,
            sample_rate_hz: self.sample_rate_hz,
            #[cfg(feature = "alloc")]
            notes: None,
        })
    }
}

#[cfg(feature = "alloc")]
impl SensorStream for SimulatedSensor {
    fn open(&mut self) -> Result<(), SensorError> {
        self.is_open = true;
        Ok(())
    }
    fn read_next<'a>(&'a mut self) -> Result<Option<SensorData<'a>>, SensorError> {
        if !self.is_open {
            return Err(SensorError::OpenFailed);
        }
        if self.frames_emitted >= self.total_frames_to_emit {
            return Err(SensorError::EndOfStream);
        }
        let payload_owned = self.build_payload();
        let data = SensorData {
            timestamp: self.timestamp_next,
            sequence_number: self.sequence_next,
            payload: alloc::borrow::Cow::Owned(payload_owned),
            metadata: self.metadata(),
        };
        self.frames_emitted = self.frames_emitted.saturating_add(1);
        self.sequence_next = SequenceNumber(self.sequence_next.0.saturating_add(1));
        self.timestamp_next = TimestampNanosecondsSinceUnixEpoch(
            self.timestamp_next
                .0
                .saturating_add(self.timestamp_step_nanoseconds),
        );
        Ok(Some(data))
    }
    fn reset(&mut self) -> Result<(), SensorError> {
        self.frames_emitted = 0;
        Ok(())
    }
    fn close(&mut self) -> Result<(), SensorError> {
        self.is_open = false;
        Ok(())
    }
    fn describe(&self) -> Option<SensorSampleMetadata> {
        self.metadata()
    }
}

pub struct SimulatedSensorFactory;
#[cfg(feature = "alloc")]
impl SensorStreamFactory for SimulatedSensorFactory {
    fn sensor_name(&self) -> &'static str {
        "simulated"
    }
    fn create_sensor_stream(
        &self,
        configuration: &SensorStreamConfiguration,
    ) -> Result<Box<dyn SensorStream>, SensorError> {
        let mut frame_length_bytes: Option<usize> = None;
        let mut total_frames_to_emit: u64 = 100;
        let mut pattern: SimulatedPattern = SimulatedPattern::Zeros;
        let mut timestamp_step_nanoseconds: u128 = 1_000_000;
        let mut sequence_start: u64 = 0;
        let mut sample_rate_hz: Option<u32> = None;
        let mut channel_count: Option<u16> = None;

        if let Some(s) = configuration.parameters.get("frame_length_bytes") {
            frame_length_bytes = Some(
                s.parse::<usize>()
                    .map_err(|_| SensorError::ConfigurationInvalid)?,
            );
        }
        if let Some(s) = configuration.parameters.get("total_frames") {
            total_frames_to_emit = s
                .parse::<u64>()
                .map_err(|_| SensorError::ConfigurationInvalid)?;
        }
        if let Some(s) = configuration.parameters.get("pattern") {
            let lower = s.to_ascii_lowercase();
            match lower.as_str() {
                "zeros" => pattern = SimulatedPattern::Zeros,
                "ramp" => pattern = SimulatedPattern::Ramp,
                "constant" => {
                    let constant_value = configuration
                        .parameters
                        .get("constant_value")
                        .and_then(|v| v.parse::<u8>().ok())
                        .unwrap_or(0u8);
                    pattern = SimulatedPattern::Constant(constant_value);
                }
                _ => return Err(SensorError::ConfigurationInvalid),
            }
        }
        if let Some(s) = configuration.parameters.get("timestamp_step_nanoseconds") {
            timestamp_step_nanoseconds = s
                .parse::<u128>()
                .map_err(|_| SensorError::ConfigurationInvalid)?;
        }
        if let Some(s) = configuration.parameters.get("sequence_start") {
            sequence_start = s
                .parse::<u64>()
                .map_err(|_| SensorError::ConfigurationInvalid)?;
        }
        if let Some(s) = configuration.parameters.get("sample_rate_hz") {
            sample_rate_hz = Some(
                s.parse::<u32>()
                    .map_err(|_| SensorError::ConfigurationInvalid)?,
            );
        }
        if let Some(s) = configuration.parameters.get("channel_count") {
            channel_count = Some(
                s.parse::<u16>()
                    .map_err(|_| SensorError::ConfigurationInvalid)?,
            );
        }

        let frame_length_bytes = frame_length_bytes.ok_or(SensorError::ConfigurationInvalid)?;
        let instance = SimulatedSensor {
            is_open: false,
            frame_length_bytes,
            total_frames_to_emit,
            frames_emitted: 0,
            pattern,
            timestamp_next: TimestampNanosecondsSinceUnixEpoch(0),
            timestamp_step_nanoseconds,
            sequence_next: SequenceNumber(sequence_start),
            sample_rate_hz,
            channel_count,
        };
        Ok(Box::new(instance))
    }
}
