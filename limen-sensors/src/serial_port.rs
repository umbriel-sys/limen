use limen_core::errors::SensorError;
use limen_core::traits::configuration::SensorStreamConfiguration;
use limen_core::traits::{SensorStream, SensorStreamFactory};
use limen_core::types::{
    SensorData, SensorSampleMetadata, SequenceNumber, TimestampNanosecondsSinceUnixEpoch,
};

use std::io::{ErrorKind, Read};
use std::time::Duration;

use serialport::SerialPort;

#[derive(Debug)]
pub struct SerialPortSensor {
    is_open: bool,
    port: Box<dyn SerialPort>,
    frame_length_bytes: usize,
    buffer: Vec<u8>,

    timestamp_next: TimestampNanosecondsSinceUnixEpoch,
    timestamp_step_nanoseconds: u128,
    sequence_next: SequenceNumber,

    sample_rate_hz: Option<u32>,
    channel_count: Option<u16>,
}

impl SerialPortSensor {
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

impl SensorStream for SerialPortSensor {
    fn open(&mut self) -> Result<(), SensorError> {
        self.is_open = true;
        Ok(())
    }

    fn read_next<'a>(&'a mut self) -> Result<Option<SensorData<'a>>, SensorError> {
        if !self.is_open {
            return Err(SensorError::OpenFailed);
        }
        let mut temporary = [0u8; 1024];
        match self.port.read(&mut temporary) {
            Ok(count) => {
                if count > 0 {
                    self.buffer.extend_from_slice(&temporary[..count]);
                }
            }
            Err(e) if e.kind() == ErrorKind::TimedOut || e.kind() == ErrorKind::WouldBlock => {}
            Err(_) => return Err(SensorError::ReadFailed),
        }
        if self.buffer.len() < self.frame_length_bytes {
            return Ok(None);
        }
        let frame = self
            .buffer
            .drain(..self.frame_length_bytes)
            .collect::<Vec<u8>>();
        let data = SensorData {
            timestamp: self.timestamp_next,
            sequence_number: self.sequence_next,
            payload: alloc::borrow::Cow::Owned(frame),
            metadata: self.metadata(),
        };
        self.sequence_next = SequenceNumber(self.sequence_next.0.saturating_add(1));
        self.timestamp_next = TimestampNanosecondsSinceUnixEpoch(
            self.timestamp_next
                .0
                .saturating_add(self.timestamp_step_nanoseconds),
        );
        Ok(Some(data))
    }

    fn reset(&mut self) -> Result<(), SensorError> {
        self.buffer.clear();
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

pub struct SerialPortSensorFactory;

impl SensorStreamFactory for SerialPortSensorFactory {
    fn sensor_name(&self) -> &'static str {
        "serial"
    }

    fn create_sensor_stream(
        &self,
        configuration: &SensorStreamConfiguration,
    ) -> Result<Box<dyn SensorStream>, SensorError> {
        let port_name = configuration
            .parameters
            .get("port_name")
            .ok_or(SensorError::ConfigurationInvalid)?
            .to_string();
        let baud_rate: u32 = configuration
            .parameters
            .get("baud_rate")
            .and_then(|s| s.parse::<u32>().ok())
            .ok_or(SensorError::ConfigurationInvalid)?;
        let frame_length_bytes: usize = configuration
            .parameters
            .get("frame_length_bytes")
            .and_then(|s| s.parse::<usize>().ok())
            .ok_or(SensorError::ConfigurationInvalid)?;

        let timestamp_step_nanoseconds: u128 = configuration
            .parameters
            .get("timestamp_step_nanoseconds")
            .and_then(|s| s.parse::<u128>().ok())
            .unwrap_or(1_000_000);
        let sequence_start: u64 = configuration
            .parameters
            .get("sequence_start")
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        let sample_rate_hz: Option<u32> = configuration
            .parameters
            .get("sample_rate_hz")
            .and_then(|s| s.parse::<u32>().ok());
        let channel_count: Option<u16> = configuration
            .parameters
            .get("channel_count")
            .and_then(|s| s.parse::<u16>().ok());

        let port = serialport::new(port_name, baud_rate)
            .timeout(Duration::from_millis(0))
            .open()
            .map_err(|_| SensorError::OpenFailed)?;

        Ok(Box::new(SerialPortSensor {
            is_open: false,
            port,
            frame_length_bytes,
            buffer: Vec::with_capacity(frame_length_bytes.saturating_mul(2)),
            timestamp_next: TimestampNanosecondsSinceUnixEpoch(0),
            timestamp_step_nanoseconds,
            sequence_next: SequenceNumber(sequence_start),
            sample_rate_hz,
            channel_count,
        }))
    }
}
