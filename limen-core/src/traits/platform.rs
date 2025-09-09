#![cfg_attr(not(feature = "std"), no_std)]

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, Default)]
pub struct PlatformConstraints {
    pub maximum_concurrent_model_tasks: u16,
    pub has_general_purpose_input_output_pins: bool,
    pub has_graphics_processing_unit: bool,
    pub recommended_worker_thread_count: u16,
}
