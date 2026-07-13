//! Shared report + helpers for the radio TX/RX binaries.

/// "NRAD"
pub const MAGIC: u32 = 0x4E52_4144;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RadioReport {
    pub magic: u32,
    pub version: u32,
    pub role: u32, // 1 = TX, 2 = RX
    pub tx_sent: u32,
    pub rx_received: u32,
    pub last_seq: u32,
    pub all_pass: u32,
    pub checksum: u32,
}

impl RadioReport {
    pub const fn zero() -> Self {
        RadioReport {
            magic: 0,
            version: 0,
            role: 0,
            tx_sent: 0,
            rx_received: 0,
            last_seq: 0,
            all_pass: 0,
            checksum: 0,
        }
    }
}

/// Start the external 32 MHz crystal (HFXO) - required by the radio.
pub fn start_hfxo() {
    unsafe {
        core::ptr::write_volatile(0x4000_0000 as *mut u32, 1); // CLOCK.TASKS_HFCLKSTART
        while core::ptr::read_volatile(0x4000_0100 as *const u32) == 0 {} // EVENTS_HFCLKSTARTED
    }
}

pub fn checksum(r: &RadioReport) -> u32 {
    r.magic ^ r.version ^ r.role ^ r.tx_sent ^ r.rx_received ^ r.last_seq ^ r.all_pass
}
