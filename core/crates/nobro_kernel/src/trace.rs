//! Opt-in, bounded-cost timing markers for on-target analysis.
//!
//! A caller emits markers only around the dispatch points it chooses. The
//! kernel stores no global sink and default builds do not compile this module.
//! GPIO pulses are the portable timing contract. `ArmItmSwoTrace` targets a
//! preconfigured Arm ITM/SWO session; configuring clocks, TPIU, DWT, probe, and
//! capture remains a board/debug-session responsibility. ETM is deliberately
//! not claimed as a universal API.

use core::ptr::{read_volatile, write_volatile};

/// Maximum MMIO work performed by one trace emission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraceDispatchPrice {
    pub status_reads: u8,
    pub maximum_writes: u8,
}

/// Stable 32-bit timing marker. The top byte is the event kind and the lower
/// 24 bits are a caller-supplied task/module tag.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraceEvent(u32);

impl TraceEvent {
    const TAG_MASK: u32 = 0x00ff_ffff;

    pub const fn dispatch_begin(tag: u32) -> Self {
        Self((1 << 24) | (tag & Self::TAG_MASK))
    }

    pub const fn dispatch_end(tag: u32) -> Self {
        Self((2 << 24) | (tag & Self::TAG_MASK))
    }

    pub const fn fault(tag: u32) -> Self {
        Self((3 << 24) | (tag & Self::TAG_MASK))
    }

    pub const fn code(self) -> u32 {
        self.0
    }

    pub const fn begins_dispatch(self) -> bool {
        self.0 >> 24 == 1
    }
}

/// Explicitly priced trace sink. Implementations must not allocate, block, or
/// retry inside `emit`.
pub trait TraceHook {
    const PRICE: TraceDispatchPrice;

    fn emit(&mut self, event: TraceEvent);
}

/// Zero-state sink useful to make generic tracing optional without branching.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoTrace;

impl TraceHook for NoTrace {
    const PRICE: TraceDispatchPrice = TraceDispatchPrice {
        status_reads: 0,
        maximum_writes: 0,
    };

    #[inline(always)]
    fn emit(&mut self, _event: TraceEvent) {}
}

/// One-write GPIO pulse sink for MCUs with atomic output-set/output-clear
/// registers. Dispatch-begin writes `set_register`; every other marker writes
/// `clear_register`.
pub struct GpioPulseTrace {
    set_register: *mut u32,
    clear_register: *mut u32,
    mask: u32,
}

impl GpioPulseTrace {
    /// # Safety
    ///
    /// Both addresses must remain valid, aligned, writable MMIO registers for
    /// the lifetime of the sink. `mask` must only address a GPIO already owned
    /// and configured as an output by the caller.
    pub const unsafe fn new(set_register: *mut u32, clear_register: *mut u32, mask: u32) -> Self {
        Self {
            set_register,
            clear_register,
            mask,
        }
    }
}

impl TraceHook for GpioPulseTrace {
    const PRICE: TraceDispatchPrice = TraceDispatchPrice {
        status_reads: 0,
        maximum_writes: 1,
    };

    #[inline(always)]
    fn emit(&mut self, event: TraceEvent) {
        let register = if event.begins_dispatch() {
            self.set_register
        } else {
            self.clear_register
        };
        // SAFETY: guaranteed by the constructor contract.
        unsafe { write_volatile(register, self.mask) };
    }
}

/// Bounded ITM stimulus sink for an Arm core whose ITM/SWO session has already
/// been configured. A disabled stimulus port drops the marker after one status
/// read; it never spins waiting for a debugger.
pub struct ArmItmSwoTrace {
    enable_register: *const u32,
    stimulus_register: *mut u32,
    stimulus_mask: u32,
}

impl ArmItmSwoTrace {
    /// # Safety
    ///
    /// The addresses must remain valid and aligned ITM TER/stimulus registers.
    /// `stimulus_mask` must select the configured stimulus port.
    pub const unsafe fn new(
        enable_register: *const u32,
        stimulus_register: *mut u32,
        stimulus_mask: u32,
    ) -> Self {
        Self {
            enable_register,
            stimulus_register,
            stimulus_mask,
        }
    }
}

impl TraceHook for ArmItmSwoTrace {
    const PRICE: TraceDispatchPrice = TraceDispatchPrice {
        status_reads: 1,
        maximum_writes: 1,
    };

    #[inline(always)]
    fn emit(&mut self, event: TraceEvent) {
        // SAFETY: guaranteed by the constructor contract. One read and at most
        // one write preserve a bounded dispatch path even without a debugger.
        let enabled = unsafe { read_volatile(self.enable_register) };
        if enabled & self.stimulus_mask != 0 {
            unsafe { write_volatile(self.stimulus_register, event.code()) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::size_of;

    #[test]
    fn event_encoding_is_stable_and_masks_tags() {
        assert_eq!(TraceEvent::dispatch_begin(0x12ff_ffff).code(), 0x01ff_ffff);
        assert_eq!(TraceEvent::dispatch_end(7).code(), 0x0200_0007);
        assert_eq!(TraceEvent::fault(9).code(), 0x0300_0009);
    }

    #[test]
    fn no_trace_has_zero_state_and_zero_price() {
        assert_eq!(size_of::<NoTrace>(), 0);
        assert_eq!(NoTrace::PRICE.status_reads, 0);
        assert_eq!(NoTrace::PRICE.maximum_writes, 0);
    }

    #[test]
    fn gpio_trace_uses_exactly_one_selected_write() {
        let mut set = 0u32;
        let mut clear = 0u32;
        let mut trace = unsafe { GpioPulseTrace::new(&mut set, &mut clear, 0x20) };
        trace.emit(TraceEvent::dispatch_begin(1));
        assert_eq!(set, 0x20);
        assert_eq!(clear, 0);
        trace.emit(TraceEvent::dispatch_end(1));
        assert_eq!(clear, 0x20);
        assert_eq!(GpioPulseTrace::PRICE.maximum_writes, 1);
    }

    #[test]
    fn itm_trace_never_waits_for_an_enabled_port() {
        let mut enable = 0u32;
        let mut stimulus = 0u32;
        let enable_register = core::ptr::addr_of_mut!(enable);
        let mut trace = unsafe { ArmItmSwoTrace::new(enable_register, &mut stimulus, 4) };
        trace.emit(TraceEvent::dispatch_begin(3));
        assert_eq!(stimulus, 0);

        unsafe { write_volatile(enable_register, 4) };
        trace.emit(TraceEvent::fault(3));
        assert_eq!(stimulus, 0x0300_0003);
        assert_eq!(ArmItmSwoTrace::PRICE.status_reads, 1);
    }
}
