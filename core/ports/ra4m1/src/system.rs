//! Host-testable RA4M1 clock/module-start transaction.
//!
//! The register adapter stays in the target binary. This module owns the ordering and
//! fail-closed policy so CI executes the same transaction that target startup uses.

pub const HOCO_HZ: u32 = 48_000_000;
pub const PCLKB_DIVIDER: u32 = 2;
pub const PCLKB_HZ: u32 = HOCO_HZ / PCLKB_DIVIDER;
pub const SCI_BAUD: u32 = 115_200;
pub const SCI_ASYNC_DIVISOR: u32 = 16;
pub const HOCO_STABILIZATION_POLLS: usize = 100_000;

const DIVIDE_BY_1: u32 = 0;
const DIVIDE_BY_2: u32 = 1;
const FCLK_SHIFT: u32 = 28;
const ICLK_SHIFT: u32 = 24;
const BCLK_SHIFT: u32 = 16;
const PCLKA_SHIFT: u32 = 12;
const PCLKB_SHIFT: u32 = 8;
const PCLKC_SHIFT: u32 = 4;
const PCLKD_SHIFT: u32 = 0;

/// UNO R4 WiFi/FSP-compatible clock-divider image: ICLK/PCLKA/PCLKC/PCLKD 48 MHz,
/// PCLKB/BCLK/FCLK 24 MHz.
pub const SCKDIVCR_VALUE: u32 = (DIVIDE_BY_2 << FCLK_SHIFT)
    | (DIVIDE_BY_1 << ICLK_SHIFT)
    | (DIVIDE_BY_2 << BCLK_SHIFT)
    | (DIVIDE_BY_1 << PCLKA_SHIFT)
    | (DIVIDE_BY_2 << PCLKB_SHIFT)
    | (DIVIDE_BY_1 << PCLKC_SHIFT)
    | (DIVIDE_BY_1 << PCLKD_SHIFT);
pub const SCKSCR_HOCO: u8 = 0;
pub const MEMWAIT_48MHZ: u8 = 1;
const _: () = assert!(SCKDIVCR_VALUE == 0x1001_0100);

pub const SCI_BRR: u8 = {
    let denominator = SCI_ASYNC_DIVISOR * SCI_BAUD;
    ((PCLKB_HZ + denominator / 2) / denominator - 1) as u8
};
pub const SCI_ACTUAL_BAUD: u32 = PCLKB_HZ / (SCI_ASYNC_DIVISOR * (SCI_BRR as u32 + 1));
pub const SCI_BAUD_ERROR_PPM: u32 = SCI_ACTUAL_BAUD.abs_diff(SCI_BAUD) * 1_000_000 / SCI_BAUD;
const _: () = assert!(SCI_BRR == 12 && SCI_BAUD_ERROR_PPM < 20_000);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemInitError {
    HocoTimeout,
    ClockTreeReadback,
    UsbClockReadback,
}

/// Minimal register transaction used by [`configure_system`].
pub trait SystemRegisters {
    fn set_vector_table(&mut self);
    fn unlock_protected_registers(&mut self);
    fn start_hoco(&mut self);
    fn hoco_stable(&self) -> bool;
    fn enable_flash_wait_state(&mut self);
    fn program_clock_dividers(&mut self);
    fn select_hoco_as_system_clock(&mut self);
    fn clock_tree_matches(&self) -> bool;
    fn select_hoco_for_usb(&mut self);
    fn usb_hoco_selected(&self) -> bool;
    fn wake_required_modules(&mut self);
    fn lock_protected_registers(&mut self);
}

