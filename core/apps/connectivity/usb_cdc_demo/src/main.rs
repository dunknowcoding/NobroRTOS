//! USB-CDC diagnostics demo: bring up the IMU and stream its health summary over a USB
//! serial port. This lets boards without a debug probe be verified by
//! opening a COM port - no debug probe or RTT needed. The USB stack is no_std /
//! no-alloc and lives entirely in this app; the kernel is not involved.
#![no_std]
#![no_main]

use cortex_m_rt::{entry, pre_init};
use defmt_rtt as _; // provides defmt.x linker section + global logger
use panic_halt as _;

use nobro_adapter_mpu9250_imu::Mpu9250Imu;
#[cfg(feature = "board-nicenano-s140")]
use nobro_hal::traits::HalClock;
use nobro_hal::{
    lease::Resource,
    traits::{HalLease, HalTimebaseProvider},
    ActivePlatform as Hal,
};
use nobro_kernel::{pool::SamplePool, CompactImuPayload};
use nobro_sal::SensorSal;
use nobro_usb::{CdcState, MountedUsb, UsbConfig, UsbStack};

const OWNER_TWIM: u8 = 3;

const SCB_VTOR: *mut u32 = 0xE000_ED08 as *mut u32;
const SCB_ICSR: *mut u32 = 0xE000_ED04 as *mut u32;
const SYST_CSR: *mut u32 = 0xE000_E010 as *mut u32;
const SYST_CVR: *mut u32 = 0xE000_E018 as *mut u32;
const NVIC_ICER: *mut u32 = 0xE000_E180 as *mut u32;
const NVIC_ICPR: *mut u32 = 0xE000_E280 as *mut u32;
const ICSR_PENDSTCLR: u32 = 1 << 25;
const ICSR_PENDSVCLR: u32 = 1 << 27;

extern "C" {
    static __vector_table: u32;
}

/// Install this image's vector table before sanitizing interrupt state inherited
/// from a firmware-stage handoff.
///
/// Some nRF bootloaders branch to the application instead of issuing a complete
/// core reset. This is intentionally narrower than a reset emulation: it masks
/// interrupts, selects the linker-provided vector table, clears inherited SysTick
/// and external-IRQ state, then leaves interrupt delivery masked until `main`.
/// Keep this pre-RAM hook register-only; no Rust static data is initialized yet.
#[pre_init]
unsafe fn sanitize_bootloader_interrupt_handoff() {
    cortex_m::interrupt::disable();
    SCB_VTOR.write_volatile(core::ptr::addr_of!(__vector_table) as u32);
    cortex_m::asm::dsb();
    cortex_m::asm::isb();
    for bank in 0..2 {
        NVIC_ICER.add(bank).write_volatile(u32::MAX);
        NVIC_ICPR.add(bank).write_volatile(u32::MAX);
    }
    SYST_CSR.write_volatile(0);
    SYST_CVR.write_volatile(0);
    SCB_ICSR.write_volatile(ICSR_PENDSTCLR | ICSR_PENDSVCLR);
    cortex_m::asm::dsb();
    cortex_m::asm::isb();
}

#[inline(never)]
fn push(buf: &mut [u8], pos: &mut usize, s: &[u8]) {
    for &b in s {
        if *pos < buf.len() {
            buf[*pos] = b;
            *pos += 1;
        }
    }
}

