#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(all(feature = "std", feature = "stdout"))]
pub mod stdout_sink;

#[cfg(all(feature = "std", feature = "file"))]
pub mod file_sink;

#[cfg(all(feature = "std", feature = "mqtt-paho"))]
pub mod mqtt_paho_sink;

#[cfg(all(feature = "std", feature = "gpio-rpi"))]
pub mod gpio_rpi_sink;

#[cfg(all(feature = "alloc", feature = "register"))]
pub fn register_all(registries: &mut limen::Registries) -> Result<(), limen::registry::RegistryError> {
    use alloc::boxed::Box;

    #[cfg(all(feature = "std", feature = "stdout"))]
    { registries.register_output_sink_factory_with_name("stdout".to_string(), Box::new(stdout_sink::StandardOutputSinkFactory))?; }
    #[cfg(all(feature = "std", feature = "file"))]
    { registries.register_output_sink_factory_with_name("file".to_string(), Box::new(file_sink::FileOutputSinkFactory))?; }
    #[cfg(all(feature = "std", feature = "mqtt-paho"))]
    { registries.register_output_sink_factory_with_name("mqtt".to_string(), Box::new(mqtt_paho_sink::MessageQueuingTelemetryTransportSinkFactory))?; }
    #[cfg(all(feature = "std", feature = "gpio-rpi"))]
    { registries.register_output_sink_factory_with_name("gpio".to_string(), Box::new(gpio_rpi_sink::GeneralPurposeInputOutputSinkFactory))?; }
    Ok(())
}
