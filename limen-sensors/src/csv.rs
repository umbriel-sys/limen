use limen_core::errors::SensorError;
use limen_core::traits::{SensorStream, SensorStreamFactory};
use limen_core::traits::configuration::SensorStreamConfiguration;
use limen_core::types::{SensorData, SensorSampleMetadata, SequenceNumber, TimestampNanosecondsSinceUnixEpoch};

#[cfg(all(feature = "alloc", feature = "std"))]
use alloc::{boxed::Box, string::String, vec::Vec};

#[cfg(all(feature = "alloc", feature = "std"))]
use alloc::borrow::Cow;

#[cfg(feature = "std")]
use std::fs::File;
#[cfg(feature = "std")]
use std::io::{BufRead, BufReader};

#[derive(Debug)]
pub struct CsvSensor {
    is_open: bool,
    reader: BufReader<File>,
    skip_header: bool,
    header_skipped: bool,
    line_buffer: String,
    timestamp_next: TimestampNanosecondsSinceUnixEpoch,
    timestamp_step_nanoseconds: u128,
    sequence_next: SequenceNumber,
    sample_rate_hz: Option<u32>,
    channel_count: Option<u16>,
}

impl CsvSensor {
    #[cfg(feature = "std")]
    fn parse_line_to_bytes(line: &str) -> Result<Vec<u8>, SensorError> {
        let mut output = Vec::new();
        for part in line.split(',') {
            let token = part.trim();
            if token.is_empty() { output.push(0u8); continue; }
            let value: u16 = token.parse::<u16>().map_err(|_| SensorError::ReadFailed)?;
            if value > 255 { return Err(SensorError::ReadFailed); }
            output.push(value as u8);
        }
        Ok(output)
    }
    #[cfg(feature = "std")]
    fn metadata(&self) -> Option<SensorSampleMetadata> {
        if self.sample_rate_hz.is_none() && self.channel_count.is_none() { return None; }
        Some(SensorSampleMetadata { channel_count: self.channel_count, sample_rate_hz: self.sample_rate_hz, #[cfg(feature="alloc")] notes: None })
    }
}

pub struct CsvSensorFactory;

#[cfg(all(feature = "alloc", feature = "std"))]
impl SensorStreamFactory for CsvSensorFactory {
    fn sensor_name(&self) -> &'static str { "csv" }

    fn create_sensor_stream(&self, configuration: &SensorStreamConfiguration) -> Result<Box<dyn SensorStream>, SensorError> {
        let path = configuration.parameters.get("path").ok_or(SensorError::ConfigurationInvalid)?.to_string();
        let skip_header = configuration.parameters.get("skip_header").map(|s| s.eq_ignore_ascii_case("true") || s == "1").unwrap_or(false);
        let timestamp_step_nanoseconds: u128 = configuration.parameters.get("timestamp_step_nanoseconds").map(|s| s.parse::<u128>().ok()).flatten().unwrap_or(1_000_000);
        let sequence_start: u64 = configuration.parameters.get("sequence_start").map(|s| s.parse::<u64>().ok()).flatten().unwrap_or(0);
        let sample_rate_hz: Option<u32> = configuration.parameters.get("sample_rate_hz").map(|s| s.parse::<u32>().ok()).flatten();
        let channel_count: Option<u16> = configuration.parameters.get("channel_count").map(|s| s.parse::<u16>().ok()).flatten();

        let file = File::open(path).map_err(|_| SensorError::OpenFailed)?;
        let reader = BufReader::new(file);

        let instance = CsvSensor {
            is_open: false,
            reader,
            skip_header,
            header_skipped: false,
            line_buffer: String::new(),
            timestamp_next: TimestampNanosecondsSinceUnixEpoch(0),
            timestamp_step_nanoseconds,
            sequence_next: SequenceNumber(sequence_start),
            sample_rate_hz,
            channel_count,
        };
        Ok(Box::new(instance))
    }
}

#[cfg(all(feature = "alloc", feature = "std"))]
impl SensorStream for CsvSensor {
    fn open(&mut self) -> Result<(), SensorError> { self.is_open = true; Ok(()) }

    fn read_next<'a>(&'a mut self) -> Result<Option<SensorData<'a>>, SensorError> {
        if !self.is_open { return Err(SensorError::OpenFailed); }
        self.line_buffer.clear();
        let bytes_read = self.reader.read_line(&mut self.line_buffer).map_err(|_| SensorError::ReadFailed)?;
        if bytes_read == 0 { return Err(SensorError::EndOfStream); }
        if self.skip_header && !self.header_skipped {
            self.header_skipped = true;
            self.line_buffer.clear();
            let bytes_read = self.reader.read_line(&mut self.line_buffer).map_err(|_| SensorError::ReadFailed)?;
            if bytes_read == 0 { return Err(SensorError::EndOfStream); }
        }
        let line_trimmed = self.line_buffer.trim();
        if line_trimmed.is_empty() { return Ok(None); }
        let payload_owned = CsvSensor::parse_line_to_bytes(line_trimmed)?;
        let data = SensorData {
            timestamp: self.timestamp_next,
            sequence_number: self.sequence_next,
            payload: alloc::borrow::Cow::Owned(payload_owned),
            metadata: self.metadata(),
        };
        self.sequence_next = SequenceNumber(self.sequence_next.0.saturating_add(1));
        self.timestamp_next = TimestampNanosecondsSinceUnixEpoch(self.timestamp_next.0.saturating_add(self.timestamp_step_nanoseconds));
        Ok(Some(data))
    }

    fn reset(&mut self) -> Result<(), SensorError> { Err(SensorError::ResetFailed) }
    fn close(&mut self) -> Result<(), SensorError> { self.is_open = false; Ok(()) }
    fn describe(&self) -> Option<SensorSampleMetadata> { self.metadata() }
}
