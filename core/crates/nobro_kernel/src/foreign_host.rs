//! Dispatcher-owned identity, authorization, quota, and trace for foreign host calls.

use core::cell::RefCell;

use critical_section::Mutex;

use crate::{
    Capability, CapabilityGrantTable, CapabilityReplayScope, CapabilitySet, CapabilityTrace,
    CapabilityTraceInput, CapabilityTraceOp, CapabilityTraceRecord, ModuleId, ModuleLaunchGate,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForeignHostQuota {
    pub max_calls: u32,
    pub max_bytes: u32,
}

impl ForeignHostQuota {
    pub const fn new(max_calls: u32, max_bytes: u32) -> Self {
        Self {
            max_calls,
            max_bytes,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForeignHostUsage {
    pub calls: u32,
    pub bytes: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForeignHostError {
    NotAdmitted,
    CapabilityDenied,
    QuotaExceeded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForeignBufferError {
    Null,
    Empty,
    TooLong,
    AddressOverflow,
    Overlap,
}

/// Validate a foreign read-only byte range before an FFI adapter constructs a
/// Rust slice. A null pointer is accepted only for an explicitly allowed
/// zero-length range.
pub fn validate_foreign_read(
    pointer: *const u8,
    len: u32,
    max_len: u32,
    allow_empty: bool,
) -> Result<usize, ForeignBufferError> {
    let len = validate_foreign_range(pointer.addr(), len, max_len, allow_empty)?;
    if len != 0 && pointer.is_null() {
        return Err(ForeignBufferError::Null);
    }
    Ok(len)
}

/// Validate disjoint read/write byte ranges for a foreign duplex operation.
///
/// Both phases must be non-empty. The function only inspects addresses and
/// lengths; the caller remains responsible for constructing slices inside the
/// foreign call's validity window.
pub fn validate_foreign_read_write(
    read: *const u8,
    read_len: u32,
    write: *mut u8,
    write_len: u32,
    max_phase_len: u32,
) -> Result<(usize, usize), ForeignBufferError> {
    let read_len = validate_foreign_read(read, read_len, max_phase_len, false)?;
    let write_len = validate_foreign_range(write.addr(), write_len, max_phase_len, false)?;
    if write.is_null() {
        return Err(ForeignBufferError::Null);
    }
    let read_start = read.addr();
    let write_start = write.addr();
    let read_end = read_start
        .checked_add(read_len)
        .ok_or(ForeignBufferError::AddressOverflow)?;
    let write_end = write_start
        .checked_add(write_len)
        .ok_or(ForeignBufferError::AddressOverflow)?;
    if read_start < write_end && write_start < read_end {
        return Err(ForeignBufferError::Overlap);
    }
    Ok((read_len, write_len))
}

fn validate_foreign_range(
    address: usize,
    len: u32,
    max_len: u32,
    allow_empty: bool,
) -> Result<usize, ForeignBufferError> {
    if len == 0 {
        return if allow_empty {
            Ok(0)
        } else {
            Err(ForeignBufferError::Empty)
        };
    }
    if address == 0 {
        return Err(ForeignBufferError::Null);
    }
    if len > max_len {
        return Err(ForeignBufferError::TooLong);
    }
    let len = len as usize;
    address
        .checked_add(len)
        .ok_or(ForeignBufferError::AddressOverflow)?;
    Ok(len)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForeignHostCall {
    pub capability: Capability,
    pub op: CapabilityTraceOp,
    pub at_us: u64,
    pub arg0: u32,
    pub arg1: u32,
    pub bytes: u32,
}

impl ForeignHostCall {
    pub const fn new(capability: Capability, op: CapabilityTraceOp, at_us: u64) -> Self {
        Self {
            capability,
            op,
            at_us,
            arg0: 0,
            arg1: 0,
            bytes: 0,
        }
    }

    pub const fn args(mut self, arg0: u32, arg1: u32) -> Self {
        self.arg0 = arg0;
        self.arg1 = arg1;
        self
    }

    pub const fn bytes(mut self, bytes: u32) -> Self {
        self.bytes = bytes;
        self
    }
}

struct ForeignHostState<const TRACE: usize> {
    module: Option<ModuleId>,
    grants: CapabilityGrantTable<1>,
    trace: CapabilityTrace<TRACE>,
    usage: ForeignHostUsage,
}

impl<const TRACE: usize> ForeignHostState<TRACE> {
    const fn new() -> Self {
        Self {
            module: None,
            grants: CapabilityGrantTable::new(),
            trace: CapabilityTrace::new(),
            usage: ForeignHostUsage { calls: 0, bytes: 0 },
        }
    }
}

/// The only entry point for a foreign module to invoke a protected host operation.
pub struct ForeignHostContext<'a, const TRACE: usize> {
    gate: &'a ModuleLaunchGate,
    quota: ForeignHostQuota,
    state: Mutex<RefCell<ForeignHostState<TRACE>>>,
}

impl<'a, const TRACE: usize> ForeignHostContext<'a, TRACE> {
    pub const fn new(gate: &'a ModuleLaunchGate, quota: ForeignHostQuota) -> Self {
        Self {
            gate,
            quota,
            state: Mutex::new(RefCell::new(ForeignHostState::new())),
        }
    }

    pub fn admit(&self, module: ModuleId, granted: CapabilitySet) {
        critical_section::with(|cs| {
            let mut state = self.state.borrow(cs).borrow_mut();
            *state = ForeignHostState::new();
            state.module = Some(module);
            let _ = state.grants.register(module, granted);
        });
    }

    pub fn invoke<F>(&self, call: ForeignHostCall, operation: F) -> Result<i32, ForeignHostError>
    where
        F: FnOnce() -> i32,
    {
        let ForeignHostCall {
            capability,
            op,
            at_us,
            arg0,
            arg1,
            bytes,
        } = call;
        let module = critical_section::with(|cs| {
            let mut state = self.state.borrow(cs).borrow_mut();
            let Some(module) = state.module else {
                return Err(ForeignHostError::NotAdmitted);
            };
            if !self.gate.is_admitted() || state.grants.authorize(module, capability).is_err() {
                state.trace.record(CapabilityTraceInput::new(
                    module,
                    capability,
                    CapabilityTraceOp::Fault,
                    at_us,
                ));
                return Err(ForeignHostError::CapabilityDenied);
            }
            let calls = state.usage.calls.saturating_add(1);
            let used_bytes = state.usage.bytes.saturating_add(bytes);
            if calls > self.quota.max_calls || used_bytes > self.quota.max_bytes {
                state.trace.record(
                    CapabilityTraceInput::new(module, capability, CapabilityTraceOp::Fault, at_us)
                        .args(arg0, arg1)
                        .result(2),
                );
                return Err(ForeignHostError::QuotaExceeded);
            }
            state.usage = ForeignHostUsage {
                calls,
                bytes: used_bytes,
            };
            state.trace.record(
                CapabilityTraceInput::new(module, capability, op, at_us)
                    .args(arg0, arg1)
                    .result(u32::MAX),
            );
            Ok(module)
        })?;

        let result = operation();
        critical_section::with(|cs| {
            self.state.borrow(cs).borrow_mut().trace.record(
                CapabilityTraceInput::new(module, capability, op, at_us)
                    .args(arg0, arg1)
                    .result(result as u32),
            );
        });
        Ok(result)
    }

    pub fn usage(&self) -> ForeignHostUsage {
        critical_section::with(|cs| self.state.borrow(cs).borrow().usage)
    }

    pub fn copy_trace(&self, out: &mut [CapabilityTraceRecord]) -> usize {
        critical_section::with(|cs| {
            self.state
                .borrow(cs)
                .borrow()
                .trace
                .copy_replay(CapabilityReplayScope::all(), out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;

    #[test]
    fn identity_capability_quota_operation_and_trace_are_one_path() {
        let gate = ModuleLaunchGate::new();
        let host = ForeignHostContext::<8>::new(&gate, ForeignHostQuota::new(1, 4));
        let calls = Cell::new(0);
        let grants = CapabilitySet::empty().with(Capability::Bus0);
        gate.install(grants);
        host.admit(ModuleId::Sensor, grants);

        assert_eq!(
            host.invoke(
                ForeignHostCall::new(Capability::Bus0, CapabilityTraceOp::Write, 10)
                    .args(0x68, 4)
                    .bytes(4),
                || {
                    calls.set(calls.get() + 1);
                    0
                }
            ),
            Ok(0)
        );
        assert_eq!(
            host.invoke(
                ForeignHostCall::new(Capability::Bus0, CapabilityTraceOp::Write, 20)
                    .args(0x68, 1)
                    .bytes(1),
                || {
                    calls.set(calls.get() + 1);
                    0
                }
            ),
            Err(ForeignHostError::QuotaExceeded)
        );
        assert_eq!(calls.get(), 1);
        assert_eq!(host.usage(), ForeignHostUsage { calls: 1, bytes: 4 });
        let mut trace = [CapabilityTraceRecord::EMPTY; 8];
        assert_eq!(host.copy_trace(&mut trace), 3);
        assert!(trace[..3]
            .iter()
            .all(|record| record.module == ModuleId::Sensor));
        assert_eq!(trace[2].op, CapabilityTraceOp::Fault);
    }

    #[test]
    fn foreign_ranges_reject_null_empty_oversize_wrap_and_overlap() {
        assert_eq!(validate_foreign_read(core::ptr::null(), 0, 8, true), Ok(0));
        assert_eq!(
            validate_foreign_read(core::ptr::null(), 1, 8, true),
            Err(ForeignBufferError::Null)
        );
        let bytes = [0u8; 8];
        assert_eq!(
            validate_foreign_read(bytes.as_ptr(), 0, 8, false),
            Err(ForeignBufferError::Empty)
        );
        assert_eq!(
            validate_foreign_read(bytes.as_ptr(), 9, 8, false),
            Err(ForeignBufferError::TooLong)
        );
        assert_eq!(
            validate_foreign_range(usize::MAX, 1, 8, false),
            Err(ForeignBufferError::AddressOverflow)
        );

        let mut duplex = [0u8; 8];
        assert_eq!(
            validate_foreign_read_write(
                duplex.as_ptr(),
                4,
                duplex.as_mut_ptr().wrapping_add(2),
                4,
                8
            ),
            Err(ForeignBufferError::Overlap)
        );
        assert_eq!(
            validate_foreign_read_write(
                duplex.as_ptr(),
                4,
                duplex.as_mut_ptr().wrapping_add(4),
                4,
                8
            ),
            Ok((4, 4))
        );
    }
}
