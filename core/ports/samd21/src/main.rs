//! NobroRTOS portable core on the SAMD21 - Cortex-M0+, all drivers our own.
//!
//! Provides target startup and status over a SERCOM0 USART
//! (D0/D1 pads on Zero-class boards) written straight from the SAMD21 datasheet:
//! OSC8M at /1 (8 MHz), GCLK0 routed to SERCOM0, 115200 8N1 with the fractional
//! baud generator. The `NOBRO-SAMD21` report includes `port_ready` plus the
//! measured PRIMASK maximum, bound, wrap state, and pass state.
//!
//! This port currently provides the portable core and serial status path; it does not
//! claim portable peripheral-provider coverage.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use panic_halt as _;

mod masked_critical_section;

const CORE_HZ: u32 = 8_000_000;
const MASK_BOUND_US: u32 = 25;

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

fn print_u32(mut value: u32) {
    let mut digits = [0_u8; 10];
    let mut used = 0;
    loop {
        digits[used] = b'0' + (value % 10) as u8;
        used += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    while used != 0 {
        used -= 1;
        uart_tx(digits[used]);
    }
}

#[entry]
fn main() -> ! {
    clocks_init();
    uart_init();

    masked_critical_section::init();
    critical_section::with(|_| {
        // Exercise nesting so the report covers the exact provider selected by
        // this image rather than an unreferenced measurement helper.
        critical_section::with(|_| core::hint::spin_loop());
    });

    loop {
        let max_cycles = masked_critical_section::max_masked_cycles();
        let max_us = masked_critical_section::max_masked_us_ceil(CORE_HZ);
        let pass = masked_critical_section::within_us(CORE_HZ, MASK_BOUND_US);
        print("NOBRO-SAMD21 arch=thumbv6m port_ready=1 mask_max_cycles=");
        print_u32(max_cycles);
        print(" mask_max_us=");
        print_u32(max_us);
        print(" mask_bound_us=");
        print_u32(MASK_BOUND_US);
        print(" mask_wrapped=");
        print_u32(masked_critical_section::counter_wrapped() as u32);
        print(" mask_pass=");
        print_u32(pass as u32);
        print("\r\n");
        cortex_m::asm::delay(8_000_000); // ~1 s at 8 MHz
    }
}
