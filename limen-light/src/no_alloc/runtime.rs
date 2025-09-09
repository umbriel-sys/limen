#![cfg_attr(not(feature = "std"), no_std)]
use crate::no_alloc::traits::{
    ModelNoAlloc, OutputSinkNoAlloc, PostprocessorNoAlloc, PreprocessorNoAlloc,
    SensorSourceBorrowed,
};
use crate::no_alloc::types::{BorrowedTensorView, BorrowedTensorViewMut, FixedShape};
use limen_core::errors::{InferenceError, OutputError, ProcessingError, RuntimeError, SensorError};
use limen_core::runtime::{FlowStatistics, RuntimeHealth, RuntimeState};
use limen_core::types::DataType;

pub struct NoAllocRuntime<
    S,
    P,
    M,
    Q,
    O,
    const SENSOR_MAX_BYTES: usize,
    const PREPROCESSOR_MAX_BYTES: usize,
    const MODEL_MAX_BYTES: usize,
    const POSTPROCESSOR_MAX_BYTES: usize,
    const MAX_DIMS: usize,
> where
    S: SensorSourceBorrowed,
    P: PreprocessorNoAlloc<MAX_DIMS>,
    M: ModelNoAlloc<MAX_DIMS>,
    Q: PostprocessorNoAlloc<MAX_DIMS>,
    O: OutputSinkNoAlloc,
{
    sensor: S,
    pre: P,
    model: M,
    post: Q,
    sink: O,

    state: RuntimeState,
    stats: FlowStatistics,

    sensor_bytes: [u8; SENSOR_MAX_BYTES],
    pre_bytes: [u8; PREPROCESSOR_MAX_BYTES],
    model_bytes: [u8; MODEL_MAX_BYTES],
    post_bytes: [u8; POSTPROCESSOR_MAX_BYTES],

    pre_shape: FixedShape<MAX_DIMS>,
    model_shape: FixedShape<MAX_DIMS>,
    post_shape: FixedShape<MAX_DIMS>,

    pre_data_type: DataType,
    model_data_type: DataType,
    post_data_type: DataType,
}

impl<
        S,
        P,
        M,
        Q,
        O,
        const SENSOR_MAX_BYTES: usize,
        const PREPROCESSOR_MAX_BYTES: usize,
        const MODEL_MAX_BYTES: usize,
        const POSTPROCESSOR_MAX_BYTES: usize,
        const MAX_DIMS: usize,
    >
    NoAllocRuntime<
        S,
        P,
        M,
        Q,
        O,
        SENSOR_MAX_BYTES,
        PREPROCESSOR_MAX_BYTES,
        MODEL_MAX_BYTES,
        POSTPROCESSOR_MAX_BYTES,
        MAX_DIMS,
    >
