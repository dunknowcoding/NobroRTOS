//! USB Serial/JTAG backend example for ESP32-C3.
//!
//! Deliberately avoids esp-println: every byte on the wire goes through
//! `nobro_usb::mount()` -> `UsbSerialJtagCdc` raw-register writes, so seeing the
//! heartbeat on the host proves the mountable backend's data path. Sent bytes are
//! echoed back in brackets to prove the read path too.
#![no_std]
#![no_main]

use esp_hal::delay::Delay;
use nobro_usb::{CdcState, UsbBackendError, UsbConfig, UsbStack};

const TX_CAPACITY: usize = 96;
const WRITE_RETRY_LIMIT: u16 = 500;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TxError {
    Timeout { remaining: usize },
    Fault(UsbBackendError),
    InvalidCount { remaining: usize, reported: usize },
}

struct PendingTx {
    bytes: [u8; TX_CAPACITY],
    offset: usize,
    len: usize,
}

impl PendingTx {
    const fn new() -> Self {
        Self {
            bytes: [0; TX_CAPACITY],
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

    fn queue_parts(&mut self, parts: &[&[u8]]) -> bool {
        if self.pending() {
            return false;
        }
        let Some(total) = parts
            .iter()
            .try_fold(0usize, |sum, part| sum.checked_add(part.len()))
        else {
            return false;
        };
        if total > self.bytes.len() {
            return false;
        }
        let mut end = 0;
        for part in parts {
            self.bytes[end..end + part.len()].copy_from_slice(part);
            end += part.len();
        }
        self.offset = 0;
        self.len = total;
        true
    }

    fn queue_echo(&mut self, payload: &[u8]) -> bool {
        // Prefix, payload, and suffix enter one retained transaction. Backpressure can
        // never abandon `]\r\n` after exposing only a prefix to the host.
        self.queue_parts(&[b"[echo:", payload, b"]\r\n"])
    }

    /// Make bounded progress while retaining every unaccepted suffix for the next call.
    fn service(&mut self, usb: &mut impl UsbStack, delay: &Delay) -> Result<(), TxError> {
        let mut retries = 0;
        while self.pending() {
            let remaining = self.len - self.offset;
            let reported = usb
                .try_write(&self.bytes[self.offset..self.len])
                .map_err(TxError::Fault)?;
            if reported > remaining {
                return Err(TxError::InvalidCount {
                    remaining,
                    reported,
                });
            }
            if reported == 0 {
                if retries == WRITE_RETRY_LIMIT {
                    return Err(TxError::Timeout { remaining });
                }
                retries += 1;
                if usb.poll() != CdcState::Configured {
                    return Err(TxError::Fault(UsbBackendError::Unavailable));
                }
                delay.delay_micros(200);
                continue;
            }
            self.offset += reported;
            retries = 0;
        }
        self.clear();
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TxDiagnostics {
    timeouts: u32,
    faults: u32,
    invalid_counts: u32,
}

impl TxDiagnostics {
    fn record(&mut self, error: TxError) {
        match error {
            TxError::Timeout { .. } => self.timeouts = self.timeouts.saturating_add(1),
            TxError::Fault(_) => self.faults = self.faults.saturating_add(1),
            TxError::InvalidCount { .. } => {
                self.invalid_counts = self.invalid_counts.saturating_add(1)
            }
        }
    }
}

#[esp_hal::main]
fn main() -> ! {
    let _peripherals = esp_hal::init(esp_hal::Config::default());
    let delay = Delay::new();

    // USB-Serial-JTAG owns its descriptors. This request is accepted for common API
    // uniformity but ignored; it is not the VID/PID/strings seen by the host.
    let cfg = UsbConfig::new(0x303A, 0x1001, "NiusRobotLab", "NobroRTOS USB-SJ", "NBROC3");
    let mut usb = match nobro_usb::try_mount(&cfg) {
        Ok(usb) => usb,
        Err(_) => loop {
            core::hint::spin_loop();
        },
    };

    let mut beat: u32 = 0;
    let mut tx = PendingTx::new();
    let mut tx_diagnostics = TxDiagnostics::default();
    loop {
        // ~1 s of fast polling so host RX is drained promptly between heartbeats
        for _ in 0..100 {
            let state = usb.poll();
            if state != CdcState::Configured {
                // A disconnect is an explicit session boundary: never leak an old echo
                // suffix into a newly enumerated host session.
                tx.clear();
                delay.delay_millis(10);
                continue;
            }
            if tx.pending() {
                if let Err(error) = tx.service(&mut usb, &delay) {
                    tx_diagnostics.record(error);
                }
                delay.delay_millis(10);
                continue;
            }
            let mut rx = [0u8; 64];
            let n = usb.read(&mut rx);
            if n > rx.len() {
                tx_diagnostics.record(TxError::InvalidCount {
                    remaining: rx.len(),
                    reported: n,
                });
            } else if n > 0 && tx.queue_echo(&rx[..n]) {
                if let Err(error) = tx.service(&mut usb, &delay) {
                    tx_diagnostics.record(error);
                }
            }
            delay.delay_millis(10);
        }
        beat += 1;
        if usb.configured() && !tx.pending() {
            let mut msg = [0u8; TX_CAPACITY];
            let head = b"NOBRO-USB-SJ backend=NUSJ configured=1 beat=";
            let mut pos = head.len();
            msg[..pos].copy_from_slice(head);
            put_u32(&mut msg, &mut pos, beat);
            let error_label = b" txerr=";
            if pos + error_label.len() <= msg.len() {
                msg[pos..pos + error_label.len()].copy_from_slice(error_label);
                pos += error_label.len();
                let total_errors = tx_diagnostics
                    .timeouts
                    .saturating_add(tx_diagnostics.faults)
                    .saturating_add(tx_diagnostics.invalid_counts);
                put_u32(&mut msg, &mut pos, total_errors);
            }
            if pos + 2 <= msg.len() {
                msg[pos] = b'\r';
                msg[pos + 1] = b'\n';
                pos += 2;
            }
            if tx.queue_parts(&[&msg[..pos]]) {
                if let Err(error) = tx.service(&mut usb, &delay) {
                    tx_diagnostics.record(error);
                }
            }
        }
    }
}
