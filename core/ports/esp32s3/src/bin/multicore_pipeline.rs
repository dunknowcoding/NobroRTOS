//! Bounded dual-core audio/camera/AI pipeline for the ESP32-S3.

#![no_std]
#![no_main]

use core::{
    fmt::Write,
    sync::atomic::{AtomicU32, Ordering},
};

use esp_hal::{
    cpu_control::{CpuControl, Stack},
    usb_serial_jtag::UsbSerialJtag,
};
use nobro_hal::HalClock;
use nobro_kernel::MpmcChannel;
use nobro_port_esp32s3::providers::{Esp32S3Clock, Esp32S3Usb};

const AUDIO_ITEMS: u32 = 64;
const CAMERA_ITEMS: u32 = 16;
const TOTAL_ITEMS: u32 = AUDIO_ITEMS + CAMERA_ITEMS;
const QUEUE_CAPACITY: usize = 8;
const AUDIO_BYTES: u32 = 320;
const CAMERA_BYTES: u32 = 12_000;
const MODEL_ARENA_BYTES: u32 = 24_576;
const MAX_PIPELINE_BYTES: u32 =
    QUEUE_CAPACITY as u32 * CAMERA_BYTES + MODEL_ARENA_BYTES + CAMERA_BYTES;

#[derive(Clone, Copy)]
struct WorkItem {
    kind: u8,
    sequence: u16,
    bytes: u32,
    produced_us: u64,
    deadline_us: u64,
    checksum: u32,
}

static WORK: MpmcChannel<WorkItem, QUEUE_CAPACITY, 2> = MpmcChannel::new();
static PROCESSED: AtomicU32 = AtomicU32::new(0);
static AUDIO_PROCESSED: AtomicU32 = AtomicU32::new(0);
static CAMERA_PROCESSED: AtomicU32 = AtomicU32::new(0);
static DEADLINE_MISSES: AtomicU32 = AtomicU32::new(0);
static BACKPRESSURE: AtomicU32 = AtomicU32::new(0);
static CHECKSUM: AtomicU32 = AtomicU32::new(0);
static MAX_LATENCY_US: AtomicU32 = AtomicU32::new(0);
static mut APP_CORE_STACK: Stack<8192> = Stack::new();

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

fn payload_checksum(kind: u8, sequence: u16, bytes: u32) -> u32 {
    (u32::from(kind) << 24) ^ (u32::from(sequence) << 8) ^ bytes ^ 0x4E42_5333
}

fn consume(item: WorkItem) {
    let mut feature = item.checksum ^ item.bytes;
    let rounds = if item.kind == 1 { 96 } else { 512 };
    for index in 0..rounds {
        feature = feature
            .rotate_left(5)
            .wrapping_add(index ^ u32::from(item.sequence));
    }
    let now = Esp32S3Clock::now_us();
    let latency = now.saturating_sub(item.produced_us);
    MAX_LATENCY_US.fetch_max(latency.min(u64::from(u32::MAX)) as u32, Ordering::AcqRel);
    if now > item.deadline_us {
        DEADLINE_MISSES.fetch_add(1, Ordering::AcqRel);
    }
    if item.kind == 1 {
        AUDIO_PROCESSED.fetch_add(1, Ordering::AcqRel);
    } else {
        CAMERA_PROCESSED.fetch_add(1, Ordering::AcqRel);
    }
    CHECKSUM.fetch_xor(feature, Ordering::AcqRel);
    PROCESSED.fetch_add(1, Ordering::Release);
}

fn app_core() -> ! {
    loop {
        if let Some(item) = WORK.try_recv() {
            consume(item);
        }
    }
}

fn item(kind: u8, sequence: u16) -> WorkItem {
    let now = Esp32S3Clock::now_us();
    let bytes = if kind == 1 { AUDIO_BYTES } else { CAMERA_BYTES };
    let budget = if kind == 1 { 20_000 } else { 100_000 };
    WorkItem {
        kind,
        sequence,
        bytes,
        produced_us: now,
        deadline_us: now.saturating_add(budget),
        checksum: payload_checksum(kind, sequence, bytes),
    }
}

#[esp_hal::main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let mut usb = Esp32S3Usb::new(UsbSerialJtag::new(peripherals.USB_DEVICE));

    for sequence in 0..QUEUE_CAPACITY as u16 {
        let _ = WORK.try_send(item(1, sequence));
    }
    if WORK.try_send(item(1, QUEUE_CAPACITY as u16)).is_err() {
        BACKPRESSURE.fetch_add(1, Ordering::AcqRel);
    }

    let mut cpu = CpuControl::new(peripherals.CPU_CTRL);
    let stack = unsafe { &mut *core::ptr::addr_of_mut!(APP_CORE_STACK) };
    let core1 = cpu.start_app_core(stack, || {
        app_core();
    });
    let core_started = core1.is_ok();
    let _core_guard = core1.ok();

    let mut sent = QUEUE_CAPACITY as u32;
    let send_timeout = Esp32S3Clock::now_us().saturating_add(2_000_000);
    while sent < TOTAL_ITEMS && Esp32S3Clock::now_us() < send_timeout {
        let kind = if sent < AUDIO_ITEMS { 1 } else { 2 };
        if WORK.try_send(item(kind, sent as u16)).is_ok() {
            sent += 1;
        } else {
            BACKPRESSURE.fetch_add(1, Ordering::AcqRel);
        }
    }

    let process_timeout = Esp32S3Clock::now_us().saturating_add(3_000_000);
    while PROCESSED.load(Ordering::Acquire) < sent && Esp32S3Clock::now_us() < process_timeout {}

    let processed = PROCESSED.load(Ordering::Acquire);
    let audio = AUDIO_PROCESSED.load(Ordering::Acquire);
    let camera = CAMERA_PROCESSED.load(Ordering::Acquire);
    let misses = DEADLINE_MISSES.load(Ordering::Acquire);
    let backpressure = BACKPRESSURE.load(Ordering::Acquire);
    let all_pass = core_started
        && sent == TOTAL_ITEMS
        && processed == TOTAL_ITEMS
        && audio == AUDIO_ITEMS
        && camera == CAMERA_ITEMS
        && misses == 0
        && backpressure > 0
        && CHECKSUM.load(Ordering::Acquire) != 0;

    loop {
        let _ = writeln!(
            usb,
            "NOBRO-S3-MC cores=2 sent={} processed={} audio={} camera={} backpressure={} misses={} max_latency_us={} bounded_bytes={} checksum={} all_pass={}",
            sent,
            processed,
            audio,
            camera,
            backpressure,
            misses,
            MAX_LATENCY_US.load(Ordering::Acquire),
            MAX_PIPELINE_BYTES,
            CHECKSUM.load(Ordering::Acquire),
            u32::from(all_pass)
        );
        esp_hal::delay::Delay::new().delay_millis(1000);
    }
}
