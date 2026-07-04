//! NobroRTOS kernel-lite on the ATmega328P (M88): the first 8-bit port.
//!
//! 2 KB of RAM rules out the full portable core, so this runs a reduced but honest
//! subset of the same subsystem checks - quota ledger arithmetic, a mailbox ring,
//! watchdog-deadline bookkeeping, and the telemetry CRC - all in u16/u32 math the
//! AVR handles natively. Every driver here is our own: the UART is raw
//! UBRR0/UCSR0/UDR0 registers, the runtime is avr-gcc's crt calling `main`.
//! Report line (38400 8N1 on the board's USB bridge):
//!   `NOBRO-AVR arch=avr8 subsystems=4 all_pass=1`
#![no_std]
#![no_main]
#![feature(asm_experimental_arch)] // AVR inline asm (the delay nop) is nightly-gated

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {}
}

// ---------------------------------------------------------------- own UART driver

const UBRR0H: *mut u8 = 0xC5 as *mut u8;
const UBRR0L: *mut u8 = 0xC4 as *mut u8;
const UCSR0A: *mut u8 = 0xC0 as *mut u8;
const UCSR0B: *mut u8 = 0xC1 as *mut u8;
const UCSR0C: *mut u8 = 0xC2 as *mut u8;
const UDR0: *mut u8 = 0xC6 as *mut u8;

fn uart_init() {
    unsafe {
        // 38400 baud @ 16 MHz, no U2X: UBRR = 16e6/16/38400 - 1 = 25 (0.16% error)
        UBRR0H.write_volatile(0);
        UBRR0L.write_volatile(25);
        UCSR0A.write_volatile(0);
        UCSR0C.write_volatile(0b0000_0110); // 8N1
        UCSR0B.write_volatile(0b0000_1000); // TX enable
    }
}

fn uart_tx(b: u8) {
    unsafe {
        while UCSR0A.read_volatile() & (1 << 5) == 0 {} // UDRE0
        UDR0.write_volatile(b);
    }
}

fn print(s: &str) {
    for &b in s.as_bytes() {
        uart_tx(b);
    }
}

fn print_u8(mut v: u8) {
    let mut tmp = [0u8; 3];
    let mut n = 0;
    if v == 0 {
        tmp[0] = b'0';
        n = 1;
    }
    while v > 0 {
        tmp[n] = b'0' + v % 10;
        v /= 10;
        n += 1;
    }
    while n > 0 {
        n -= 1;
        uart_tx(tmp[n]);
    }
}

// ---------------------------------------------------------------- kernel-lite checks

/// Quota ledger arithmetic (the kernel's reserve/release invariant, u16-sized).
fn check_quota() -> bool {
    let budget: u16 = 1024;
    let mut used: u16 = 0;
    let reservations: [u16; 3] = [256, 512, 128];
    for r in reservations {
        if used + r > budget {
            return false;
        }
        used += r;
    }
    let over = used + 256 > budget; // 1152 > 1024 must be rejected
    used -= 512; // release
    over && used == 384 && used + 256 <= budget
}

/// Mailbox ring (the kernel's bounded queue) at an AVR-friendly capacity.
fn check_mailbox() -> bool {
    let mut ring = [0u8; 8];
    let (mut head, mut len) = (0usize, 0usize);
    for i in 0..10u8 {
        // push, dropping oldest when full (ring semantics)
        if len == ring.len() {
            head = (head + 1) % ring.len();
            len -= 1;
        }
        ring[(head + len) % ring.len()] = i;
        len += 1;
    }
    // oldest two (0,1) were dropped; front must be 2, back must be 9
    len == 8 && ring[head] == 2 && ring[(head + len - 1) % ring.len()] == 9
}

/// Watchdog deadline bookkeeping with wrapping timestamps.
fn check_watchdog() -> bool {
    let feed_interval: u16 = 500;
    let mut last_feed: u16 = 65_400; // near wrap
    let now: u16 = 100; // wrapped past 65535
    let elapsed = now.wrapping_sub(last_feed); // 236 ticks
    let ok_before = elapsed < feed_interval;
    last_feed = now;
    let starved = 700u16.wrapping_sub(last_feed) >= feed_interval; // 600 >= 500
    ok_before && starved
}

/// Telemetry frame CRC-8 (poly 0x07) - matches the collector's framing check.
fn crc8(data: &[u8]) -> u8 {
    let mut crc: u8 = 0;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x07 } else { crc << 1 };
        }
    }
    crc
}

fn check_crc() -> bool {
    // reference vector: crc8(0x07) over "123456789" = 0xF4
    crc8(b"123456789") == 0xF4 && crc8(b"") == 0 && crc8(b"nobro") != 0
}

#[no_mangle]
pub extern "C" fn main() -> ! {
    uart_init();

    let results = [check_quota(), check_mailbox(), check_watchdog(), check_crc()];
    let all = results.iter().all(|&r| r);

    loop {
        print("NOBRO-AVR arch=avr8 subsystems=");
        print_u8(results.len() as u8);
        print(" all_pass=");
        print_u8(u8::from(all));
        print("\r\n");
        // ~1 s at 16 MHz
        for _ in 0..8u8 {
            for _ in 0..65_000u16 {
                unsafe { core::arch::asm!("nop") };
            }
        }
    }
}