/// Configure clocks and module-stop state, relocking protected registers on every exit.
pub fn configure_system(registers: &mut impl SystemRegisters) -> Result<(), SystemInitError> {
    registers.set_vector_table();
    registers.unlock_protected_registers();
    registers.start_hoco();

    let mut stable = false;
    for _ in 0..HOCO_STABILIZATION_POLLS {
        if registers.hoco_stable() {
            stable = true;
            break;
        }
        core::hint::spin_loop();
    }
    if !stable {
        registers.lock_protected_registers();
        return Err(SystemInitError::HocoTimeout);
    }

    // ICLK rises from reset-time MOCO to 48 MHz. Program the flash wait state before
    // selecting the faster source, then validate all divider/source fields together.
    registers.enable_flash_wait_state();
    registers.program_clock_dividers();
    registers.select_hoco_as_system_clock();
    if !registers.clock_tree_matches() {
        registers.lock_protected_registers();
        return Err(SystemInitError::ClockTreeReadback);
    }
    registers.select_hoco_for_usb();
    if !registers.usb_hoco_selected() {
        registers.lock_protected_registers();
        return Err(SystemInitError::UsbClockReadback);
    }
    // PRCR.PRC1 must still be unlocked here; otherwise module-stop writes are ignored.
    registers.wake_required_modules();
    registers.lock_protected_registers();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{configure_system, SystemInitError, SystemRegisters};

    const VECTOR: u8 = 1;
    const UNLOCK: u8 = 2;
    const START: u8 = 3;
    const WAIT_STATE: u8 = 4;
    const DIVIDERS: u8 = 5;
    const SYSTEM_CLOCK: u8 = 6;
    const USB_CLOCK: u8 = 7;
    const WAKE: u8 = 8;
    const LOCK: u8 = 9;

    struct FakeRegisters {
        events: [u8; 9],
        count: usize,
        stable: bool,
        clock_readback: bool,
        usb_readback: bool,
    }

    impl FakeRegisters {
        fn new(stable: bool, clock_readback: bool, usb_readback: bool) -> Self {
            Self {
                events: [0; 9],
                count: 0,
                stable,
                clock_readback,
                usb_readback,
            }
        }

        fn record(&mut self, event: u8) {
            self.events[self.count] = event;
            self.count += 1;
        }

        fn observed(&self) -> &[u8] {
            &self.events[..self.count]
        }
    }

    impl SystemRegisters for FakeRegisters {
        fn set_vector_table(&mut self) {
            self.record(VECTOR);
        }
        fn unlock_protected_registers(&mut self) {
            self.record(UNLOCK);
        }
        fn start_hoco(&mut self) {
            self.record(START);
        }
        fn hoco_stable(&self) -> bool {
            self.stable
        }
        fn enable_flash_wait_state(&mut self) {
            self.record(WAIT_STATE);
        }
        fn program_clock_dividers(&mut self) {
            self.record(DIVIDERS);
        }
        fn select_hoco_as_system_clock(&mut self) {
            self.record(SYSTEM_CLOCK);
        }
        fn clock_tree_matches(&self) -> bool {
            self.clock_readback
        }
        fn select_hoco_for_usb(&mut self) {
            self.record(USB_CLOCK);
        }
        fn usb_hoco_selected(&self) -> bool {
            self.usb_readback
        }
        fn wake_required_modules(&mut self) {
            self.record(WAKE);
        }
        fn lock_protected_registers(&mut self) {
            self.record(LOCK);
        }
    }

    #[test]
    fn startup_order_keeps_module_stop_write_inside_unlock_window() {
        let mut registers = FakeRegisters::new(true, true, true);
        assert_eq!(configure_system(&mut registers), Ok(()));
        assert_eq!(
            registers.observed(),
            &[
                VECTOR,
                UNLOCK,
                START,
                WAIT_STATE,
                DIVIDERS,
                SYSTEM_CLOCK,
                USB_CLOCK,
                WAKE,
                LOCK,
            ]
        );
    }

    #[test]
    fn hoco_timeout_relocks_without_programming_consumers() {
        let mut registers = FakeRegisters::new(false, true, true);
        assert_eq!(
            configure_system(&mut registers),
            Err(SystemInitError::HocoTimeout)
        );
        assert_eq!(registers.observed(), &[VECTOR, UNLOCK, START, LOCK]);
    }

    #[test]
    fn failed_system_clock_readback_relocks_before_usb_or_modules() {
        let mut registers = FakeRegisters::new(true, false, true);
        assert_eq!(
            configure_system(&mut registers),
            Err(SystemInitError::ClockTreeReadback)
        );
        assert_eq!(
            registers.observed(),
            &[
                VECTOR,
                UNLOCK,
                START,
                WAIT_STATE,
                DIVIDERS,
                SYSTEM_CLOCK,
                LOCK
            ]
        );
    }

    #[test]
    fn failed_usb_selector_readback_relocks_without_waking_modules() {
        let mut registers = FakeRegisters::new(true, true, false);
        assert_eq!(
            configure_system(&mut registers),
            Err(SystemInitError::UsbClockReadback)
        );
        assert_eq!(
            registers.observed(),
            &[
                VECTOR,
                UNLOCK,
                START,
                WAIT_STATE,
                DIVIDERS,
                SYSTEM_CLOCK,
                USB_CLOCK,
                LOCK,
            ]
        );
    }
}
