//! USB stack modularity demo: the app talks only to `nobro_usb::mount()` + the `UsbStack`
//! trait, never a concrete backend. Building with `--features backend-nrf-usbd` (default),
//! a board-supported backend swaps the whole USB stack under the same app code - the
//! "modularized, configurable, mountable" USB requirement. This build mounts
//! the nrf-usbd backend and echoes CDC bytes once enumerated (a NobroRTOS greeting on
//! connect), matching ArduinoNRF Layer-0's native NrfUsbd behavior.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

use nobro_usb::{mount, CdcState, UsbConfig, UsbStack};

#[entry]
fn main() -> ! {
    let cfg = UsbConfig::new(
        0x1209,
        0x0001,
        "NiusRobotLab",
        "NobroRTOS Modular CDC",
        "NBRO-USB",
    );
    let mut usb = mount(&cfg);
    let _ = usb.backend_id(); // which stack got mounted (for diagnostics)

    let mut greeted = false;
    let mut rx = [0u8; 64];

    loop {
        if usb.poll() == CdcState::Configured {
            if !greeted {
                let banner = b"NobroRTOS modular USB stack up.\r\n";
                if usb.write(banner) != banner.len() {
                    defmt::warn!("USB banner backpressure");
                }
                greeted = true;
            }
            // echo whatever the host sends
            let n = usb.read(&mut rx);
            if n > 0 {
                if usb.write(&rx[..n]) != n {
                    defmt::warn!("USB echo backpressure");
                }
            }
        } else {
            greeted = false;
        }
    }
}
