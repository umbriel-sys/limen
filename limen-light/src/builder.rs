#[cfg(feature = "alloc")]
use limen_core::runtime::BackpressurePolicy;

#[cfg(feature = "alloc")]
use crate::pipeline::LightPipeline;

#[cfg(feature = "alloc")]
#[derive(Clone, Debug)]
pub struct LightPipelineBuilder {
    preprocessor_input_queue_capacity: usize,
    postprocessor_output_queue_capacity: usize,
    backpressure_policy_for_preprocessor_input_queue: BackpressurePolicy,
    backpressure_policy_for_postprocessor_output_queue: BackpressurePolicy,
}

#[cfg(feature = "alloc")]
impl LightPipelineBuilder {
    pub fn new() -> Self {
        Self {
            preprocessor_input_queue_capacity: 64,
            postprocessor_output_queue_capacity: 64,
            backpressure_policy_for_preprocessor_input_queue: BackpressurePolicy::Block,
            backpressure_policy_for_postprocessor_output_queue: BackpressurePolicy::DropOldest,
        }
    }
    pub fn with_preprocessor_input_queue_capacity(mut self, capacity: usize) -> Self {
        self.preprocessor_input_queue_capacity = capacity.max(1);
        self
    }
    pub fn with_postprocessor_output_queue_capacity(mut self, capacity: usize) -> Self {
        self.postprocessor_output_queue_capacity = capacity.max(1);
        self
    }
    pub fn with_backpressure_policy_for_preprocessor_input_queue(
        mut self,
        policy: BackpressurePolicy,
    ) -> Self {
        self.backpressure_policy_for_preprocessor_input_queue = policy;
        self
    }
    pub fn with_backpressure_policy_for_postprocessor_output_queue(
        mut self,
        policy: BackpressurePolicy,
    ) -> Self {
        self.backpressure_policy_for_postprocessor_output_queue = policy;
        self
    }
    pub fn build<S, P, M, Q, O>(
        self,
        sensor_stream_instance: S,
        preprocessor_instance: P,
        model_instance: M,
        postprocessor_instance: Q,
        output_sink_instance: O,
    ) -> LightPipeline<S, P, M, Q, O>
    where
        S: limen_core::traits::SensorStream,
        P: limen_core::traits::Preprocessor,
        M: limen_core::traits::Model,
        Q: limen_core::traits::Postprocessor,
        O: limen_core::traits::OutputSink,
    {
        LightPipeline::new(
            sensor_stream_instance,
            preprocessor_instance,
            model_instance,
            postprocessor_instance,
            output_sink_instance,
            self.preprocessor_input_queue_capacity,
            self.postprocessor_output_queue_capacity,
            self.backpressure_policy_for_preprocessor_input_queue,
            self.backpressure_policy_for_postprocessor_output_queue,
        )
    }
}

#[cfg(feature = "alloc")]
impl Default for LightPipelineBuilder {
    fn default() -> Self {
        Self::new()
    }
}
