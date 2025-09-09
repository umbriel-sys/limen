use limen_core::errors::OutputError;
use limen_core::traits::{OutputSink, OutputSinkFactory};
use limen_core::traits::configuration::OutputSinkConfiguration;
use limen_core::types::TensorOutput;

use std::time::Duration;
use paho_mqtt as mqtt;

pub struct MessageQueuingTelemetryTransportSink {
    client: mqtt::Client,
    topic: String,
    quality_of_service: i32,
    retain: bool,
}

impl OutputSink for MessageQueuingTelemetryTransportSink {
    fn write(&mut self, output: &TensorOutput) -> Result<(), OutputError> {
        let message = mqtt::Message::new(&self.topic, output.buffer.clone(), self.quality_of_service);
        self.client.publish(message).map_err(|_| OutputError::WriteFailed)?;
        Ok(())
    }
    fn close(&mut self) -> Result<(), OutputError> {
        let _ = self.client.disconnect(None);
        Ok(())
    }
}

pub struct MessageQueuingTelemetryTransportSinkFactory;
impl OutputSinkFactory for MessageQueuingTelemetryTransportSinkFactory {
    fn sink_name(&self) -> &'static str { "mqtt" }
    fn create_output_sink(&self, configuration: &OutputSinkConfiguration) -> Result<Box<dyn OutputSink>, OutputError> {
        let server_uri = configuration.parameters.get("server_uri").ok_or(OutputError::Other { message: "missing 'server_uri'".to_string() })?.to_string();
        let client_id = configuration.parameters.get("client_id").ok_or(OutputError::Other { message: "missing 'client_id'".to_string() })?.to_string();
        let topic = configuration.parameters.get("topic").ok_or(OutputError::Other { message: "missing 'topic'".to_string() })?.to_string();
        let qos: i32 = configuration.parameters.get("qos").and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
        let retain: bool = configuration.parameters.get("retain").map(|s| s == "1" || s.eq_ignore_ascii_case("true")).unwrap_or(false);

        let create_options = mqtt::CreateOptionsBuilder::new().server_uri(server_uri).client_id(client_id).finalize();
        let client = mqtt::Client::new(create_options).map_err(|_| OutputError::Other { message: "failed to create MQTT client".to_string() })?;
        let connect_options = mqtt::ConnectOptionsBuilder::new().keep_alive_interval(Duration::from_secs(15)).clean_session(true).finalize();
        client.connect(connect_options).map_err(|_| OutputError::Other { message: "MQTT connection failed".to_string() })?;
        Ok(Box::new(MessageQueuingTelemetryTransportSink { client, topic, quality_of_service: qos, retain }))
    }
}
