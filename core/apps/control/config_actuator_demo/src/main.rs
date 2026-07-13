//! Config-driven actuator bring-up on real hardware. No servo-specific code: the
//! app picks a servo BRAND from the nobro-device catalog (SG90), asks the shared
//! `angle_to_pulse` driver for the pulse at each angle, drives it on the leased PwmBank
//!, and reads the live pulse back. Swapping the brand is a one-line catalog change.
//! Proves the data-first device-module framework actuates real hardware.
//! NOBRO_CFG_ACTUATOR_REPORT.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

use nobro_device::catalog::SERVO_SG90;
use nobro_hal::PwmBankSession;

#[repr(C)]
#[derive(Clone, Copy)]
struct Report {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    pulse_min: u32,   // angle_to_pulse(0)
    pulse_mid: u32,   // angle_to_pulse(90)
    pulse_max: u32,   // angle_to_pulse(180)
    readback_ok: u32, // every commanded pulse read back exactly
}
const MAGIC: u32 = 0x4E43_4641; // "NCFA"

#[no_mangle]
#[used]
static mut NOBRO_CFG_ACTUATOR_REPORT: Report = Report {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    pulse_min: 0,
    pulse_mid: 0,
    pulse_max: 0,
    readback_ok: 0,
};

const OWNER: u8 = 11;

#[entry]
fn main() -> ! {
    let servo = SERVO_SG90; // <- change this one const to support a different brand

    // Compute the pulses purely from the profile (no servo-specific logic here).
    let angles = [0i16, 45, 90, 135, 180];
    let pulse_min = u32::from(servo.angle_to_pulse(0));
    let pulse_mid = u32::from(servo.angle_to_pulse(90));
    let pulse_max = u32::from(servo.angle_to_pulse(180));

    // Drive channel 0 of the leased PWM bank at each angle and read the live pulse back.
    let mut bank = unsafe {
        PwmBankSession::acquire(
            OWNER,
            [Some(24), None, None, None],
            u32::from(servo.center_us),
        )
    }
    .unwrap_or_else(|_| defmt::panic!("PWM session"));

    let mut readback_ok = 1u32;
    for &a in &angles {
        let want = servo.angle_to_pulse(a);
        bank.set_pulse_us(0, u32::from(want))
            .unwrap_or_else(|_| defmt::panic!("stale PWM session"));
        cortex_m::asm::delay(1_600_000); // let a period elapse
        if bank.read_pulse_us(0).unwrap_or(0) != u32::from(want) {
            readback_ok = 0;
        }
    }
    let freq_ok = bank.frequency_hz().unwrap_or(0) == 50;

    // SG90 spec sanity: 0deg=500us, 90deg~1450us, 180deg=2400us.
    let pass = readback_ok == 1
        && freq_ok
        && pulse_min == 500
        && pulse_max == 2400
        && (1_400..=1_500).contains(&pulse_mid);
    let ap = u32::from(pass);
    unsafe {
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!(NOBRO_CFG_ACTUATOR_REPORT),
            Report {
                magic: MAGIC,
                version: 1,
                completed: 1,
                all_pass: ap,
                pulse_min,
                pulse_mid,
                pulse_max,
                readback_ok,
            },
        );
    }

    loop {
        cortex_m::asm::delay(16_000_000);
    }
}
