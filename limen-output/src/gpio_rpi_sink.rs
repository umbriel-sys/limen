use limen_core::errors::OutputError;
use limen_core::traits::{OutputSink, OutputSinkFactory};
use limen_core::traits::configuration::OutputSinkConfiguration;
use limen_core::types::TensorOutput;

use rppal::gpio::{Gpio, OutputPin};

pub struct GeneralPurposeInputOutputSink { pin: OutputPin, active_high: bool }

impl OutputSink for GeneralPurposeInputOutputSink {
    fn write(&mut self, output: &TensorOutput) -> Result<(), OutputError> {
        let is_active = output.buffer.first().copied().unwrap_or(0u8) != 0;
        if self.active_high { if is_active { self.pin.set_high(); } else { self.pin.set_low(); } }
        else { if is_active { self.pin.set_low(); } else { self.pin.set_high(); } }
        Ok(())
    }
}

pub struct GeneralPurposeInputOutputSinkFactory;
impl OutputSinkFactory for GeneralPurposeInputOutputSinkFactory {
    fn sink_name(&self) -> &'static str { "gpio" }
    fn create_output_sink(&self, configuration: &OutputSinkConfiguration) -> Result<Box<dyn OutputSink>, OutputError> {
        let pin_number: u8 = configuration.parameters.get("pin").and_then(|s| s.parse::<u16>().ok()).and_then(|v| u8::try_from(v).ok())
            .ok_or(OutputError::Other { message: "missing or invalid 'pin'".to_string() })?;
        let active_high: bool = configuration.parameters.get("active_high").map(|s| s == "1" || s.eq_ignore_ascii_case("true")).unwrap_or(true);
        let gpio = Gpio::new().map_err(|_| OutputError::Other { message: "failed to access GPIO".to_string() })?;
        let pin = gpio.get(pin_number as u8).map_err(|_| OutputError::Other { message: "invalid GPIO pin".to_string() })?.into_output();
        Ok(Box::new(GeneralPurposeInputOutputSink { pin, active_high }))
    }
}
