use limen_core::errors::RuntimeError;
#[cfg(feature = "alloc")]
use limen_core::runtime::{
    BackpressurePolicy, FlowStatistics, Runtime as CoreRuntime, RuntimeHealth, RuntimeState,
};

#[cfg(feature = "alloc")]
pub struct LightPipeline<S, P, M, Q, O>
where
    S: limen_core::traits::SensorStream,
    P: limen_core::traits::Preprocessor,
    M: limen_core::traits::Model,
    Q: limen_core::traits::Postprocessor,
    O: limen_core::traits::OutputSink,
{
    inner_runtime: CoreRuntime<S, P, M, Q, O>,
}

#[cfg(feature = "alloc")]
impl<S, P, M, Q, O> LightPipeline<S, P, M, Q, O>
where
    S: limen_core::traits::SensorStream,
    P: limen_core::traits::Preprocessor,
    M: limen_core::traits::Model,
    Q: limen_core::traits::Postprocessor,
    O: limen_core::traits::OutputSink,
{
    pub fn from_core_runtime(inner_runtime: CoreRuntime<S, P, M, Q, O>) -> Self {
        Self { inner_runtime }
    }
    pub fn open(&mut self) -> Result<(), RuntimeError> {
        self.inner_runtime.open()
    }
    pub fn request_stop(&mut self) {
        self.inner_runtime.request_stop();
    }
    pub fn run_step(&mut self) -> Result<bool, RuntimeError> {
        self.inner_runtime.run_step()
    }
    pub fn drain(&mut self) -> Result<(), RuntimeError> {
        self.inner_runtime.drain()
    }
    pub fn health(&self) -> RuntimeHealth {
        self.inner_runtime.health()
    }
    pub fn statistics(&self) -> &FlowStatistics {
        self.inner_runtime.statistics()
    }
    pub fn state(&self) -> RuntimeState {
        self.inner_runtime.health().state
    }
    pub fn inner_runtime_mut(&mut self) -> &mut CoreRuntime<S, P, M, Q, O> {
        &mut self.inner_runtime
    }
    pub fn inner_runtime(&self) -> &CoreRuntime<S, P, M, Q, O> {
        &self.inner_runtime
    }
    pub fn new(
        sensor_stream_instance: S,
        preprocessor_instance: P,
        model_instance: M,
        postprocessor_instance: Q,
        output_sink_instance: O,
        preprocessor_input_queue_capacity: usize,
        postprocessor_output_queue_capacity: usize,
        backpressure_policy_for_preprocessor_input_queue: BackpressurePolicy,
        backpressure_policy_for_postprocessor_output_queue: BackpressurePolicy,
    ) -> Self {
        let core_runtime = CoreRuntime::new(
            sensor_stream_instance,
            preprocessor_instance,
            model_instance,
            postprocessor_instance,
            output_sink_instance,
            preprocessor_input_queue_capacity,
            postprocessor_output_queue_capacity,
            backpressure_policy_for_preprocessor_input_queue,
            backpressure_policy_for_postprocessor_output_queue,
        );
        Self::from_core_runtime(core_runtime)
    }
    #[cfg(feature = "std")]
    pub fn run_blocking(
        &mut self,
        idle_sleep_duration_milliseconds: u64,
    ) -> Result<(), RuntimeError> {
        self.inner_runtime
            .run_blocking(idle_sleep_duration_milliseconds)
    }
}
