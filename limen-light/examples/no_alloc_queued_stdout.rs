use limen_light::no_alloc::runtime_queued::{NoAllocQueuedRuntime, BackpressurePolicyNoAlloc};
use limen_light::no_alloc::components::sensor_simulated::{SimulatedSensorNoAlloc, SimulatedPattern};
use limen_light::no_alloc::components::pre_identity::IdentityPreprocessorNoAlloc;
use limen_light::no_alloc::components::model_identity::IdentityModelNoAlloc;
use limen_light::no_alloc::components::post_identity::IdentityPostprocessorNoAlloc;
use limen_light::no_alloc::components::sink_stdout::StandardOutputSinkNoAlloc;
use limen_core::types::DataType;

fn main() {
    let sensor = SimulatedSensorNoAlloc::new(16, 8, SimulatedPattern::Ramp);
    let pre = IdentityPreprocessorNoAlloc::new(DataType::Unsigned8, None);
    let model = IdentityModelNoAlloc::default();
    let post = IdentityPostprocessorNoAlloc::default();
    let sink = StandardOutputSinkNoAlloc::new(8);

    let mut rt: NoAllocQueuedRuntime<_,_,_,_,_, 64,64,64,64, 4, 4, 4> =
        NoAllocQueuedRuntime::new_with_policies(sensor, pre, model, post, sink, BackpressurePolicyNoAlloc::Block, BackpressurePolicyNoAlloc::DropOldest);
    rt.open().unwrap();
    loop {
        let made_progress = rt.run_step().unwrap();
        if !made_progress { }
        if rt.state() == limen_core::runtime::RuntimeState::Stopped { break; }
    }
}
