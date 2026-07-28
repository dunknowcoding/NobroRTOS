#![no_main]
#![no_std]

use core::{ptr, slice};

use cortex_m_rt::entry;
use defmt_rtt as _;
use ed25519_dalek::{Signer, SigningKey};
use nobro_secure::{
    verify_signed_boot, BoardBootAdapter, BoardBootLayout, BootFlash, BootSelectionKind,
    BootVectorPolicy, FlashRange, PinnedKeyPolicy, SignedImageManifest, Slot, VerifiedSignedImage,
    SLOT_HEADER_BYTES,
};
use panic_halt as _;

const SLOT_A: u32 = 0x000C_0000;
const SLOT_B: u32 = 0x000C_1000;
const JOURNAL_A: u32 = 0x000C_2000;
const JOURNAL_B: u32 = 0x000C_3000;
const PAGE: u32 = 4096;
const FLASH_BYTES: u32 = 0x0010_0000;
const SOFTDEVICE_END: u32 = 0x0002_6000;
const BOOTLOADER_START: u32 = 0x000F_4000;

const NVMC: usize = 0x4001_E000;
const READY: usize = 0x400;
const CONFIG: usize = 0x504;
const ERASEPAGE: usize = 0x508;

#[repr(C)]
pub struct BootSlotReport {
    magic: u32,
    version: u32,
    all_pass: u32,
    adopted: u32,
    trial: u32,
    reverted: u32,
    confirmed: u32,
    protected_unchanged: u32,
    trial_generation: u32,
    confirmed_generation: u32,
    diagnostic_checksum: u32,
}

#[no_mangle]
pub static mut NOBRO_BOOT_SLOT_REPORT: BootSlotReport = BootSlotReport {
    magic: 0,
    version: 0,
    all_pass: 0,
    adopted: 0,
    trial: 0,
    reverted: 0,
    confirmed: 0,
    protected_unchanged: 0,
    trial_generation: 0,
    confirmed_generation: 0,
    diagnostic_checksum: 0,
};

struct NrfNvmc;

impl NrfNvmc {
    fn wait_ready() {
        while unsafe { ptr::read_volatile((NVMC + READY) as *const u32) } == 0 {
            core::hint::spin_loop();
        }
    }

    fn mode(value: u32) {
        unsafe {
            ptr::write_volatile((NVMC + CONFIG) as *mut u32, value);
        }
        Self::wait_ready();
    }
}

impl BootFlash for NrfNvmc {
    type Error = ();

    fn capacity(&self) -> u32 {
        FLASH_BYTES
    }

    fn erase_size(&self) -> u32 {
        PAGE
    }

    fn program_size(&self) -> u32 {
        4
    }

    fn read(&self, address: u32, output: &mut [u8]) -> Result<(), Self::Error> {
        let end = address.checked_add(output.len() as u32).ok_or(())?;
        if end > FLASH_BYTES {
            return Err(());
        }
        let source = unsafe { slice::from_raw_parts(address as *const u8, output.len()) };
        output.copy_from_slice(source);
        Ok(())
    }

    fn erase(&mut self, address: u32, len: u32) -> Result<(), Self::Error> {
        let end = address.checked_add(len).ok_or(())?;
        if !address.is_multiple_of(PAGE) || !len.is_multiple_of(PAGE) || end > FLASH_BYTES {
            return Err(());
        }
        Self::mode(2);
        let mut page = address;
        while page < end {
            unsafe {
                ptr::write_volatile((NVMC + ERASEPAGE) as *mut u32, page);
            }
            Self::wait_ready();
            page += PAGE;
        }
        Self::mode(0);
        Ok(())
    }

    fn program(&mut self, address: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        let end = address.checked_add(bytes.len() as u32).ok_or(())?;
        if !address.is_multiple_of(4) || !bytes.len().is_multiple_of(4) || end > FLASH_BYTES {
            return Err(());
        }
        Self::mode(1);
        for (index, word) in bytes.chunks_exact(4).enumerate() {
            unsafe {
                ptr::write_volatile(
                    (address as usize + index * 4) as *mut u32,
                    u32::from_le_bytes([word[0], word[1], word[2], word[3]]),
                );
            }
            Self::wait_ready();
        }
        Self::mode(0);
        Ok(())
    }
}

fn layout() -> BoardBootLayout<2> {
    BoardBootLayout {
        slots: [FlashRange::new(SLOT_A, PAGE), FlashRange::new(SLOT_B, PAGE)],
        journal: [
            FlashRange::new(JOURNAL_A, PAGE),
            FlashRange::new(JOURNAL_B, PAGE),
        ],
        protected: [
            FlashRange::new(0, SOFTDEVICE_END),
            FlashRange::new(BOOTLOADER_START, FLASH_BYTES - BOOTLOADER_START),
        ],
    }
}

