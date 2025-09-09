#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
use alloc::{collections::VecDeque, boxed::Box, vec::Vec};

use crate::errors::{RuntimeError};
use crate::types::{TensorInput, TensorOutput};
use crate::traits::{SensorStream, Preprocessor, Model, Postprocessor, OutputSink};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackpressurePolicy {
    Block,
    DropNewest,
    DropOldest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeState {
    Initialized,
    Running,
    Stopping,
    Stopped,
}

#[derive(Clone, Debug, Default)]
pub struct FlowStatistics {
    pub total_sensor_samples_received: u64,
    pub total_preprocessor_outputs_produced: u64,
    pub total_model_inferences_completed: u64,
    pub total_postprocessor_outputs_produced: u64,
    pub total_outputs_written: u64,
    pub total_dropped_at_preprocessor_input_queue: u64,
    pub total_dropped_at_postprocessor_output_queue: u64,
}

#[derive(Clone, Debug)]
pub struct RuntimeHealth {
    pub state: RuntimeState,
    pub statistics: FlowStatistics,
    pub preprocessor_input_queue_length: usize,
    pub preprocessor_input_queue_capacity: usize,
    pub postprocessor_output_queue_length: usize,
    pub postprocessor_output_queue_capacity: usize,
}

pub struct Runtime<S, P, M, Q, O>
where
    S: SensorStream,
    P: Preprocessor,
    M: Model,
    Q: Postprocessor,
    O: OutputSink,
{
    sensor: S,
    pre: P,
    model: M,
    post: Q,
    sink: O,

    state: RuntimeState,
    stats: FlowStatistics,

    pre_q: VecDeque<TensorInput>,
    post_q: VecDeque<TensorOutput>,

    pre_q_cap: usize,
    post_q_cap: usize,

    pre_policy: BackpressurePolicy,
    post_policy: BackpressurePolicy,
}

impl<S, P, M, Q, O> Runtime<S, P, M, Q, O>
where
    S: SensorStream,
    P: Preprocessor,
    M: Model,
    Q: Postprocessor,
    O: OutputSink,
{
    pub fn new(
        sensor: S,
        pre: P,
        model: M,
        post: Q,
        sink: O,
        pre_q_cap: usize,
        post_q_cap: usize,
        pre_policy: BackpressurePolicy,
        post_policy: BackpressurePolicy,
    ) -> Self {
        Self {
            sensor, pre, model, post, sink,
            state: RuntimeState::Initialized,
            stats: FlowStatistics::default(),
            pre_q: VecDeque::with_capacity(pre_q_cap.max(1)),
            post_q: VecDeque::with_capacity(post_q_cap.max(1)),
            pre_q_cap: pre_q_cap.max(1),
            post_q_cap: post_q_cap.max(1),
            pre_policy,
            post_policy,
        }
    }

    pub fn open(&mut self) -> Result<(), RuntimeError> {
        if self.state != RuntimeState::Initialized {
            return Err(RuntimeError::RuntimeNotOpen);
        }
        self.sensor.open().map_err(|_| RuntimeError::ComponentFailureDuringOpen)?;
        self.state = RuntimeState::Running;
        Ok(())
    }

    pub fn request_stop(&mut self) {
        if self.state == crate::runtime::RuntimeState::Running {
            self.state = crate::runtime::RuntimeState::Stopping;
        }
    }

    fn push_with_backpressure<T>(
        q: &mut VecDeque<T>,
        cap: usize,
        policy: BackpressurePolicy,
        mut item: T,
    ) -> bool {
        if q.len() < cap {
            q.push_back(item);
            true
        } else {
            match policy {
                BackpressurePolicy::Block => false,
                BackpressurePolicy::DropNewest => false,
                BackpressurePolicy::DropOldest => {
                    q.pop_front();
                    q.push_back(item);
                    true
                }
            }
        }
    }

    pub fn run_step(&mut self) -> Result<bool, RuntimeError> {
        use crate::errors::{SensorError, ProcessingError, InferenceError, OutputError};

        if self.state != RuntimeState::Running && self.state != RuntimeState::Stopping {
            return Err(RuntimeError::RuntimeNotOpen);
        }

        let mut did_work = false;

        // 1) Sensor -> Preprocessor -> enqueue pre
        match self.sensor.read_next() {
            Ok(Some(data)) => {
                self.stats.total_sensor_samples_received = self.stats.total_sensor_samples_received.saturating_add(1);
                match self.pre.process(data) {
                    Ok(ti) => {
                        if Self::push_with_backpressure(&mut self.pre_q, self.pre_q_cap, self.pre_policy, ti) {
                            self.stats.total_preprocessor_outputs_produced = self.stats.total_preprocessor_outputs_produced.saturating_add(1);
                            did_work = true;
                        } else {
                            self.stats.total_dropped_at_preprocessor_input_queue = self.stats.total_dropped_at_preprocessor_input_queue.saturating_add(1);
                        }
                    }
                    Err(_e) => { /* drop this sample */ }
                }
            }
            Ok(None) => { /* no data */ }
            Err(SensorError::EndOfStream) => { self.request_stop(); }
            Err(_other) => { self.request_stop(); }
        }

        // 2) Model + Postprocess -> enqueue post
        if let Some(ti) = self.pre_q.pop_front() {
            match self.model.infer(&ti) {
                Ok(to_mid) => {
                    self.stats.total_model_inferences_completed = self.stats.total_model_inferences_completed.saturating_add(1);
                    match self.post.process(to_mid) {
                        Ok(to) => {
                            self.stats.total_postprocessor_outputs_produced = self.stats.total_postprocessor_outputs_produced.saturating_add(1);
                            if Self::push_with_backpressure(&mut self.post_q, self.post_q_cap, self.post_policy, to) {
                                did_work = true
                            } else {
                                self.stats.total_dropped_at_postprocessor_output_queue = self.stats.total_dropped_at_postprocessor_output_queue.saturating_add(1);
                            }
                        }
                        Err(_e) => { /* drop */ }
                    }
                }
                Err(_e) => { /* drop */ }
            }
        }

        // 3) Sink write
        if let Some(to) = self.post_q.pop_front() {
            let _ = self.sink.write(&to).map_err(|_e| OutputError::WriteFailed);
            self.stats.total_outputs_written = self.stats.total_outputs_written.saturating_add(1);
            did_work = true;
        }

        // 4) stopping condition
        if self.state == RuntimeState::Stopping && self.pre_q.is_empty() && self.post_q.is_empty() {
            self.sink.close().ok();
            self.sensor.close().ok();
            self.state = RuntimeState::Stopped;
        }

        Ok(did_work)
    }

    pub fn drain(&mut self) -> Result<(), RuntimeError> {
        while self.state != RuntimeState::Stopped {
            let progressed = self.run_step()?;
            if !progressed {
                // No idle sleeping without std; in std path, run_blocking does it.
                if self.state == RuntimeState::Running {
                    // Request stop when nothing else arrives; callers usually request_stop explicitly.
                    self.request_stop();
                }
            }
        }
        Ok(())
    }

    #[cfg(feature = "std")]
    pub fn run_blocking(&mut self, idle_sleep_duration_milliseconds: u64) -> Result<(), RuntimeError> {
        use std::time::Duration;
        use std::thread::sleep;
        self.open()?;
        loop {
            let progressed = self.run_step()?;
            if !progressed {
                sleep(Duration::from_millis(idle_sleep_duration_milliseconds));
            }
            if self.state == RuntimeState::Stopped { break; }
        }
        Ok(())
    }

    pub fn health(&self) -> RuntimeHealth {
        RuntimeHealth {
            state: self.state,
            statistics: self.stats.clone(),
            preprocessor_input_queue_length: self.pre_q.len(),
            preprocessor_input_queue_capacity: self.pre_q_cap,
            postprocessor_output_queue_length: self.post_q.len(),
            postprocessor_output_queue_capacity: self.post_q_cap,
        }
    }

    pub fn statistics(&self) -> &FlowStatistics { &self.stats }
}
