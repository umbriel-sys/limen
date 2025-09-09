#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
pub mod simulated;

#[cfg(all(feature = "csv", feature = "alloc", feature = "std"))]
pub mod csv;

#[cfg(all(feature = "serialport", feature = "alloc", feature = "std"))]
pub mod serial_port;

#[cfg(all(feature = "mqtt-paho", feature = "alloc", feature = "std"))]
pub mod mqtt_paho_sensor;

#[cfg(all(feature = "alloc", feature = "register"))]
pub fn register_all(registries: &mut limen::Registries) -> Result<(), limen::registry::RegistryError> {
    use alloc::boxed::Box;
    use simulated::SimulatedSensorFactory;

    registries.register_sensor_stream_factory_with_name("simulated".to_string(), Box::new(SimulatedSensorFactory))?;

    #[cfg(all(feature = "csv", feature = "std"))]
    {
        use crate::csv::CsvSensorFactory;
        registries.register_sensor_stream_factory_with_name("csv".to_string(), Box::new(CsvSensorFactory))?;
    }

    #[cfg(all(feature = "serialport", feature = "std"))]
    {
        use crate::serial_port::SerialPortSensorFactory;
        registries.register_sensor_stream_factory_with_name("serial".to_string(), Box::new(SerialPortSensorFactory))?;
    }

    #[cfg(all(feature = "mqtt-paho", feature = "std"))]
    {
        use crate::mqtt_paho_sensor::MessageQueuingTelemetryTransportSensorFactory;
        registries.register_sensor_stream_factory_with_name("mqtt".to_string(), Box::new(MessageQueuingTelemetryTransportSensorFactory))?;
    }

    Ok(())
}
