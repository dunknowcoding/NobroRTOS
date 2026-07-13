//! Watchdog recovery on real hardware (M70): on first boot, arm the nRF52840 WDT and stop
//! feeding it so it actually resets the chip; on the reboot, detect the watchdog reset
//! cause (RESETREAS.DOG) and self-certify the recovery. The flash script clears RESETREAS
//! first, so the DOG bit can only come from our own watchdog. Reports via NOBRO_WDT_REPORT
//! (J-Link mem32).
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

#[repr(C)]
#[derive(Clone, Copy)]
struct Report {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    reset_reason: u32,
    recovered: u32,
    wdt_crv: u32,
    checksum: u32,
}
const MAGIC: u32 = 0x4E57_4447; // "NWDG"

#[no_mangle]
#[used]
static mut NOBRO_WDT_REPORT: Report = Report {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    reset_reason: 0,
    recovered: 0,
    wdt_crv: 0,
    checksum: 0,
};

const RESETREAS: u32 = 0x4000_0400;
const WDT: u32 = 0x4001_0000;
const DOG: u32 = 1 << 1; // RESETREAS watchdog bit
const CRV: u32 = 16_383; // ~0.5 s at 32.768 kHz

unsafe fn rd(a: u32) -> u32 {
    core::ptr::read_volatile(a as *const u32)
}
unsafe fn wr(a: u32, v: u32) {
    core::ptr::write_volatile(a as *mut u32, v);
}

#[entry]
fn main() -> ! {
    unsafe {
        let reason = rd(RESETREAS);
        wr(RESETREAS, reason); // write-1-to-clear the latched bits
        let dog = reason & DOG != 0;

        if dog {
            // Recovered from our own watchdog reset (RESETREAS was cleared at flash time,
            // so DOG can only be our WDT). Do NOT re-arm -> no reset loop.
            let cs = MAGIC ^ 1 ^ 1 ^ 1 ^ reason ^ 1 ^ CRV;
            NOBRO_WDT_REPORT = Report {
                magic: MAGIC,
                version: 1,
                completed: 1,
                all_pass: 1,
                reset_reason: reason,
                recovered: 1,
                wdt_crv: CRV,
                checksum: cs,
            };
            loop {
                cortex_m::asm::delay(16_000_000);
            }
        }

        // First boot: arm the WDT and stop feeding it so it resets the chip.
        NOBRO_WDT_REPORT = Report {
            magic: MAGIC,
            version: 1,
            completed: 0,
            all_pass: 0,
            reset_reason: reason,
            recovered: 0,
            wdt_crv: CRV,
            checksum: 0,
        };
        wr(WDT + 0x504, CRV); // CRV: ~0.5 s
        wr(WDT + 0x508, 1); // RREN: enable reload register 0
        wr(WDT + 0x50C, 1); // CONFIG: run while CPU sleeps
        wr(WDT + 0x000, 1); // TASKS_START
        loop {
            cortex_m::asm::nop(); // never feed -> WDT resets the chip
        }
    }
}