#[inline(never)]
fn push_u32(buf: &mut [u8], pos: &mut usize, mut v: u32) {
    let mut tmp = [0u8; 10];
    let mut n = 0;
    if v == 0 {
        push(buf, pos, b"0");
        return;
    }
    while v > 0 {
        tmp[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    while n > 0 {
        n -= 1;
        if *pos < buf.len() {
            buf[*pos] = tmp[n];
            *pos += 1;
        }
    }
}

fn device_serial(words: [u32; 2]) -> [u8; 16] {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut serial = [0u8; 16];
    for (word_index, word) in words.into_iter().rev().enumerate() {
        for nibble in 0..8 {
            let shift = 28 - nibble * 4;
            serial[word_index * 8 + nibble] = HEX[((word >> shift) & 0xF) as usize];
        }
    }
    serial
}

static mut USB_SERIAL: [u8; 16] = [b'0'; 16];

fn install_device_serial(words: [u32; 2]) -> &'static str {
    let serial = device_serial(words);
    unsafe {
        let destination = core::ptr::addr_of_mut!(USB_SERIAL).cast::<u8>();
        core::ptr::copy_nonoverlapping(serial.as_ptr(), destination, serial.len());
        let bytes = core::slice::from_raw_parts(destination.cast_const(), serial.len());
        core::str::from_utf8_unchecked(bytes)
    }
}

#[inline(never)]
fn write_line(usb: &mut MountedUsb, line: &[u8]) -> bool {
    for packet in line.chunks(nobro_usb::CDC_PACKET_SIZE) {
        if usb.write_all(packet).is_err() {
            return false;
        }
    }
    true
}

#[inline(never)]
fn read_dfu_command(usb: &mut MountedUsb, dfu_command_pos: &mut u8) -> bool {
    let mut command_bytes = [0u8; 16];
    if let Ok(count) = usb.try_read(&mut command_bytes) {
        for &byte in &command_bytes[..count] {
            *dfu_command_pos = match (*dfu_command_pos, byte) {
                (0, b'D') => 1,
                (1, b'F') => 2,
                (2, b'U') => 3,
                (3, b'\r') => 3,
                (3, b'\n') => return true,
                (_, b'\n') => 0,
                _ => 0,
            };
        }
    }
    false
}

#[inline(never)]
fn write_human_report(
    usb: &mut MountedUsb,
    who: u32,
    addr: u32,
    i2c_ok: u32,
    reads: u32,
    errors: u32,
    accel_mg: u32,
    temp_centi_c: u32,
    gyro_mag_mdps: u32,
    pass: bool,
) -> bool {
    let mut buf = [0u8; 128];
    let mut n = 0usize;
    push(&mut buf, &mut n, b"NobroRTOS IMU who=0x");
    let hi = (who >> 4) & 0xF;
    let lo = who & 0xF;
    let hexd = |d: u32| {
        if d < 10 {
            b'0' + d as u8
        } else {
            b'a' + (d - 10) as u8
        }
    };
    if n + 2 <= buf.len() {
        buf[n] = hexd(hi);
        buf[n + 1] = hexd(lo);
        n += 2;
    }
    push(&mut buf, &mut n, b" addr=");
    push_u32(&mut buf, &mut n, addr);
    push(&mut buf, &mut n, b" i2c=");
    push_u32(&mut buf, &mut n, i2c_ok);
    push(&mut buf, &mut n, b" reads=");
    push_u32(&mut buf, &mut n, reads);
    push(&mut buf, &mut n, b" err=");
    push_u32(&mut buf, &mut n, errors);
    push(&mut buf, &mut n, b" accel=");
    push_u32(&mut buf, &mut n, accel_mg);
    push(&mut buf, &mut n, b"mg temp=");
    push_u32(&mut buf, &mut n, temp_centi_c);
    push(&mut buf, &mut n, b" gyro=");
    push_u32(&mut buf, &mut n, gyro_mag_mdps);
    push(&mut buf, &mut n, b"mdps ");
    push(&mut buf, &mut n, if pass { b"PASS\r\n" } else { b"..\r\n" });
    write_line(usb, &buf[..n])
}

#[inline(never)]
fn write_machine_report(
    usb: &mut MountedUsb,
    who: u32,
    reads: u32,
    errors: u32,
    accel_mg: u32,
    pass: bool,
) -> bool {
    let mut mline = [0u8; 96];
    let mut m = 0usize;
    push(&mut mline, &mut m, b"NOBRO-CDC who=");
    push_u32(&mut mline, &mut m, who);
    push(&mut mline, &mut m, b" reads=");
    push_u32(&mut mline, &mut m, reads);
    push(&mut mline, &mut m, b" errors=");
    push_u32(&mut mline, &mut m, errors);
    push(&mut mline, &mut m, b" accel_mg=");
    push_u32(&mut mline, &mut m, accel_mg);
    push(&mut mline, &mut m, b" all_pass=");
    push_u32(&mut mline, &mut m, u32::from(pass));
    push(&mut mline, &mut m, b"\r\n");
    write_line(usb, &mline[..m])
}

#[cfg(feature = "board-nicenano-s140")]
const USB_DFU_DETACH_US: u64 = 20_000;

#[cfg(feature = "board-nicenano-s140")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum DfuHandoff {
    Idle,
    Quiescing,
    Detached { started_us: u64 },
}

#[cfg(feature = "board-nicenano-s140")]
fn interval_elapsed(now_us: u64, started_us: u64, interval_us: u64) -> bool {
    now_us.wrapping_sub(started_us) >= interval_us
}

