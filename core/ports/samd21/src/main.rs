//! NobroRTOS portable core on the SAMD21 (M87) - Cortex-M0+, all drivers our own.
//!
//! Runs the shared cross-MCU conformance suite and reports over a SERCOM0 USART
//! (D0/D1 pads on Zero-class boards) written straight from the SAMD21 datasheet:
//! OSC8M at /1 (8 MHz), GCLK0 routed to SERCOM0, 115200 8N1 with the fractional
//! baud generator. Report line: `NOBRO-SAMD21 arch=thumbv6m subsystems=7 all_pass=1`.
//!
//! BUILD-VERIFIED ONLY: no SAMD board exists on the bench (owner's call to ship the
//! port anyway). Every register sequence below is datasheet-derived and unproven on
//! silicon - hardware bring-up notes stay in this header until a board arrives.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use panic_halt as _;

use nobro_conformance::run_all;

// ---------------------------------------------------------------- clocks (own driver)

const SYSCTRL_OSC8M: *mut u32 = 0x4000_0820 as *mut u32;
const GCLK_CLKCTRL: *mut u16 = 0x4000_0C02 as *mut u16;
const PM_APBCMASK: *mut u32 = 0x4000_0420 as *mut u32;

fn clocks_init() {
    unsafe {
        // OSC8M prescaler /8 -> /1: clear PRESC (bits 9:8), core now runs at 8 MHz.
        let v = SYSCTRL_OSC8M.read_volatile();
        SYSCTRL_OSC8M.write_volatile(v & !(0b11 << 8));
        // Feed GCLK0 (already on OSC8M after reset) to SERCOM0_CORE, and clock the bus.
        PM_APBCMASK.write_volatile(PM_APBCMASK.read_volatile() | (1 << 2)); // SERCOM0
        GCLK_CLKCTRL.write_volatile((1 << 14) | 0x14); // CLKEN, GCLK0 -> SERCOM0_CORE
    }
}

// ---------------------------------------------------------------- SERCOM0 USART

const SERCOM0: u32 = 0x4200_0800;
const CTRLA: *mut u32 = SERCOM0 as *mut u32;
const CTRLB: *mut u32 = (SERCOM0 + 0x04) as *mut u32;
const BAUD: *mut u16 = (SERCOM0 + 0x0C) as *mut u16;
const INTFLAG: *mut u8 = (SERCOM0 + 0x18) as *mut u8;
const SYNCBUSY: *mut u32 = (SERCOM0 + 0x1C) as *mut u32;
const DATA: *mut u16 = (SERCOM0 + 0x28) as *mut u16;

const PORT_A: u32 = 0x4100_4400;
const PMUX_BASE: *mut u8 = (PORT_A + 0x30) as *mut u8;
const PINCFG_BASE: *mut u8 = (PORT_A + 0x40) as *mut u8;

fn uart_init() {
    unsafe {
        // PA10 (TX, pad 2) / PA11 (RX, pad 3) -> peripheral function C (SERCOM0)
        PINCFG_BASE.add(10).write_volatile(1); // PMUXEN
        PINCFG_BASE.add(11).write_volatile(1);
        PMUX_BASE.add(5).write_volatile(0x22); // PA10 odd/even pair -> function C both

        // USART: internal clock (MODE=1), LSB first (DORD), TX on pad 2 (TXPO=1),
        // RX on pad 3 (RXPO=3), 16x arithmetic oversampling (SAMPR=0)
        CTRLA.write_volatile((1 << 30) | (0x3 << 20) | (0x1 << 24) | (0x1 << 2));
        // 115200 @ 8 MHz, 16x: BAUD = 65536 * (1 - 16*115200/8e6)
        BAUD.write_volatile(50437);
        CTRLB.write_volatile((1 << 16) | (1 << 17)); // TXEN | RXEN
        while SYNCBUSY.read_volatile() != 0 {}
        CTRLA.write_volatile(CTRLA.read_volatile() | (1 << 1)); // ENABLE
        while SYNCBUSY.read_volatile() != 0 {}
    }
}

fn uart_tx(b: u8) {
    unsafe {
        while INTFLAG.read_volatile() & 1 == 0 {} // DRE
        DATA.write_volatile(u16::from(b));
    }
}

fn print(s: &str) {
    for &b in s.as_bytes() {
        uart_tx(b);
    }
}

#[entry]
fn main() -> ! {
    clocks_init();
    uart_init();

    let results = run_all(); // the shared cross-MCU conformance suite (M92)
    let all = results.iter().all(|&r| r);

    loop {
        print(if all {
            "NOBRO-SAMD21 arch=thumbv6m subsystems=7 all_pass=1\r\n"
        } else {
            "NOBRO-SAMD21 arch=thumbv6m subsystems=7 all_pass=0\r\n"
        });
        cortex_m::asm::delay(8_000_000); // ~1 s at 8 MHz
    }
}
