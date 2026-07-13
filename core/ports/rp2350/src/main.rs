//! NobroRTOS portable core on the RP2350 / Pico 2 W with self-DFU autonomy.
//!
//! Runs the timebase provider and a bounded cross-core application over USB CDC.
//! Sending the line `DFU` over the same serial port reboots the chip into the BOOTSEL
//! UF2 bootloader, so the host can reflash without anyone touching the board.
#![no_std]
#![no_main]

use panic_halt as _;
use rp235x_hal as hal;

use hal::usb::UsbBus;
use usb_device::{class_prelude::*, prelude::*};
use usbd_serial::SerialPort;

use core::sync::atomic::{AtomicU32, Ordering};
use hal::multicore::{Multicore, Stack};

use nobro_kernel::{AsyncCore, MpmcChannel, ReactorExecutor};

mod portable;

/// RP2350 boot: the bootrom requires this image-definition block.
#[link_section = ".start_block"]
#[used]
pub static IMAGE_DEF: hal::block::ImageDef = hal::block::ImageDef::secure_exe();

const XTAL_FREQ_HZ: u32 = 12_000_000;

// Core 1 runs a bounded reactor rather than a heartbeat counter. It drains
// work items core0 sends over a cross-core `MpmcChannel`, computes a running
// multiply-accumulate, and publishes the live result. The channel is a bounded,
// critical-section-based cross-core transport, so a waker set on
// core1's `AsyncCore` from core0's send is observed across the core boundary).
static mut CORE1_STACK: Stack<4096> = Stack::new();
static CORE1_ACC: AtomicU32 = AtomicU32::new(0); // live result core0 reports
static CORE1_PROCESSED: AtomicU32 = AtomicU32::new(0);
static XCORE_WORK: MpmcChannel<u32, 4, 2> = MpmcChannel::new();
static CORE1_REACTOR: AsyncCore<1> = AsyncCore::new();

