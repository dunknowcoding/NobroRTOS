#![no_main]

use libfuzzer_sys::fuzz_target;
use nobro_kernel::{
    Capability, CapabilityGrantTable, CapabilityReplayScope, CapabilitySet, CapabilityTrace,
    CapabilityTraceInput, CapabilityTraceOp, FaultThresholds, HealthMonitor, KernelError,
    Mailbox, Message, MessageKind, ModuleId,
};

fuzz_target!(|data: &[u8]| {
    let mut mailbox = Mailbox::<16>::with_control_reserve(2);
    let mut health = HealthMonitor::<4>::new();
    let mut grants = CapabilityGrantTable::<4>::new();
    let _ = grants.register(ModuleId::Sensor, CapabilitySet::empty().with(Capability::Mailbox));
    let mut trace = CapabilityTrace::<16>::new();

    for (sequence, chunk) in data.chunks(4).enumerate() {
        let opcode = chunk.first().copied().unwrap_or(0);
        let module = if opcode & 1 == 0 { ModuleId::Sensor } else { ModuleId::Radio };
        match opcode % 5 {
            0 => { let _ = mailbox.push(Message::new(module, ModuleId::Kernel, MessageKind::Command, sequence as u32, 0)); }
            1 => { let _ = mailbox.pop_for(module); }
            2 => { let _ = health.record_error(module, KernelError::BusTimeout, sequence as u64, FaultThresholds::DEFAULT, |_| nobro_kernel::Action::RetryNow); }
            3 => { let _ = trace.record_authorized(&grants, CapabilityTraceInput::new(module, Capability::Mailbox, CapabilityTraceOp::Write, sequence as u64)); }
            _ => { let mut out = [nobro_kernel::CapabilityTraceRecord::EMPTY; 4]; let _ = trace.copy_replay(CapabilityReplayScope::all(), &mut out); }
        }
    }
});
