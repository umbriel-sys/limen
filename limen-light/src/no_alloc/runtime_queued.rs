#![cfg_attr(not(feature = "std"), no_std)]
use heapless::Deque;
use limen_core::errors::{RuntimeError, SensorError, OutputError};
use limen_core::runtime::{FlowStatistics, RuntimeHealth, RuntimeState};
use limen_core::types::DataType;
use crate::no_alloc::traits::{SensorSourceBorrowed, PreprocessorNoAlloc, ModelNoAlloc, PostprocessorNoAlloc, OutputSinkNoAlloc};
use crate::no_alloc::types::{BorrowedTensorView, FixedShape, TensorFrame};

#[derive(Clone, Copy, Debug)]
pub enum BackpressurePolicyNoAlloc { Block, DropNewest, DropOldest }

pub struct NoAllocQueuedRuntime<
    S, P, M, Q, O,
    const SENSOR_MAX_BYTES: usize,
    const PREPROCESSOR_MAX_BYTES: usize,
    const MODEL_MAX_BYTES: usize,
    const POSTPROCESSOR_MAX_BYTES: usize,
    const MAX_DIMS: usize,
    const PREPROCESSOR_QUEUE_CAPACITY: usize,
    const POSTPROCESSOR_QUEUE_CAPACITY: usize,
>
where
    S: SensorSourceBorrowed,
    P: PreprocessorNoAlloc<MAX_DIMS>,
    M: ModelNoAlloc<MAX_DIMS>,
    Q: PostprocessorNoAlloc<MAX_DIMS>,
    O: OutputSinkNoAlloc,
{
    sensor: S, pre: P, model: M, post: Q, sink: O,
    state: RuntimeState, stats: FlowStatistics,

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

    preprocessor_input_queue: Deque<TensorFrame<PREPROCESSOR_MAX_BYTES, MAX_DIMS>, PREPROCESSOR_QUEUE_CAPACITY>,
    postprocessor_output_queue: Deque<TensorFrame<POSTPROCESSOR_MAX_BYTES, MAX_DIMS>, POSTPROCESSOR_QUEUE_CAPACITY>,

    backpressure_policy_for_preprocessor_input_queue: BackpressurePolicyNoAlloc,
    backpressure_policy_for_postprocessor_output_queue: BackpressurePolicyNoAlloc,
}

impl<
    S, P, M, Q, O,
    const SENSOR_MAX_BYTES: usize,
    const PREPROCESSOR_MAX_BYTES: usize,
    const MODEL_MAX_BYTES: usize,
    const POSTPROCESSOR_MAX_BYTES: usize,
    const MAX_DIMS: usize,
    const PREPROCESSOR_QUEUE_CAPACITY: usize,
    const POSTPROCESSOR_QUEUE_CAPACITY: usize,
> NoAllocQueuedRuntime<
    S, P, M, Q, O,
    SENSOR_MAX_BYTES, PREPROCESSOR_MAX_BYTES, MODEL_MAX_BYTES, POSTPROCESSOR_MAX_BYTES, MAX_DIMS,
    PREPROCESSOR_QUEUE_CAPACITY, POSTPROCESSOR_QUEUE_CAPACITY
