//! USB Serial/JTAG backend example for ESP32-C3.
//!
//! Deliberately avoids esp-println: every byte on the wire goes through
//! `nobro_usb::mount()` -> `UsbSerialJtagCdc` raw-register writes, so seeing the
//! heartbeat on the host proves the mountable backend's data path. Sent bytes are
//! echoed back in brackets to prove the read path too.
#![no_std]
#![no_main]

use esp_hal::delay::Delay;
use nobro_usb::{CdcState, UsbConfig, UsbStack};

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

/// Append a decimal u32 to `buf` at `pos`, advancing `pos`.
fn put_u32(buf: &mut [u8], pos: &mut usize, mut v: u32) {
    let mut tmp = [0u8; 10];
    let mut n = 0;
    if v == 0 {
        tmp[0] = b'0';
        n = 1;
    }
    while v > 0 {
        tmp[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    while n > 0 && *pos < buf.len() {
        n -= 1;
        buf[*pos] = tmp[n];
        *pos += 1;
    }
}

/// Drain `data` through the non-blocking `UsbStack::write`, waiting out FIFO-busy
/// gaps (bounded so an unplugged host cannot wedge the loop).
fn write_all(usb: &mut impl UsbStack, delay: &Delay, data: &[u8]) {
    let mut off = 0;
    let mut spins = 0;
    while off < data.len() && spins < 500 {
        let n = usb.write(&data[off..]);
        off += n;
        if n == 0 {
            delay.delay_micros(200);
            spins += 1;
        }
    }
}

#[esp_hal::main]
fn main() -> ! {
    let _peripherals = esp_hal::init(esp_hal::Config::default());
    let delay = Delay::new();

    let cfg = UsbConfig::new(0x303A, 0x1001, "NiusRobotLab", "NobroRTOS USB-SJ", "NBROC3");
    let mut usb = nobro_usb::mount(&cfg);

    let mut beat: u32 = 0;
    loop {
        // ~1 s of fast polling so host RX is drained promptly between heartbeats
        for _ in 0..100 {
            let state = usb.poll();
            let mut rx = [0u8; 64];
            let n = usb.read(&mut rx);
            if n > 0 && state == CdcState::Configured {
                // UsbStack::write is non-blocking: after each 64-byte URB the IN FIFO
                // stays busy until the host fetches it, so retry the remainder.
                write_all(&mut usb, &delay, b"[echo:");
                write_all(&mut usb, &delay, &rx[..n]);
                write_all(&mut usb, &delay, b"]\r\n");
            }
            delay.delay_millis(10);
        }
        beat += 1;
        if usb.configured() {
            let mut msg = [0u8; 64];
            let head = b"NOBRO-USB-SJ backend=NUSJ configured=1 beat=";
            let mut pos = head.len();
            msg[..pos].copy_from_slice(head);
            put_u32(&mut msg, &mut pos, beat);
            if pos + 2 <= msg.len() {
                msg[pos] = b'\r';
                msg[pos + 1] = b'\n';
                pos += 2;
            }
            write_all(&mut usb, &delay, &msg[..pos]);
        }
    }
}
