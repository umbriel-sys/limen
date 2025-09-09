use limen_core::errors::SensorError;
use limen_core::traits::configuration::SensorStreamConfiguration;
use limen_core::traits::{SensorStream, SensorStreamFactory};
use limen_core::types::{
    SensorData, SensorSampleMetadata, SequenceNumber, TimestampNanosecondsSinceUnixEpoch,
};

use paho_mqtt as mqtt;
use std::time::Duration;

pub struct MessageQueuingTelemetryTransportSensor {
    is_open: bool,
    client: mqtt::Client,
    consumer: mqtt::Receiver<Option<mqtt::Message>>,
    topic: String,
    quality_of_service: i32,

    timestamp_next: TimestampNanosecondsSinceUnixEpoch,
    timestamp_step_nanoseconds: u128,
    sequence_next: SequenceNumber,

    sample_rate_hz: Option<u32>,
    channel_count: Option<u16>,
}

impl MessageQueuingTelemetryTransportSensor {
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

impl SensorStream for MessageQueuingTelemetryTransportSensor {
    fn open(&mut self) -> Result<(), SensorError> {
        self.is_open = true;
        Ok(())
    }

    fn read_next<'a>(&'a mut self) -> Result<Option<SensorData<'a>>, SensorError> {
        if !self.is_open {
            return Err(SensorError::OpenFailed);
        }
        match self.consumer.try_recv() {
            Ok(Some(message)) => {
                let payload_slice = message.payload();
                let owned = payload_slice.to_vec();
                let data = SensorData {
                    timestamp: self.timestamp_next,
                    sequence_number: self.sequence_next,
                    payload: alloc::borrow::Cow::Owned(owned),
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
            Ok(None) => Ok(None),
            Err(_e) => Ok(None),
        }
    }

    fn reset(&mut self) -> Result<(), SensorError> {
        Ok(())
    }
    fn close(&mut self) -> Result<(), SensorError> {
        self.is_open = false;
        let _ = self.client.unsubscribe(&self.topic);
        let _ = self.client.disconnect(None);
        Ok(())
    }
    fn describe(&self) -> Option<SensorSampleMetadata> {
        self.metadata()
    }
}

pub struct MessageQueuingTelemetryTransportSensorFactory;

impl SensorStreamFactory for MessageQueuingTelemetryTransportSensorFactory {
    fn sensor_name(&self) -> &'static str {
        "mqtt"
    }

    fn create_sensor_stream(
        &self,
        configuration: &SensorStreamConfiguration,
    ) -> Result<Box<dyn SensorStream>, SensorError> {
        let server_uri = configuration
            .parameters
            .get("server_uri")
            .ok_or(SensorError::ConfigurationInvalid)?
            .to_string();
        let client_id = configuration
            .parameters
            .get("client_id")
            .ok_or(SensorError::ConfigurationInvalid)?
            .to_string();
        let topic = configuration
            .parameters
            .get("topic")
            .ok_or(SensorError::ConfigurationInvalid)?
            .to_string();
        let quality_of_service: i32 = configuration
            .parameters
            .get("qos")
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(0);

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

        let create_options = mqtt::CreateOptionsBuilder::new()
            .server_uri(server_uri)
            .client_id(client_id)
            .finalize();
        let client = mqtt::Client::new(create_options).map_err(|_| SensorError::OpenFailed)?;
        let consumer = client.start_consuming();
        let connect_options = mqtt::ConnectOptionsBuilder::new()
            .keep_alive_interval(Duration::from_secs(15))
            .clean_session(true)
            .finalize();
        client
            .connect(connect_options)
            .map_err(|_| SensorError::OpenFailed)?;
        client
            .subscribe(&topic, quality_of_service)
            .map_err(|_| SensorError::OpenFailed)?;

        Ok(Box::new(MessageQueuingTelemetryTransportSensor {
            is_open: false,
            client,
            consumer,
            topic,
            quality_of_service,
            timestamp_next: TimestampNanosecondsSinceUnixEpoch(0),
            timestamp_step_nanoseconds,
            sequence_next: SequenceNumber(sequence_start),
            sample_rate_hz,
            channel_count,
        }))
    }
}
