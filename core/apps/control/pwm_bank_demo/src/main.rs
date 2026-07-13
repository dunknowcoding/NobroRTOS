//! PWM bank under leases (M150): Resource::Pwm0 gates a four-channel 50 Hz servo/ESC
//! bank; a second owner is rejected; four distinct pulses are set and read back from
//! the live SEQ RAM the hardware fetches. NOBRO_PWM_BANK_REPORT (mem32).
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

use nobro_hal::{lease::LeaseError, PwmBankSession};

#[repr(C)]
#[derive(Clone, Copy)]
struct Report {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    lease_ok: u32,
    readback_ok: u32,
    freq_hz: u32,
    checksum: u32,
}
const MAGIC: u32 = 0x4E50_5742; // "NPWB"

#[no_mangle]
#[used]
static mut NOBRO_PWM_BANK_REPORT: Report = Report {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    lease_ok: 0,
    readback_ok: 0,
    freq_hz: 0,
    checksum: 0,
};

const OWNER_A: u8 = 11;
const OWNER_B: u8 = 12;
const PULSES: [u32; 4] = [1000, 1250, 1500, 2000];

#[entry]
fn main() -> ! {
    // The bank is a managed resource: exclusive lease, conflicts rejected.
    let mut bank =
        unsafe { PwmBankSession::acquire(OWNER_A, [Some(24), Some(2), Some(29), Some(31)], 1500) }
            .unwrap_or_else(|_| defmt::panic!("PWM session"));
    let acquired = true;
    let conflict = matches!(
        unsafe { PwmBankSession::acquire(OWNER_B, [None, None, None, None], 1500) },
        Err(LeaseError::AlreadyHeld)
    );
    let lease_ok = acquired && conflict;

    // Ch0 on the servo header (P0.24); ch1..3 on spare analog pins (P0.02/P0.29/P0.31).
    for (ch, p) in PULSES.iter().enumerate() {
        bank.set_pulse_us(ch, *p)
            .unwrap_or_else(|_| defmt::panic!("stale PWM session"));
    }
    cortex_m::asm::delay(3_200_000); // let a couple of 20 ms periods elapse

    let mut readback_ok = 1u32;
    for (ch, p) in PULSES.iter().enumerate() {
        if bank.read_pulse_us(ch).unwrap_or(0) != *p {
            readback_ok = 0;
        }
    }
    let freq_hz = bank.frequency_hz().unwrap_or(0);
    drop(bank);
    let released =
        unsafe { PwmBankSession::acquire(OWNER_B, [None, None, None, None], 1500) }.is_ok();

    let pass = lease_ok && readback_ok == 1 && freq_hz == 50 && released;
    let (ap, lo) = (u32::from(pass), u32::from(lease_ok));
    let cs = MAGIC ^ 1 ^ 1 ^ ap ^ lo ^ readback_ok ^ freq_hz;
    unsafe {
        NOBRO_PWM_BANK_REPORT = Report {
            magic: MAGIC,
            version: 1,
            completed: 1,
            all_pass: ap,
            lease_ok: lo,
            readback_ok,
            freq_hz,
            checksum: cs,
        };
    }

    loop {
        cortex_m::asm::delay(16_000_000);
    }
}