fn token(
    image: &[u8],
    slot: Slot,
    version: u32,
    signing: &SigningKey,
    keys: &PinnedKeyPolicy<1>,
) -> VerifiedSignedImage {
    let load_addr = match slot {
        Slot::A => SLOT_A + SLOT_HEADER_BYTES,
        Slot::B => SLOT_B + SLOT_HEADER_BYTES,
    };
    let mut manifest = SignedImageManifest {
        key_id: 7,
        version,
        image_len: image.len() as u32,
        load_addr,
        entry_addr: load_addr | 1,
        stack_top: 0x2003_F000,
        measurement: nobro_secure::SecureBoot::measure(image),
        signature: [0; 64],
    };
    manifest.signature = signing.sign(&manifest.signing_digest()).to_bytes();
    verify_signed_boot(
        image,
        &manifest,
        keys,
        BootVectorPolicy::cortex_m(
            SLOT_A + SLOT_HEADER_BYTES,
            JOURNAL_A,
            0x2000_0000,
            0x2004_0000,
        ),
        0,
    )
    .unwrap()
}

fn diagnostic_checksum(words: &[u32]) -> u32 {
    words
        .iter()
        .fold(0x70B0_0700, |sum, word| sum.rotate_left(5) ^ word)
}

#[entry]
fn main() -> ! {
    let image_a = [0xA5u8; 256];
    let image_b = [0x5Au8; 256];
    let signing = SigningKey::from_bytes(&[7; 32]);
    let mut keys = PinnedKeyPolicy::<1>::new();
    let key_ok = keys.pin(7, signing.verifying_key().to_bytes());
    let token_a = token(&image_a, Slot::A, 1, &signing, &keys);
    let token_b = token(&image_b, Slot::B, 2, &signing, &keys);

    let protected_before = unsafe {
        ptr::read_volatile(0x0000_1000 as *const u32)
            ^ ptr::read_volatile(BOOTLOADER_START as *const u32)
    };

    let flash = NrfNvmc;
    let mut adapter = BoardBootAdapter::new(flash, layout()).unwrap();
    let vectors = BootVectorPolicy::cortex_m(
        SLOT_A + SLOT_HEADER_BYTES,
        JOURNAL_A,
        0x2000_0000,
        0x2004_0000,
    );
    let adopted = adapter
        .initialize_active(Slot::A, &image_a, &token_a, &keys, vectors)
        .is_ok();
    let fixture_ok = adopted;
    let staged_once = adapter.stage(Slot::B, &image_b, &token_b).is_ok();
    let trial = adapter.select_boot(&keys, vectors).unwrap();
    let reverted = adapter.select_boot(&keys, vectors).unwrap();
    let staged_twice = adapter.stage(Slot::B, &image_b, &token_b).is_ok();
    let trial_again = adapter.select_boot(&keys, vectors).unwrap();
    let confirmed_receipt = adapter.confirm(2, &keys, vectors).unwrap();
    let confirmed = adapter.select_boot(&keys, vectors).unwrap();
    let flash = adapter.into_flash();

    let protected_after = unsafe {
        ptr::read_volatile(0x0000_1000 as *const u32)
            ^ ptr::read_volatile(BOOTLOADER_START as *const u32)
    };
    let protected_unchanged = protected_before == protected_after;
    let readback_ok = {
        let mut a = [0u8; 256];
        let mut b = [0u8; 256];
        flash.read(SLOT_A + SLOT_HEADER_BYTES, &mut a).is_ok()
            && flash.read(SLOT_B + SLOT_HEADER_BYTES, &mut b).is_ok()
            && a == image_a
            && b == image_b
    };

    let trial_ok = trial.slot == Slot::B && trial.kind == BootSelectionKind::Trial;
    let revert_ok = reverted.slot == Slot::A && reverted.kind == BootSelectionKind::Reverted;
    let confirmed_ok = trial_again.slot == Slot::B
        && trial_again.kind == BootSelectionKind::Trial
        && confirmed.slot == Slot::B
        && confirmed.kind == BootSelectionKind::Confirmed;
    let all_pass = key_ok
        && fixture_ok
        && adopted
        && staged_once
        && trial_ok
        && revert_ok
        && staged_twice
        && confirmed_ok
        && protected_unchanged
        && readback_ok;
    let fields = [
        0x4E42_534C,
        1,
        u32::from(all_pass),
        u32::from(adopted),
        u32::from(trial_ok),
        u32::from(revert_ok),
        u32::from(confirmed_ok),
        u32::from(protected_unchanged),
        trial.generation,
        confirmed_receipt.generation,
    ];
    unsafe {
        NOBRO_BOOT_SLOT_REPORT = BootSlotReport {
            magic: fields[0],
            version: fields[1],
            all_pass: fields[2],
            adopted: fields[3],
            trial: fields[4],
            reverted: fields[5],
            confirmed: fields[6],
            protected_unchanged: fields[7],
            trial_generation: fields[8],
            confirmed_generation: fields[9],
            diagnostic_checksum: diagnostic_checksum(&fields),
        };
    }

    loop {
        cortex_m::asm::bkpt();
    }
}
