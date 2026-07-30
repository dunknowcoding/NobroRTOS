//! NobroRTOS shared-RP contract status firmware for the official Pico.
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};

use panic_halt as _;
use rp2040_hal as hal;
use usb_device::{class_prelude::UsbBusAllocator, prelude::*};
use usbd_serial::SerialPort;

#[cfg(feature = "dma-completion")]
use hal::dma::DMAExt;
use hal::{
    multicore::{Multicore, Stack},
    usb::UsbBus,
};
use nobro_hal::{Rp2MulticoreContract, Rp2Power, Rp2ResetBackend, RP2040_RUNTIME};

#[cfg(feature = "dma-completion")]
mod dma_completion;
mod portable;

#[link_section = ".boot2"]
#[used]
pub static BOOT2: [u8; 256] = rp2040_boot2::BOOT_LOADER_W25Q080;

const XTAL_FREQ_HZ: u32 = 12_000_000;
static CORE1_STACK: Stack<1024> = Stack::new();
static CORE1_HEARTBEAT: AtomicU32 = AtomicU32::new(0);

fn core1_task() {
    loop {
        let next = CORE1_HEARTBEAT.load(Ordering::Relaxed).wrapping_add(1);
        CORE1_HEARTBEAT.store(next, Ordering::Relaxed);
        cortex_m::asm::wfe();
    }
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
    let timer = hal::Timer::new(pac.TIMER, &mut pac.RESETS, &clocks);
    let _reset_cause = <portable::Rp2040Reset as Rp2ResetBackend>::reset_cause();
    let _power = Rp2Power::try_new(portable::Rp2040Power, 2).unwrap();
    #[cfg(feature = "dma-completion")]
    let _dma_contract_ok = {
        let channels = pac.DMA.split(&mut pac.RESETS);
        let mut provider = dma_completion::Dma0Completion::new(
            channels.ch0,
            dma_completion::DmaCompletionPriority::port_default(),
        );
        dma_completion::validate_contract(&mut provider)
    };

    let core1_lease = Rp2MulticoreContract::try_acquire(1).unwrap();
    let mut sio = hal::Sio::new(pac.SIO);
    let mut multicore = Multicore::new(&mut pac.PSM, &mut pac.PPB, &mut sio.fifo);
    multicore.cores()[1]
        .spawn(CORE1_STACK.take().unwrap(), core1_task)
        .unwrap();
    core::mem::forget(core1_lease);

    let usb_bus = UsbBusAllocator::new(UsbBus::new(
        pac.USBCTRL_REGS,
        pac.USBCTRL_DPRAM,
        clocks.usb_clock,
        true,
        &mut pac.RESETS,
    ));
    let mut serial = SerialPort::new(&usb_bus);
    let mut usb = UsbDeviceBuilder::new(&usb_bus, UsbVidPid(0x1209, 0x4e43))
        .strings(&[StringDescriptors::default()
            .manufacturer("NobroRTOS")
            .product("NobroRTOS RP2040")
            .serial_number("NOBRO-RP2040")])
        .unwrap()
        .device_class(usbd_serial::USB_CLASS_CDC)
        .build();

    let timebase_ok = portable::verify_timebase_provider();
    let mut last_report = timer.get_counter();
    let mut command = [0u8; 8];
    let mut command_len = 0usize;

    loop {
        let _ = usb.poll(&mut [&mut serial]);
        cortex_m::asm::sev();

        if (timer.get_counter() - last_report).to_millis() >= 1_000 {
            last_report = timer.get_counter();
            let report = if timebase_ok
                && RP2040_RUNTIME.cores == 2
                && CORE1_HEARTBEAT.load(Ordering::Relaxed) != 0
            {
                b"NOBRO-RP2040 shared=1 timebase=1 cores=2 all_pass=1\r\n".as_slice()
            } else {
                b"NOBRO-RP2040 shared=1 all_pass=0\r\n".as_slice()
            };
            let _ = serial.write(report);
        }

        let mut input = [0u8; 16];
        if let Ok(count) = serial.read(&mut input) {
            for &byte in &input[..count] {
                if matches!(byte, b'\r' | b'\n') {
                    if &command[..command_len] == b"DFU" {
                        let _ = serial.write(b"rebooting to BOOTSEL\r\n");
                        hal::rom_data::reset_to_usb_boot(0, 0);
                    }
                    command_len = 0;
                } else if command_len < command.len() {
                    command[command_len] = byte;
                    command_len += 1;
                }
            }
        }

        cortex_m::asm::wfe();
    }
}
