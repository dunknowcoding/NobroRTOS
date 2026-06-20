//! MVK: GPIOTE -> PPI -> TIMER0 CAPTURE, log timestamps over defmt/RTT.
//! Target: board1 (ProMicro no-SD @ 0x1000), J-Link + RTT.

#![no_std]
#![no_main]

use cortex_m::asm;
use defmt_rtt as _;
use panic_probe as _;

use nobro_hal::{
    lease::{Resource, ResourceLease},
    ppi,
    timer::MicroTimer,
};

const OWNER: u8 = 1;

#[cortex_m_rt::entry]
fn main() -> ! {
    defmt::info!("NobroRTOS MVK ppi_timestamp start");

    if ResourceLease::acquire(Resource::Timer0, OWNER).is_err() {
        defmt::panic!("Timer0 lease failed");
    }

    // Assume reset defaults bring HFCLK; Phase 1 adds explicit clock init.
    unsafe {
        MicroTimer::init();
        ppi::led_init_output();
        ppi::trigger_input_init();
        ppi::mvk_setup_gpiote_ppi_capture();
    }

    defmt::info!("Trigger D2/P0.17 low->hi; RTT logs capture_us vs now_us");

    let mut last_capture: u32 = 0;
    let mut toggle: bool = false;

    loop {
        unsafe {
            // Poll GPIOTE event (PPI already latched TIMER CC[1] on edge).
            if (*nrf52840_pac::GPIOTE::ptr()).events_in[0].read().bits() != 0 {
                (*nrf52840_pac::GPIOTE::ptr()).events_in[0].reset();
                let captured = MicroTimer::captured_cc1_us();
                let now = MicroTimer::now_us() as u32;
                let delta = now.wrapping_sub(captured);
                if captured != last_capture {
                    last_capture = captured;
                    ppi::led_toggle();
                    defmt::info!("capture_us={} now_us={} delta_us={}", captured, now, delta);
                }
            }

            // Heartbeat: software toggle for scope sanity (1 Hz).
            if toggle {
                toggle = false;
            } else {
                asm::delay(16_000_000);
                toggle = true;
                ppi::led_toggle();
                defmt::trace!("heartbeat now_us={}", MicroTimer::now_us());
            }
        }
    }
}