fn core1_task() {
    let mut exec = ReactorExecutor::bind(&CORE1_REACTOR);
    // The worker never completes (a service loop), so this stack-pinned future is
    // valid for the whole `-> !` lifetime of core1.
    let worker = core::pin::pin!(async {
        loop {
            let item = XCORE_WORK.recv().await; // parks when empty; core0 wakes it
            let Some(value) = item else { continue };
            // Real work: a saturating multiply-accumulate fusion step.
            let acc = CORE1_ACC
                .load(Ordering::Relaxed)
                .wrapping_add(value.wrapping_mul(3));
            CORE1_ACC.store(acc, Ordering::Relaxed);
            CORE1_PROCESSED.fetch_add(1, Ordering::Relaxed);
        }
    });
    exec.spawn(worker).ok();
    loop {
        exec.run_ready(8);
        cortex_m::asm::wfe(); // sleep until core0's send (or a timer) wakes us
    }
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

fn put_bytes(buf: &mut [u8], pos: &mut usize, bytes: &[u8]) {
    let room = buf.len().saturating_sub(*pos);
    let count = room.min(bytes.len());
    buf[*pos..*pos + count].copy_from_slice(&bytes[..count]);
    *pos += count;
}

#[hal::entry]
fn main() -> ! {
    let mut pac = hal::pac::Peripherals::take().unwrap();
    let mut watchdog = hal::Watchdog::new(pac.WATCHDOG);
    let clocks = hal::clocks::init_clocks_and_plls(
        XTAL_FREQ_HZ,
        pac.XOSC,
        pac.CLOCKS,
        pac.PLL_SYS,
        pac.PLL_USB,
        &mut pac.RESETS,
        &mut watchdog,
    )
    .unwrap();
    let timer = hal::Timer::new_timer0(pac.TIMER0, &mut pac.RESETS, &clocks);

    // Bring up core1 with its own stack and reactor task.
    let mut sio = hal::Sio::new(pac.SIO);
    let mut mc = Multicore::new(&mut pac.PSM, &mut pac.PPB, &mut sio.fifo);
    let core1 = &mut mc.cores()[1];
    #[allow(static_mut_refs)]
    let stack_alloc = unsafe { CORE1_STACK.take().unwrap() };
    let _ = core1.spawn(stack_alloc, core1_task);

    let usb_bus = UsbBusAllocator::new(UsbBus::new(
        pac.USB,
        pac.USB_DPRAM,
        clocks.usb_clock,
        true,
        &mut pac.RESETS,
    ));
    let mut serial = SerialPort::new(&usb_bus);
    let mut usb_dev = UsbDeviceBuilder::new(&usb_bus, UsbVidPid(0x2E8A, 0x000A))
        .strings(&[StringDescriptors::default()
            .manufacturer("NobroRTOS")
            .product("nobro-rp2350-selftest")
            .serial_number("NBRO2350")])
        .unwrap()
        .device_class(2) // CDC
        .build();

    let timebase_ok = portable::verify_timebase_provider();
    let all = timebase_ok;

    let mut line_buf = [0u8; 16];
    let mut line_len = 0usize;
    let mut last_report = timer.get_counter();

    loop {
        let _ = usb_dev.poll(&mut [&mut serial]);

        // Feed a work item to core1 over the cross-core channel each iteration
        // (non-blocking; the 4-slot ring backpressures if core1 falls behind),
        // then wake core1's reactor so it drains and computes.
        let mut feed: u32 = 1;
        if XCORE_WORK.try_send(feed).is_ok() {
            feed = feed.wrapping_add(1);
            cortex_m::asm::sev(); // wake core1's wfe
        }

        // heartbeat once a second
        let now = timer.get_counter();
        if (now - last_report).to_millis() >= 1000 {
            last_report = now;
            let mut msg = [0u8; 128];
            let mut pos = 0;
            put_bytes(
                &mut msg,
                &mut pos,
                b"NOBRO-RP2350 arch=thumbv8m providers=1 timebase=",
            );
            put_u32(&mut msg, &mut pos, u32::from(timebase_ok));
            put_bytes(&mut msg, &mut pos, b" all_pass=");
            put_u32(&mut msg, &mut pos, u32::from(all));
            // Report the LIVE cross-core reactor result: how many work items
            // core1 processed and its running accumulator.
            put_bytes(&mut msg, &mut pos, b" cores=2 core1_processed=");
            put_u32(&mut msg, &mut pos, CORE1_PROCESSED.load(Ordering::Relaxed));
            put_bytes(&mut msg, &mut pos, b" core1_acc=");
            put_u32(&mut msg, &mut pos, CORE1_ACC.load(Ordering::Relaxed));
            if pos + 2 <= msg.len() {
                msg[pos] = b'\r';
                msg[pos + 1] = b'\n';
                pos += 2;
            }
            let _ = serial.write(&msg[..pos]);
        }

        // self-DFU: the line "DFU" reboots into the BOOTSEL UF2 bootloader
        let mut rx = [0u8; 16];
        if let Ok(n) = serial.read(&mut rx) {
            for &c in &rx[..n] {
                if c == b'\n' || c == b'\r' {
                    if &line_buf[..line_len] == b"DFU" {
                        let _ = serial.write(b"rebooting to BOOTSEL\r\n");
                        // give the host a moment to drain the ack
                        let t0 = timer.get_counter();
                        while (timer.get_counter() - t0).to_millis() < 100 {
                            let _ = usb_dev.poll(&mut [&mut serial]);
                        }
                        hal::reboot::reboot(
                            hal::reboot::RebootKind::BootSel {
                                picoboot_disabled: false,
                                msd_disabled: false,
                            },
                            hal::reboot::RebootArch::Normal,
                        );
                    }
                    line_len = 0;
                } else if line_len < line_buf.len() {
                    line_buf[line_len] = c;
                    line_len += 1;
                }
            }
        }
    }
}
