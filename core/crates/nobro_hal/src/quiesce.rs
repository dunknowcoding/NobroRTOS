//! Board-specific peripheral shutdown used before a lease changes owner.
//!
//! Recovery must stop hardware before publishing the slot as free. Otherwise an old
//! DMA engine or interrupt can mutate the new owner's buffers/state after reassignment.

use crate::lease::Resource;

#[cfg(test)]
use core::sync::atomic::{AtomicU32, Ordering};

#[cfg(test)]
static QUIESCE_COUNTS: [AtomicU32; Resource::COUNT] =
    [const { AtomicU32::new(0) }; Resource::COUNT];

#[cfg(test)]
fn index(resource: Resource) -> usize {
    Resource::ALL
        .iter()
        .position(|item| *item == resource)
        .unwrap()
}

pub(crate) fn resource(resource: Resource) {
    let _ = resource;
    #[cfg(test)]
    QUIESCE_COUNTS[index(resource)].fetch_add(1, Ordering::AcqRel);
    #[cfg(all(target_arch = "arm", feature = "platform-nrf52840"))]
    unsafe {
        quiesce_nrf52840(resource);
    }
}

#[cfg(test)]
pub(crate) fn count(resource: Resource) -> u32 {
    QUIESCE_COUNTS[index(resource)].load(Ordering::Acquire)
}

#[cfg(all(target_arch = "arm", feature = "platform-nrf52840"))]
unsafe fn write(base: u32, offset: u32, value: u32) {
    core::ptr::write_volatile((base + offset) as *mut u32, value);
}

#[cfg(all(target_arch = "arm", feature = "platform-nrf52840"))]
unsafe fn quiesce_nrf52840(resource: Resource) {
    match resource {
        Resource::Twim0 | Resource::Spim0 => {
            let base = 0x4000_3000;
            write(base, 0x014, 1); // TASKS_STOP
            write(base, 0x308, 0xFFFF_FFFF); // INTENCLR
            write(base, 0x500, 0); // ENABLE
                                   // EasyDMA PTR/MAXCNT pairs. Harmless for polled TWIM aliases.
            for offset in [0x534, 0x538, 0x544, 0x548] {
                write(base, offset, 0);
            }
        }
        Resource::Twim1 => {
            let base = 0x4000_4000;
            write(base, 0x014, 1);
            write(base, 0x308, 0xFFFF_FFFF);
            write(base, 0x500, 0);
        }
        Resource::Radio => {
            let base = 0x4000_1000;
            write(base, 0x200, 0); // SHORTS
            write(base, 0x308, 0xFFFF_FFFF);
            write(base, 0x010, 1); // TASKS_DISABLE
            write(base, 0x504, 0); // PACKETPTR
        }
        Resource::Timer0 | Resource::Timer1 | Resource::Timer2 => {
            let base = if resource == Resource::Timer0 {
                0x4000_8000
            } else if resource == Resource::Timer1 {
                0x4000_9000
            } else {
                0x4000_A000
            };
            write(base, 0x004, 1); // TASKS_STOP
            write(base, 0x308, 0xFFFF_FFFF);
            write(base, 0x000, 0); // TASKS_START clear/no-op
        }
        Resource::Rtc2 => {
            let base = 0x4002_4000;
            write(base, 0x004, 1);
            write(base, 0x308, 0xFFFF_FFFF);
            write(base, 0x344, 0xFFFF_FFFF); // EVTENCLR
        }
        Resource::Pwm0 => {
            let base = 0x4001_C000;
            write(base, 0x004, 1); // TASKS_STOP
            write(base, 0x308, 0xFFFF_FFFF);
            write(base, 0x500, 0);
            write(base, 0x520, 0); // SEQ0.PTR
            write(base, 0x524, 0); // SEQ0.CNT
            for offset in [0x560, 0x564, 0x568, 0x56C] {
                write(base, offset, 0x8000_0000);
            }
        }
        Resource::Egu0 => {
            write(0x4001_4000, 0x308, 0xFFFF_FFFF);
        }
        Resource::Ppi => {
            write(0x4001_F000, 0x508, 0xFFFF_FFFF); // CHENCLR
        }
        Resource::Gpio => {
            // Preserve the driven levels during owner recovery. The next owner
            // must configure every pin it uses before issuing an operation.
        }
        Resource::Gpiote => {
            let base = 0x4000_6000;
            write(base, 0x304, 0xFFFF_FFFF); // INTENCLR
            for channel in 0..8 {
                write(base, 0x510 + channel * 4, 0); // CONFIG[n]
            }
        }
        Resource::Uarte0 => {
            let base = 0x4000_2000;
            write(base, 0x00c, 1); // TASKS_STOPTX
            write(base, 0x004, 1); // TASKS_STOPRX
            write(base, 0x500, 0); // ENABLE
        }
        Resource::Saadc => {
            let base = 0x4000_7000;
            write(base, 0x008, 1); // TASKS_STOP
            write(base, 0x500, 0); // ENABLE
        }
        Resource::Nvmc => {
            write(0x4001_E000, 0x504, 0); // CONFIG: read-only
        }
    }
    core::arch::asm!("dsb sy", "isb sy", options(nostack, preserves_flags));
}