#[cfg(feature = "board-nicenano-s140")]
fn enter_uf2_bootloader_after_detach() -> ! {
    const POWER_GPREGRET: *mut u32 = 0x4000_051C as *mut u32;
    const UF2_DFU_MAGIC: u32 = 0x57;

    // The caller has already detached through nobro-usb and held that lifecycle
    // state without polling for USB_DFU_DETACH_US. Never write USBD.ENABLE or
    // USBPULLUP here: raw teardown would bypass the driver's EasyDMA-parity and
    // revision-specific errata ownership.
    unsafe {
        core::ptr::write_volatile(POWER_GPREGRET, UF2_DFU_MAGIC);
    }
    cortex_m::asm::dsb();
    cortex_m::peripheral::SCB::sys_reset();
}

#[entry]
fn main() -> ! {
    // The pre-RAM handoff sanitizer deliberately leaves PRIMASK set. Re-enable only
    // now, after cortex-m-rt has initialized .data/.bss and the inherited pending
    // interrupt state has been cleared.
    unsafe {
        cortex_m::interrupt::enable();
    }
    let periph = nrf52840_pac::Peripherals::take().unwrap();

    // LED progress indicator so a probe-less board shows how far USB got.
    unsafe {
        nobro_hal::ppi::led_init_output();
        nobro_hal::ppi::led_toggle();
    }

    // The lifecycle uses this 1 MHz clock for bounded detach/enable transitions.
    Hal::acquire(Resource::Timer0, 2).unwrap_or_else(|_| defmt::panic!("timer lease"));
    unsafe {
        Hal::init_timebase();
    }

    // A per-MCU serial prevents Windows from merging identical CDC identities when
    // several boards run this image at the same time.
    let serial_id = install_device_serial([
        periph.FICR.deviceid[0].read().bits(),
        periph.FICR.deviceid[1].read().bits(),
    ]);
    // Keep hardware bring-up inside nobro-usb. Demos must not recreate raw CLOCK,
    // POWER, trim, or USBD sequences: doing so previously bypassed OUTPUTRDY and the
    // 64-byte EP0 contract, so the demo could diverge from the stack it was meant to test.
    let config = UsbConfig::new(0x1209, 0x0001, "NiusRobotLab", "NobroRTOS CDC", serial_id);
    let mut usb = nobro_usb::mount(&config);
    // Start the non-blocking inherited-session cleanup before optional sensor work.
    // If an adapter stalls or is absent, it cannot prevent the stack from claiming
    // the controller and dropping a bootloader-owned pull-up first.
    let _ = usb.poll();

    Hal::acquire(Resource::Twim0, OWNER_TWIM).unwrap_or_else(|_| defmt::panic!("I2C lease"));
    let imu = Mpu9250Imu::probe_and_init(OWNER_TWIM);
    let (who, addr, i2c_ok) = match &imu {
        Ok(d) => (u32::from(d.who_am_i()), u32::from(d.addr()), 1u32),
        Err(_) => (0, 0, 0),
    };
    let mut imu = imu.ok();

    let mut reads: u32 = 0;
    let mut errors: u32 = 0;
    let mut accel_mg: u32 = 0;
    let mut spin: u32 = 0;

    let mut blink: u32 = 0;
    #[cfg(feature = "board-nicenano-s140")]
    let usb_start = Hal::now_us();
    #[cfg(feature = "board-nicenano-s140")]
    let mut last_usb_recovery = usb_start;
    #[cfg(feature = "board-nicenano-s140")]
    let mut ever_configured = false;
    let mut max_state: u8 = 0; // 0=Default, 1=Addressed, 2=Configured (max reached)
    let mut dfu_command_pos: u8 = 0;
    #[cfg(feature = "board-nicenano-s140")]
    let mut dfu_handoff = DfuHandoff::Idle;
    loop {
        #[cfg(feature = "board-nicenano-s140")]
        match dfu_handoff {
            DfuHandoff::Detached { started_us } => {
                if interval_elapsed(Hal::now_us(), started_us, USB_DFU_DETACH_US) {
                    enter_uf2_bootloader_after_detach();
                }
                // The one-way handoff has already repaired EasyDMA parity, proved
                // ENABLE=Disabled, and released errata ownership. Deliberately do not
                // poll or perform USB I/O during this host-visible disconnect dwell.
                continue;
            }
            DfuHandoff::Quiescing => {
                match usb.poll_bootloader_handoff() {
                    Ok(true) => {
                        dfu_handoff = DfuHandoff::Detached {
                            started_us: Hal::now_us(),
                        };
                    }
                    Ok(false) | Err(_) => {
                        // Never reset across an uncertain controller state. A real
                        // terminal driver fault remains fail-closed for probe recovery.
                        dfu_handoff = DfuHandoff::Quiescing;
                    }
                }
                continue;
            }
            DfuHandoff::Idle => {}
        }

        let usb_state = usb.poll();
        blink = blink.wrapping_add(1);
        let s = match usb_state {
            CdcState::Addressed => 1u8,
            CdcState::Configured => 2u8,
            _ => 0u8,
        };
        if s > max_state {
            max_state = s;
        }
        let configured = s >= 2;
        #[cfg(feature = "board-nicenano-s140")]
        if configured {
            ever_configured = true;
        }
        // If a host abandons the initial control transfer and leaves the controller
        // suspended with VBUS present, retry the application identity in place. Keep
        // this rate-limited: an ordinary configured-device suspend is valid and must
        // not be turned into a reconnect loop. Explicit "DFU" remains the only path
        // that changes to the bootloader identity.
        #[cfg(feature = "board-nicenano-s140")]
        if !ever_configured {
            const USB_RECOVERY_INTERVAL_US: u64 = 5_000_000;
            let now = Hal::now_us();
            if interval_elapsed(now, last_usb_recovery, USB_RECOVERY_INTERVAL_US) {
                let _ = usb.force_reenumeration();
                last_usb_recovery = now;
            }
        }
        // Fast flicker once Configured (USB working); slow blink while enumeration is
        // stuck. Lets a probe-less board report its USB state on the LED.
        // LED rate reports how far USB enumeration got: SLOW (~1 Hz) = stuck at Default
        // (device descriptor / first control transfer fails); MEDIUM (~5 Hz) = reached
        // Addressed (SET_ADDRESS ok, config stage fails); FAST flicker = Configured.
        let period = match max_state {
            2 => 20_000,
            1 => 150_000,
            _ => 700_000,
        };
        if blink % period == 0 {
            unsafe {
                nobro_hal::ppi::led_toggle();
            }
        }
        if !configured {
            continue;
        }

        // A line containing only "DFU" enters the UF2 bootloader. This keeps future
        // update cycles host-driven even on boards without a debug probe.
        if read_dfu_command(&mut usb, &mut dfu_command_pos) {
            #[cfg(feature = "board-nicenano-s140")]
            {
                dfu_handoff = DfuHandoff::Quiescing;
            }
            dfu_command_pos = 0;
        }
        #[cfg(feature = "board-nicenano-s140")]
        if dfu_handoff != DfuHandoff::Idle {
            // Attempt the lifecycle-owned teardown at the top of the next iteration.
            // In particular, do not let write_line()/write_all() poll the backend
            // after accepting the command.
            continue;
        }

        // Sample the IMU occasionally; keep usb.poll() the hot path for USB timing.
        spin = spin.wrapping_add(1);
        if spin % 4096 == 0 {
            if let Some(d) = imu.as_mut() {
                match d.poll() {
                    Ok(Some(sample)) => {
                        reads += 1;
                        if let Some(p) = CompactImuPayload::read_from_handle(sample.handle) {
                            accel_mg = p.into_sample(sample.captured_us).accel_mag_mg;
                        }
                        SamplePool::release(sample.handle);
                    }
                    Ok(None) => {}
                    Err(_) => errors += 1,
                }
            }
        }

        if spin % 600_000 == 0 {
            let pass = i2c_ok == 1 && reads >= 10 && (800..1200).contains(&accel_mg);
            let temp_centi_c = imu.as_ref().map(|d| d.last_temp_centi_c()).unwrap_or(0);
            let gyro_mag_mdps = imu.as_ref().map(|d| d.last_gyro_mag_mdps()).unwrap_or(0);
            if !write_human_report(
                &mut usb,
                who,
                addr,
                i2c_ok,
                reads,
                errors,
                accel_mg,
                temp_centi_c,
                gyro_mag_mdps,
                pass,
            ) {
                defmt::warn!("USB telemetry backpressure");
            }

            // Machine-decodable twin of the line above, in the standard
            // `NOBRO-<NAME> key=value` shape the host tools and the web-flasher
            // report console parse (nobro_rtos.node / parseStatusLine).
            if !write_machine_report(&mut usb, who, reads, errors, accel_mg, pass) {
                defmt::warn!("USB model-report backpressure");
            }
        }
    }
}
