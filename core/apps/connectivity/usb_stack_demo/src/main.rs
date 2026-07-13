//! Cortex-M USB stack modularity demo. The app talks only to
//! `nobro_usb::try_mount()` + [`UsbStack`], never a concrete backend. The same binary
//! source selects nRF52840 USBD or RA4M1 USBFS; ESP chips use architecture-specific
//! examples in their port crates. The RA feature proves the exact config and link map;
//! complete UNO R4 clock/mux startup lives in the RA4M1 port executable.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

use nobro_usb::{try_mount, CdcState, UsbBackendError, UsbConfig, UsbStack, CDC_PACKET_SIZE};

#[cfg(all(feature = "backend-nrf-usbd", feature = "backend-ra-usbfs"))]
compile_error!("USB demo backend features are mutually exclusive");

#[cfg(feature = "backend-nrf-usbd")]
fn usb_config() -> UsbConfig {
    UsbConfig::new(
        0x1209,
        0x0001,
        "NiusRobotLab",
        "NobroRTOS Modular CDC",
        "NBRO-USB",
    )
}

#[cfg(feature = "backend-ra-usbfs")]
fn usb_config() -> UsbConfig {
    // The allocation-free RA backend has flash-resident descriptors. Passing its exact
    // identity keeps mount preflight fallible and guarantees no config-mismatch panic.
    nobro_usb::RA4M1_USB_CONFIG
}

struct PendingPacket {
    bytes: [u8; CDC_PACKET_SIZE],
    offset: usize,
    len: usize,
}

impl PendingPacket {
    const fn new() -> Self {
        Self {
            bytes: [0; CDC_PACKET_SIZE],
            offset: 0,
            len: 0,
        }
    }

    fn pending(&self) -> bool {
        self.offset < self.len
    }

    fn clear(&mut self) {
        self.offset = 0;
        self.len = 0;
    }

    fn queue(&mut self, bytes: &[u8]) -> bool {
        if self.pending() || bytes.len() > self.bytes.len() {
            return false;
        }
        self.bytes[..bytes.len()].copy_from_slice(bytes);
        self.offset = 0;
        self.len = bytes.len();
        true
    }

    fn service(&mut self, usb: &mut impl UsbStack) -> Result<(), UsbBackendError> {
        if !self.pending() {
            return Ok(());
        }
        let remaining = self.len - self.offset;
        let accepted = usb.try_write(&self.bytes[self.offset..self.len])?;
        if accepted > remaining {
            self.clear();
            return Err(UsbBackendError::InvalidState);
        }
        self.offset += accepted;
        if self.offset == self.len {
            self.clear();
        }
        Ok(())
    }
}

#[entry]
fn main() -> ! {
    let cfg = usb_config();
    let mut usb = match try_mount(&cfg) {
        Ok(usb) => usb,
        Err(_) => {
            defmt::error!("USB mount preflight failed");
            loop {
                cortex_m::asm::wfi();
            }
        }
    };
    let _ = usb.backend_id(); // which stack got mounted (for diagnostics)

    let mut greeted = false;
    let mut rx = [0u8; 64];
    let mut pending = PendingPacket::new();

    loop {
        if usb.poll() == CdcState::Configured {
            if pending.service(&mut usb).is_err() {
                pending.clear();
                greeted = false;
                defmt::warn!("USB write fault");
                continue;
            }
            if pending.pending() {
                continue;
            }
            if !greeted {
                let banner = b"NobroRTOS modular USB stack up.\r\n";
                let _ = pending.queue(banner);
                greeted = true;
                continue;
            }
            // Read only when the prior packet has drained, so backpressure never drops
            // or duplicates a prefix of the echo.
            let n = match usb.read_available(&mut rx) {
                Ok(count) => count,
                Err(_) => {
                    defmt::warn!("USB read unavailable");
                    continue;
                }
            };
            if n > 0 {
                let _ = pending.queue(&rx[..n]);
            }
        } else {
            greeted = false;
            pending.clear();
        }
    }
}