>
where
    S: SensorSourceBorrowed, P: PreprocessorNoAlloc<MAX_DIMS>, M: ModelNoAlloc<MAX_DIMS>, Q: PostprocessorNoAlloc<MAX_DIMS>, O: OutputSinkNoAlloc,
{
    pub fn new_with_policies(sensor: S, pre: P, model: M, post: Q, sink: O, preprocessor_policy: BackpressurePolicyNoAlloc, postprocessor_policy: BackpressurePolicyNoAlloc) -> Self {
        Self {
            sensor, pre, model, post, sink,
            state: RuntimeState::Initialized, stats: FlowStatistics::default(),
            sensor_bytes: [0u8; SENSOR_MAX_BYTES],
            pre_bytes: [0u8; PREPROCESSOR_MAX_BYTES],
            model_bytes: [0u8; MODEL_MAX_BYTES],
            post_bytes: [0u8; POSTPROCESSOR_MAX_BYTES],
            pre_shape: FixedShape::new(), model_shape: FixedShape::new(), post_shape: FixedShape::new(),
            pre_data_type: DataType::Unsigned8, model_data_type: DataType::Unsigned8, post_data_type: DataType::Unsigned8,
            preprocessor_input_queue: Deque::new(), postprocessor_output_queue: Deque::new(),
            backpressure_policy_for_preprocessor_input_queue: preprocessor_policy,
            backpressure_policy_for_postprocessor_output_queue: postprocessor_policy,
        }
    }
    pub fn new(sensor: S, pre: P, model: M, post: Q, sink: O) -> Self {
        Self::new_with_policies(sensor, pre, model, post, sink, BackpressurePolicyNoAlloc::Block, BackpressurePolicyNoAlloc::DropOldest)
    }
    pub fn open(&mut self) -> Result<(), RuntimeError> {
        if self.state != RuntimeState::Initialized { return Err(RuntimeError::RuntimeNotOpen); }
        self.sensor.open().map_err(|_| RuntimeError::ComponentFailureDuringOpen)?;
        self.state = RuntimeState::Running; Ok(())
    }
    pub fn request_stop(&mut self) { if self.state == RuntimeState::Running { self.state = RuntimeState::Stopping; } }
    pub fn close(&mut self) -> Result<(), RuntimeError> { self.sensor.close().map_err(|_| RuntimeError::ComponentFailureDuringClose)?; self.state = RuntimeState::Stopped; Ok(()) }

    fn push_with_backpressure<T, const CAP: usize>(queue: &mut Deque<T, CAP>, policy: BackpressurePolicyNoAlloc, item: T) -> bool {
        if queue.push_back(item).is_ok() { return true; }
        match policy {
            BackpressurePolicyNoAlloc::Block => false,
            BackpressurePolicyNoAlloc::DropNewest => false,
            BackpressurePolicyNoAlloc::DropOldest => { let _ = queue.pop_front(); queue.push_back(item).is_ok() }
        }
    }

    pub fn run_step(&mut self) -> Result<bool, RuntimeError> {
        if self.state != RuntimeState::Running && self.state != RuntimeState::Stopping { return Err(RuntimeError::RuntimeNotOpen); }
        let mut did_any_work = false;

        match self.sensor.read_next_into(&mut self.sensor_bytes) {
            Ok(Some((sensor_len, _meta))) => {
                self.stats.total_sensor_samples_received += 1;
                self.pre_shape.clear();
                match self.pre.process(&self.sensor_bytes[..sensor_len], &mut self.pre_data_type, &mut self.pre_shape, &mut self.pre_bytes) {
                    Ok(pre_len) => {
                        self.stats.total_preprocessor_outputs_produced += 1;
                        if let Ok(frame) = TensorFrame::<PREPROCESSOR_MAX_BYTES, MAX_DIMS>::try_from_parts(self.pre_data_type, self.pre_shape.as_slice(), &self.pre_bytes[..pre_len]) {
                            let enq = Self::push_with_backpressure(&mut self.preprocessor_input_queue, self.backpressure_policy_for_preprocessor_input_queue, frame);
                            if !enq { self.stats.total_dropped_at_preprocessor_input_queue += 1; } else { did_any_work = true; }
                        } else { self.stats.total_dropped_at_preprocessor_input_queue += 1; }
                    }
                    Err(_e) => {}
                }
            }
            Ok(None) => {}
            Err(SensorError::EndOfStream) => { self.request_stop(); }
            Err(_other) => { self.request_stop(); }
        }

        if let Some(pre_frame) = self.preprocessor_input_queue.pop_front() {
            self.model_shape.clear();
            match self.model.infer(BorrowedTensorView { data_type: pre_frame.data_type, shape: pre_frame.shape.as_slice(), buffer: pre_frame.bytes.as_slice() }, &mut self.model_data_type, &mut self.model_shape, &mut self.model_bytes) {
                Ok(model_len) => {
                    self.stats.total_model_inferences_completed += 1;
                    self.post_shape.clear();
                    match self.post.process(BorrowedTensorView { data_type: self.model_data_type, shape: self.model_shape.as_slice(), buffer: &self.model_bytes[..model_len] }, &mut self.post_data_type, &mut self.post_shape, &mut self.post_bytes) {
                        Ok(post_len) => {
                            self.stats.total_postprocessor_outputs_produced += 1;
                            if let Ok(frame) = TensorFrame::<POSTPROCESSOR_MAX_BYTES, MAX_DIMS>::try_from_parts(self.post_data_type, self.post_shape.as_slice(), &self.post_bytes[..post_len]) {
                                let enq = Self::push_with_backpressure(&mut self.postprocessor_output_queue, self.backpressure_policy_for_postprocessor_output_queue, frame);
                                if !enq { self.stats.total_dropped_at_postprocessor_output_queue += 1; } else { did_any_work = true; }
                            } else { self.stats.total_dropped_at_postprocessor_output_queue += 1; }
                        }
                        Err(_e) => {}
                    }
                }
                Err(_e) => {}
            }
        }

        if let Some(post_frame) = self.postprocessor_output_queue.pop_front() {
            let _ = self.sink.write(BorrowedTensorView { data_type: post_frame.data_type, shape: post_frame.shape.as_slice(), buffer: post_frame.bytes.as_slice() }).map_err(|_e| OutputError::WriteFailed);
            self.stats.total_outputs_written += 1;
            did_any_work = true;
        }

        if self.state == RuntimeState::Stopping && self.preprocessor_input_queue.is_empty() && self.postprocessor_output_queue.is_empty() {
            self.close()?;
        }

        Ok(did_any_work)
    }

    pub fn health(&self) -> RuntimeHealth {
        RuntimeHealth {
            state: self.state, statistics: self.stats.clone(),
            preprocessor_input_queue_length: self.preprocessor_input_queue.len(),
            preprocessor_input_queue_capacity: PREPROCESSOR_QUEUE_CAPACITY,
            postprocessor_output_queue_length: self.postprocessor_output_queue.len(),
            postprocessor_output_queue_capacity: POSTPROCESSOR_QUEUE_CAPACITY,
        }
    }
    pub fn statistics(&self) -> &FlowStatistics { &self.stats }
    pub fn state(&self) -> RuntimeState { self.state }
}