where
    S: SensorSourceBorrowed,
    P: PreprocessorNoAlloc<MAX_DIMS>,
    M: ModelNoAlloc<MAX_DIMS>,
    Q: PostprocessorNoAlloc<MAX_DIMS>,
    O: OutputSinkNoAlloc,
{
    pub fn new(sensor: S, pre: P, model: M, post: Q, sink: O) -> Self {
        Self {
            sensor,
            pre,
            model,
            post,
            sink,
            state: RuntimeState::Initialized,
            stats: FlowStatistics::default(),
            sensor_bytes: [0u8; SENSOR_MAX_BYTES],
            pre_bytes: [0u8; PREPROCESSOR_MAX_BYTES],
            model_bytes: [0u8; MODEL_MAX_BYTES],
            post_bytes: [0u8; POSTPROCESSOR_MAX_BYTES],
            pre_shape: FixedShape::new(),
            model_shape: FixedShape::new(),
            post_shape: FixedShape::new(),
            pre_data_type: DataType::Unsigned8,
            model_data_type: DataType::Unsigned8,
            post_data_type: DataType::Unsigned8,
        }
    }
    pub fn open(&mut self) -> Result<(), RuntimeError> {
        if self.state != RuntimeState::Initialized {
            return Err(RuntimeError::RuntimeNotOpen);
        }
        self.sensor
            .open()
            .map_err(|_| RuntimeError::ComponentFailureDuringOpen)?;
        self.state = RuntimeState::Running;
        Ok(())
    }
    pub fn request_stop(&mut self) {
        if self.state == RuntimeState::Running {
            self.state = RuntimeState::Stopping;
        }
    }
    pub fn close(&mut self) -> Result<(), RuntimeError> {
        self.sensor
            .close()
            .map_err(|_| RuntimeError::ComponentFailureDuringClose)?;
        self.state = RuntimeState::Stopped;
        Ok(())
    }
    pub fn run_step(&mut self) -> Result<bool, RuntimeError> {
        if self.state != RuntimeState::Running && self.state != RuntimeState::Stopping {
            return Err(RuntimeError::RuntimeNotOpen);
        }
        let mut did_work = false;

        let sensor_len_opt = match self.sensor.read_next_into(&mut self.sensor_bytes) {
            Ok(Some((n, _meta))) => {
                self.stats.total_sensor_samples_received += 1;
                Some(n)
            }
            Ok(None) => None,
            Err(SensorError::EndOfStream) => {
                self.request_stop();
                None
            }
            Err(_) => {
                self.request_stop();
                None
            }
        };

        if let Some(sensor_len) = sensor_len_opt {
            self.pre_shape.clear();
            let pre_len = match self.pre.process(
                &self.sensor_bytes[..sensor_len],
                &mut self.pre_data_type,
                &mut self.pre_shape,
                &mut self.pre_bytes,
            ) {
                Ok(n) => {
                    self.stats.total_preprocessor_outputs_produced += 1;
                    n
                }
                Err(_) => {
                    return Ok(true);
                }
            };

            self.model_shape.clear();
            let model_len = match self.model.infer(
                BorrowedTensorView {
                    data_type: self.pre_data_type,
                    shape: self.pre_shape.as_slice(),
                    buffer: &self.pre_bytes[..pre_len],
                },
                &mut self.model_data_type,
                &mut self.model_shape,
                &mut self.model_bytes,
            ) {
                Ok(n) => {
                    self.stats.total_model_inferences_completed += 1;
                    n
                }
                Err(_) => {
                    return Ok(true);
                }
            };

            self.post_shape.clear();
            let post_len = match self.post.process(
                BorrowedTensorView {
                    data_type: self.model_data_type,
                    shape: self.model_shape.as_slice(),
                    buffer: &self.model_bytes[..model_len],
                },
                &mut self.post_data_type,
                &mut self.post_shape,
                &mut self.post_bytes,
            ) {
                Ok(n) => {
                    self.stats.total_postprocessor_outputs_produced += 1;
                    n
                }
                Err(_) => {
                    return Ok(true);
                }
            };

            let _ = self
                .sink
                .write(BorrowedTensorView {
                    data_type: self.post_data_type,
                    shape: self.post_shape.as_slice(),
                    buffer: &self.post_bytes[..post_len],
                })
                .map_err(|_e| OutputError::WriteFailed);
            self.stats.total_outputs_written += 1;
            did_work = true;
        }

        if self.state == RuntimeState::Stopping {
            self.close()?;
        }
        Ok(did_work)
    }
    pub fn health(&self) -> RuntimeHealth {
        RuntimeHealth {
            state: self.state,
            statistics: self.stats.clone(),
            preprocessor_input_queue_length: 0,
            preprocessor_input_queue_capacity: 0,
            postprocessor_output_queue_length: 0,
            postprocessor_output_queue_capacity: 0,
        }
    }
    pub fn statistics(&self) -> &FlowStatistics {
        &self.stats
    }
    pub fn state(&self) -> RuntimeState {
        self.state
    }
}
