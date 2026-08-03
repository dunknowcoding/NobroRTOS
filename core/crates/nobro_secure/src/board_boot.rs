//! Power-cut-safe board slot integration.
//!
//! The adapter owns only explicitly declared application slots and journal pages.
//! Bootloaders, SoftDevices, calibration rows, and other immutable regions must be
//! listed as protected ranges; layout validation rejects every overlap before the
//! first media mutation.

use nobro_crypto::sha256::{sha256, Sha256};

use crate::{
    verify_signed_measurement, BootVectorPolicy, PinnedKeyPolicy, SignedImageManifest, Slot,
    VerifiedBootPlan, VerifiedSignedImage,
};

pub const SLOT_HEADER_BYTES: u32 = 256;
const RECORD_BYTES: usize = 256;
const RECORD_BODY_BYTES: usize = 252;
const RECORD_DIGEST_OFFSET: usize = 216;
const RECORD_MARKER_OFFSET: usize = 252;
const RECORD_MAGIC: u32 = 0x4E42_4A52;
const RECORD_SCHEMA: u32 = 1;
const RECORD_COMMITTED: u32 = 0xB007_C0DE;
const SLOT_HEADER_MAGIC: u32 = 0x4E42_5348;
const SLOT_HEADER_SCHEMA: u32 = 1;
const SLOT_HEADER_DIGEST_OFFSET: usize = 160;
const SLOT_HEADER_MARKER_OFFSET: usize = 252;
const SLOT_HEADER_COMMITTED: u32 = 0x51A7_C0DE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlashRange {
    pub start: u32,
    pub len: u32,
}

impl FlashRange {
    pub const fn new(start: u32, len: u32) -> Self {
        Self { start, len }
    }

    pub const fn end(self) -> Option<u32> {
        self.start.checked_add(self.len)
    }

    pub const fn contains(self, start: u32, len: u32) -> bool {
        let Some(end) = start.checked_add(len) else {
            return false;
        };
        let Some(self_end) = self.end() else {
            return false;
        };
        start >= self.start && end <= self_end
    }

    pub const fn overlaps(self, other: Self) -> bool {
        let (Some(a_end), Some(b_end)) = (self.end(), other.end()) else {
            return true;
        };
        self.start < b_end && other.start < a_end
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootLayoutError {
    EmptyRange,
    AddressOverflow,
    OutsideMedia,
    Misaligned,
    SlotOverlap,
    JournalOverlap,
    ProtectedOverlap,
    JournalTooSmall,
    SlotTooSmall,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoardBootLayout<const PROTECTED: usize> {
    pub slots: [FlashRange; 2],
    pub journal: [FlashRange; 2],
    pub protected: [FlashRange; PROTECTED],
}

impl<const PROTECTED: usize> BoardBootLayout<PROTECTED> {
    pub fn validate(
        &self,
        capacity: u32,
        erase_size: u32,
        program_size: u32,
    ) -> Result<(), BootLayoutError> {
        if erase_size == 0 || program_size == 0 {
            return Err(BootLayoutError::Misaligned);
        }
        for range in self.slots.iter().chain(self.journal.iter()) {
            let Some(end) = range.end() else {
                return Err(BootLayoutError::AddressOverflow);
            };
            if range.len == 0 {
                return Err(BootLayoutError::EmptyRange);
            }
            if end > capacity {
                return Err(BootLayoutError::OutsideMedia);
            }
            if range.start % erase_size != 0
                || range.len % erase_size != 0
                || range.start % program_size != 0
            {
                return Err(BootLayoutError::Misaligned);
            }
        }
        if self.slots.iter().any(|slot| slot.len <= SLOT_HEADER_BYTES) {
            return Err(BootLayoutError::SlotTooSmall);
        }
        if self.slots[0].overlaps(self.slots[1]) {
            return Err(BootLayoutError::SlotOverlap);
        }
        if self.journal[0].overlaps(self.journal[1]) {
            return Err(BootLayoutError::JournalOverlap);
        }
        if self
            .journal
            .iter()
            .any(|page| page.len < RECORD_BYTES as u32)
        {
            return Err(BootLayoutError::JournalTooSmall);
        }
        for slot in self.slots {
            if self.journal.iter().any(|page| slot.overlaps(*page)) {
                return Err(BootLayoutError::JournalOverlap);
            }
        }
        for protected in self.protected {
            let Some(end) = protected.end() else {
                return Err(BootLayoutError::AddressOverflow);
            };
            if protected.len == 0 || end > capacity {
                return Err(BootLayoutError::OutsideMedia);
            }
            if self
                .slots
                .iter()
                .chain(self.journal.iter())
                .any(|owned| owned.overlaps(protected))
            {
                return Err(BootLayoutError::ProtectedOverlap);
            }
        }
        Ok(())
    }

    const fn slot(&self, slot: Slot) -> FlashRange {
        self.slots[slot_index(slot)]
    }
}

pub trait BootFlash {
    type Error;

    fn capacity(&self) -> u32;
    fn erase_size(&self) -> u32;
    fn program_size(&self) -> u32;
    fn read(&self, address: u32, output: &mut [u8]) -> Result<(), Self::Error>;
    fn erase(&mut self, address: u32, len: u32) -> Result<(), Self::Error>;
    fn program(&mut self, address: u32, bytes: &[u8]) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootAdapterError<E> {
    Media(E),
    Layout(BootLayoutError),
    Uninitialized,
    NoBootableImage,
    ActiveSlotWrite,
    ImageDoesNotMatchToken,
    SlotTooSmall,
    JournalRollback,
    NoPendingTrial,
    VersionMismatch,
    ReadbackMismatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootSelectionKind {
    Confirmed,
    Trial,
    Reverted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoardBootSelection {
    pub slot: Slot,
    pub plan: VerifiedBootPlan,
    pub kind: BootSelectionKind,
    pub generation: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoardBootReceipt {
    pub slot: Slot,
    pub version: u32,
    pub generation: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SlotDescriptor {
    valid: bool,
    plan: VerifiedBootPlan,
    measurement: [u8; 32],
    manifest_digest: [u8; 32],
}

impl SlotDescriptor {
    const EMPTY: Self = Self {
        valid: false,
        plan: VerifiedBootPlan {
            version: 0,
            image_len: 0,
            load_addr: 0,
            entry_addr: 0,
            stack_top: 0,
        },
        measurement: [0; 32],
        manifest_digest: [0; 32],
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct JournalState {
    generation: u32,
    active: Slot,
    confirmed_version: u32,
    pending: Option<Slot>,
    trial_attempted: bool,
    slots: [SlotDescriptor; 2],
}

pub struct BoardBootAdapter<F, const PROTECTED: usize> {
    flash: F,
    layout: BoardBootLayout<PROTECTED>,
}

impl<F: BootFlash, const PROTECTED: usize> BoardBootAdapter<F, PROTECTED> {
    pub fn new(
        flash: F,
        layout: BoardBootLayout<PROTECTED>,
    ) -> Result<Self, BootAdapterError<F::Error>> {
        layout
            .validate(flash.capacity(), flash.erase_size(), flash.program_size())
            .map_err(BootAdapterError::Layout)?;
        Ok(Self { flash, layout })
    }

    pub fn into_flash(self) -> F {
        self.flash
    }

    /// Establish an existing verified image as the first confirmed slot.
    ///
    /// This reads and hashes the already-installed bytes. It never erases or
    /// programs an application slot, so a board can adopt its factory image
    /// without rewriting it.
    pub fn adopt_active<const N: usize>(
        &mut self,
        slot: Slot,
        verified: &VerifiedSignedImage,
        keys: &PinnedKeyPolicy<N>,
        vectors: BootVectorPolicy,
    ) -> Result<BoardBootReceipt, BootAdapterError<F::Error>> {
        if self.read_latest()?.is_some() {
            return Err(BootAdapterError::JournalRollback);
        }
        let descriptor = self.descriptor_from_media(slot, verified)?;
        if !self.descriptor_authorized(slot, descriptor, keys, vectors, 0)? {
            return Err(BootAdapterError::ImageDoesNotMatchToken);
        }
        let state = JournalState {
            generation: 1,
            active: slot,
            confirmed_version: descriptor.plan.version,
            pending: None,
            trial_attempted: false,
            slots: {
                let mut slots = [SlotDescriptor::EMPTY; 2];
                slots[slot_index(slot)] = descriptor;
                slots
            },
        };
        self.commit_state(None, state)?;
        Ok(BoardBootReceipt {
            slot,
            version: descriptor.plan.version,
            generation: state.generation,
        })
    }

    /// Install the first signed image into an empty owned slot and establish it
    /// as confirmed. Factory code uses this path; normal updates use [`Self::stage`].
    pub fn initialize_active<const N: usize>(
        &mut self,
        slot: Slot,
        image: &[u8],
        verified: &VerifiedSignedImage,
        keys: &PinnedKeyPolicy<N>,
        vectors: BootVectorPolicy,
    ) -> Result<BoardBootReceipt, BootAdapterError<F::Error>> {
        if self.read_latest()?.is_some() {
            return Err(BootAdapterError::JournalRollback);
        }
        self.validate_candidate(slot, image, verified)?;
        self.write_and_verify(slot, image, verified)?;
        self.adopt_active(slot, verified, keys, vectors)
    }

    /// Install a verified candidate into the inactive slot and atomically stage it.
    ///
    /// The verified token binds both vectors and image measurement. The adapter
    /// hashes the caller's bytes again before and after programming, closing the
    /// verification-to-write substitution gap.
    pub fn stage(
        &mut self,
        slot: Slot,
        image: &[u8],
        verified: &VerifiedSignedImage,
    ) -> Result<BoardBootReceipt, BootAdapterError<F::Error>> {
        let Some((page, mut state)) = self.read_latest()? else {
            return Err(BootAdapterError::Uninitialized);
        };
        if slot == state.active {
            return Err(BootAdapterError::ActiveSlotWrite);
        }
        self.validate_candidate(slot, image, verified)?;
        self.write_and_verify(slot, image, verified)?;
        let descriptor = descriptor_from_verified(verified);
        state.slots[slot_index(slot)] = descriptor;
        state.pending = Some(slot);
        state.trial_attempted = false;
        state.generation = next_generation(state.generation)?;
        self.commit_state(Some(page), state)?;
        Ok(BoardBootReceipt {
            slot,
            version: descriptor.plan.version,
            generation: state.generation,
        })
    }

    /// Select a bootable image and persist trial consumption before returning it.
    ///
    /// A reset after the returned trial but before [`Self::confirm`] therefore
    /// selects the last confirmed slot. A corrupt candidate is reverted in the
    /// same call; a corrupt confirmed slot fails closed.
    pub fn select_boot<const N: usize>(
        &mut self,
        keys: &PinnedKeyPolicy<N>,
        vectors: BootVectorPolicy,
    ) -> Result<BoardBootSelection, BootAdapterError<F::Error>> {
        let Some((page, mut state)) = self.read_latest()? else {
            return Err(BootAdapterError::Uninitialized);
        };
        if let Some(pending) = state.pending {
            if !state.trial_attempted {
                state.trial_attempted = true;
                state.generation = next_generation(state.generation)?;
                self.commit_state(Some(page), state)?;
                if self.descriptor_authorized(
                    pending,
                    state.slots[slot_index(pending)],
                    keys,
                    vectors,
                    state.confirmed_version,
                )? {
                    return Ok(selection(state, pending, BootSelectionKind::Trial));
                }
                let (new_page, mut committed) =
                    self.read_latest()?.ok_or(BootAdapterError::Uninitialized)?;
                committed.pending = None;
                committed.trial_attempted = false;
                committed.generation = next_generation(committed.generation)?;
                self.commit_state(Some(new_page), committed)?;
                if self.descriptor_authorized(
                    committed.active,
                    committed.slots[slot_index(committed.active)],
                    keys,
                    vectors,
                    committed.confirmed_version,
                )? {
                    return Ok(selection(
                        committed,
                        committed.active,
                        BootSelectionKind::Reverted,
                    ));
                }
                return Err(BootAdapterError::NoBootableImage);
            }

            state.pending = None;
            state.trial_attempted = false;
            state.generation = next_generation(state.generation)?;
            self.commit_state(Some(page), state)?;
            if self.descriptor_authorized(
                state.active,
                state.slots[slot_index(state.active)],
                keys,
                vectors,
                state.confirmed_version,
            )? {
                return Ok(selection(state, state.active, BootSelectionKind::Reverted));
            }
            return Err(BootAdapterError::NoBootableImage);
        }

        if self.descriptor_authorized(
            state.active,
            state.slots[slot_index(state.active)],
            keys,
            vectors,
            state.confirmed_version,
        )? {
            Ok(selection(state, state.active, BootSelectionKind::Confirmed))
        } else {
            Err(BootAdapterError::NoBootableImage)
        }
    }

    pub fn confirm<const N: usize>(
        &mut self,
        version: u32,
        keys: &PinnedKeyPolicy<N>,
        vectors: BootVectorPolicy,
    ) -> Result<BoardBootReceipt, BootAdapterError<F::Error>> {
        let Some((page, mut state)) = self.read_latest()? else {
            return Err(BootAdapterError::Uninitialized);
        };
        let Some(slot) = state.pending else {
            return Err(BootAdapterError::NoPendingTrial);
        };
        let descriptor = state.slots[slot_index(slot)];
        if !state.trial_attempted || descriptor.plan.version != version {
            return Err(BootAdapterError::VersionMismatch);
        }
        if !self.descriptor_authorized(slot, descriptor, keys, vectors, state.confirmed_version)? {
            return Err(BootAdapterError::NoBootableImage);
        }
        state.active = slot;
        state.confirmed_version = version;
        state.pending = None;
        state.trial_attempted = false;
        state.generation = next_generation(state.generation)?;
        self.commit_state(Some(page), state)?;
        Ok(BoardBootReceipt {
            slot,
            version,
            generation: state.generation,
        })
    }

    fn validate_candidate(
        &self,
        slot: Slot,
        image: &[u8],
        verified: &VerifiedSignedImage,
    ) -> Result<(), BootAdapterError<F::Error>> {
        let plan = verified.plan();
        let region = self.layout.slot(slot);
        let image_start = region
            .start
            .checked_add(SLOT_HEADER_BYTES)
            .ok_or(BootAdapterError::SlotTooSmall)?;
        if plan.load_addr != image_start || !region.contains(plan.load_addr, plan.image_len) {
            return Err(BootAdapterError::SlotTooSmall);
        }
        if image.len() != plan.image_len as usize || sha256(image) != verified.image_measurement() {
            return Err(BootAdapterError::ImageDoesNotMatchToken);
        }
        Ok(())
    }

    fn descriptor_from_media(
        &self,
        slot: Slot,
        verified: &VerifiedSignedImage,
    ) -> Result<SlotDescriptor, BootAdapterError<F::Error>> {
        let plan = verified.plan();
        let region = self.layout.slot(slot);
        let image_start = region
            .start
            .checked_add(SLOT_HEADER_BYTES)
            .ok_or(BootAdapterError::SlotTooSmall)?;
        if plan.load_addr != image_start || !region.contains(plan.load_addr, plan.image_len) {
            return Err(BootAdapterError::SlotTooSmall);
        }
        let descriptor = descriptor_from_verified(verified);
        let Some(manifest) = self.read_slot_manifest(slot)? else {
            return Err(BootAdapterError::ImageDoesNotMatchToken);
        };
        if manifest != verified.manifest() || !self.descriptor_intact(slot, descriptor)? {
            return Err(BootAdapterError::ImageDoesNotMatchToken);
        }
        Ok(descriptor)
    }

    fn write_and_verify(
        &mut self,
        slot: Slot,
        image: &[u8],
        verified: &VerifiedSignedImage,
    ) -> Result<(), BootAdapterError<F::Error>> {
        let region = self.layout.slot(slot);
        self.flash
            .erase(region.start, region.len)
            .map_err(BootAdapterError::Media)?;
        let width = self.flash.program_size() as usize;
        let mut offset = 0usize;
        let mut word = [0xFFu8; 32];
        if width == 0 || width > word.len() {
            return Err(BootAdapterError::Layout(BootLayoutError::Misaligned));
        }
        while offset < image.len() {
            word[..width].fill(0xFF);
            let take = width.min(image.len() - offset);
            word[..take].copy_from_slice(&image[offset..offset + take]);
            self.flash
                .program(
                    region.start + SLOT_HEADER_BYTES + offset as u32,
                    &word[..width],
                )
                .map_err(BootAdapterError::Media)?;
            offset += take;
        }
        let actual = self.hash_media(region.start + SLOT_HEADER_BYTES, image.len() as u32)?;
        if actual != sha256(image) {
            return Err(BootAdapterError::ReadbackMismatch);
        }
        self.write_slot_manifest(slot, verified.manifest())?;
        Ok(())
    }

    fn descriptor_intact(
        &self,
        slot: Slot,
        descriptor: SlotDescriptor,
    ) -> Result<bool, BootAdapterError<F::Error>> {
        if !descriptor.valid {
            return Ok(false);
        }
        let region = self.layout.slot(slot);
        if descriptor.plan.load_addr != region.start + SLOT_HEADER_BYTES
            || !region.contains(descriptor.plan.load_addr, descriptor.plan.image_len)
        {
            return Ok(false);
        }
        Ok(
            self.hash_media(descriptor.plan.load_addr, descriptor.plan.image_len)?
                == descriptor.measurement,
        )
    }

    fn descriptor_authorized<const N: usize>(
        &self,
        slot: Slot,
        descriptor: SlotDescriptor,
        keys: &PinnedKeyPolicy<N>,
        vectors: BootVectorPolicy,
        rollback_floor: u32,
    ) -> Result<bool, BootAdapterError<F::Error>> {
        if !self.descriptor_intact(slot, descriptor)? {
            return Ok(false);
        }
        let Some(manifest) = self.read_slot_manifest(slot)? else {
            return Ok(false);
        };
        if manifest.signing_digest() != descriptor.manifest_digest
            || manifest.measurement != descriptor.measurement
            || manifest.version != descriptor.plan.version
            || manifest.image_len != descriptor.plan.image_len
            || manifest.load_addr != descriptor.plan.load_addr
            || manifest.entry_addr != descriptor.plan.entry_addr
            || manifest.stack_top != descriptor.plan.stack_top
        {
            return Ok(false);
        }
        Ok(verify_signed_measurement(
            descriptor.measurement,
            &manifest,
            keys,
            vectors,
            rollback_floor,
        )
        .is_ok())
    }

    fn write_slot_manifest(
        &mut self,
        slot: Slot,
        manifest: SignedImageManifest,
    ) -> Result<(), BootAdapterError<F::Error>> {
        let region = self.layout.slot(slot);
        let header = encode_slot_header(manifest);
        let width = self.flash.program_size() as usize;
        if width == 0 || SLOT_HEADER_MARKER_OFFSET % width != 0 || 4usize % width != 0 {
            return Err(BootAdapterError::Layout(BootLayoutError::Misaligned));
        }
        for offset in (0..SLOT_HEADER_MARKER_OFFSET).step_by(width) {
            self.flash
                .program(
                    region.start + offset as u32,
                    &header[offset..offset + width],
                )
                .map_err(BootAdapterError::Media)?;
        }
        self.flash
            .program(
                region.start + SLOT_HEADER_MARKER_OFFSET as u32,
                &SLOT_HEADER_COMMITTED.to_le_bytes(),
            )
            .map_err(BootAdapterError::Media)?;
        if self.read_slot_manifest(slot)? != Some(manifest) {
            return Err(BootAdapterError::ReadbackMismatch);
        }
        Ok(())
    }

    fn read_slot_manifest(
        &self,
        slot: Slot,
    ) -> Result<Option<SignedImageManifest>, BootAdapterError<F::Error>> {
        let mut header = [0u8; SLOT_HEADER_BYTES as usize];
        self.flash
            .read(self.layout.slot(slot).start, &mut header)
            .map_err(BootAdapterError::Media)?;
        Ok(decode_slot_header(&header))
    }

    fn hash_media(&self, start: u32, len: u32) -> Result<[u8; 32], BootAdapterError<F::Error>> {
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 64];
        let mut offset = 0u32;
        while offset < len {
            let take = (len - offset).min(buffer.len() as u32) as usize;
            self.flash
                .read(start + offset, &mut buffer[..take])
                .map_err(BootAdapterError::Media)?;
            hash.update(&buffer[..take]);
            offset += take as u32;
        }
        Ok(hash.finalize())
    }

    fn read_latest(&self) -> Result<Option<(usize, JournalState)>, BootAdapterError<F::Error>> {
        let mut latest: Option<(usize, JournalState)> = None;
        for (index, page) in self.layout.journal.iter().enumerate() {
            let mut bytes = [0u8; RECORD_BYTES];
            self.flash
                .read(page.start, &mut bytes)
                .map_err(BootAdapterError::Media)?;
            let Some(state) = decode_record(&bytes) else {
                continue;
            };
            if latest
                .as_ref()
                .is_none_or(|(_, current)| state.generation > current.generation)
            {
                latest = Some((index, state));
            }
        }
        Ok(latest)
    }

    fn commit_state(
        &mut self,
        current_page: Option<usize>,
        state: JournalState,
    ) -> Result<(), BootAdapterError<F::Error>> {
        if let Some((_, current)) = self.read_latest()? {
            if state.generation <= current.generation
                || state.confirmed_version < current.confirmed_version
            {
                return Err(BootAdapterError::JournalRollback);
            }
        }
        let target = current_page.map_or(0, |page| page ^ 1);
        let page = self.layout.journal[target];
        let bytes = encode_record(state);
        self.flash
            .erase(page.start, page.len)
            .map_err(BootAdapterError::Media)?;
        let width = self.flash.program_size() as usize;
        if width == 0 || RECORD_BODY_BYTES % width != 0 || 4usize % width != 0 {
            return Err(BootAdapterError::Layout(BootLayoutError::Misaligned));
        }
        for offset in (0..RECORD_BODY_BYTES).step_by(width) {
            self.flash
                .program(page.start + offset as u32, &bytes[offset..offset + width])
                .map_err(BootAdapterError::Media)?;
        }
        let mut readback = [0u8; RECORD_BODY_BYTES];
        self.flash
            .read(page.start, &mut readback)
            .map_err(BootAdapterError::Media)?;
        if readback != bytes[..RECORD_BODY_BYTES] {
            return Err(BootAdapterError::ReadbackMismatch);
        }
        self.flash
            .program(
                page.start + RECORD_MARKER_OFFSET as u32,
                &RECORD_COMMITTED.to_le_bytes(),
            )
            .map_err(BootAdapterError::Media)?;
        let mut committed = [0u8; RECORD_BYTES];
        self.flash
            .read(page.start, &mut committed)
            .map_err(BootAdapterError::Media)?;
        if decode_record(&committed) != Some(state) {
            return Err(BootAdapterError::ReadbackMismatch);
        }
        Ok(())
    }
}

const fn slot_index(slot: Slot) -> usize {
    match slot {
        Slot::A => 0,
        Slot::B => 1,
    }
}

const fn next_generation<E>(generation: u32) -> Result<u32, BootAdapterError<E>> {
    match generation.checked_add(1) {
        Some(next) => Ok(next),
        None => Err(BootAdapterError::JournalRollback),
    }
}

fn descriptor_from_verified(verified: &VerifiedSignedImage) -> SlotDescriptor {
    SlotDescriptor {
        valid: true,
        plan: verified.plan(),
        measurement: verified.image_measurement(),
        manifest_digest: verified.manifest_digest(),
    }
}

fn selection(state: JournalState, slot: Slot, kind: BootSelectionKind) -> BoardBootSelection {
    BoardBootSelection {
        slot,
        plan: state.slots[slot_index(slot)].plan,
        kind,
        generation: state.generation,
    }
}

fn encode_record(state: JournalState) -> [u8; RECORD_BYTES] {
    let mut out = [0xFFu8; RECORD_BYTES];
    put_u32(&mut out, 0, RECORD_MAGIC);
    put_u32(&mut out, 4, RECORD_SCHEMA);
    put_u32(&mut out, 8, state.generation);
    put_u32(&mut out, 12, slot_index(state.active) as u32);
    put_u32(&mut out, 16, state.confirmed_version);
    put_u32(
        &mut out,
        20,
        state
            .pending
            .map_or(u32::MAX, |slot| slot_index(slot) as u32),
    );
    put_u32(&mut out, 24, u32::from(state.trial_attempted));
    for (index, descriptor) in state.slots.iter().enumerate() {
        let base = 32 + index * 88;
        put_u32(&mut out, base, u32::from(descriptor.valid));
        put_u32(&mut out, base + 4, descriptor.plan.version);
        put_u32(&mut out, base + 8, descriptor.plan.image_len);
        put_u32(&mut out, base + 12, descriptor.plan.load_addr);
        put_u32(&mut out, base + 16, descriptor.plan.entry_addr);
        put_u32(&mut out, base + 20, descriptor.plan.stack_top);
        out[base + 24..base + 56].copy_from_slice(&descriptor.measurement);
        out[base + 56..base + 88].copy_from_slice(&descriptor.manifest_digest);
    }
    let digest = sha256(&out[..RECORD_DIGEST_OFFSET]);
    out[RECORD_DIGEST_OFFSET..RECORD_DIGEST_OFFSET + 32].copy_from_slice(&digest);
    out
}

fn decode_record(bytes: &[u8; RECORD_BYTES]) -> Option<JournalState> {
    if get_u32(bytes, RECORD_MARKER_OFFSET) != RECORD_COMMITTED
        || get_u32(bytes, 0) != RECORD_MAGIC
        || get_u32(bytes, 4) != RECORD_SCHEMA
        || sha256(&bytes[..RECORD_DIGEST_OFFSET])
            != bytes[RECORD_DIGEST_OFFSET..RECORD_DIGEST_OFFSET + 32]
    {
        return None;
    }
    let active = decode_slot(get_u32(bytes, 12))?;
    let pending_raw = get_u32(bytes, 20);
    let pending = if pending_raw == u32::MAX {
        None
    } else {
        Some(decode_slot(pending_raw)?)
    };
    let trial = get_u32(bytes, 24);
    if trial > 1 {
        return None;
    }
    let mut slots = [SlotDescriptor::EMPTY; 2];
    for (index, descriptor) in slots.iter_mut().enumerate() {
        let base = 32 + index * 88;
        let valid = get_u32(bytes, base);
        if valid > 1 {
            return None;
        }
        let mut measurement = [0u8; 32];
        measurement.copy_from_slice(&bytes[base + 24..base + 56]);
        let mut manifest_digest = [0u8; 32];
        manifest_digest.copy_from_slice(&bytes[base + 56..base + 88]);
        *descriptor = SlotDescriptor {
            valid: valid == 1,
            plan: VerifiedBootPlan {
                version: get_u32(bytes, base + 4),
                image_len: get_u32(bytes, base + 8),
                load_addr: get_u32(bytes, base + 12),
                entry_addr: get_u32(bytes, base + 16),
                stack_top: get_u32(bytes, base + 20),
            },
            measurement,
            manifest_digest,
        };
    }
    Some(JournalState {
        generation: get_u32(bytes, 8),
        active,
        confirmed_version: get_u32(bytes, 16),
        pending,
        trial_attempted: trial == 1,
        slots,
    })
}

fn encode_slot_header(manifest: SignedImageManifest) -> [u8; SLOT_HEADER_BYTES as usize] {
    let mut out = [0xFFu8; SLOT_HEADER_BYTES as usize];
    put_u32(&mut out, 0, SLOT_HEADER_MAGIC);
    put_u32(&mut out, 4, SLOT_HEADER_SCHEMA);
    put_u32(&mut out, 8, manifest.key_id);
    put_u32(&mut out, 12, manifest.version);
    put_u32(&mut out, 16, manifest.image_len);
    put_u32(&mut out, 20, manifest.load_addr);
    put_u32(&mut out, 24, manifest.entry_addr);
    put_u32(&mut out, 28, manifest.stack_top);
    out[32..64].copy_from_slice(&manifest.measurement);
    out[64..128].copy_from_slice(&manifest.signature);
    out[128..160].copy_from_slice(&manifest.signing_digest());
    let digest = sha256(&out[..SLOT_HEADER_DIGEST_OFFSET]);
    out[SLOT_HEADER_DIGEST_OFFSET..SLOT_HEADER_DIGEST_OFFSET + 32].copy_from_slice(&digest);
    out
}

fn decode_slot_header(bytes: &[u8; SLOT_HEADER_BYTES as usize]) -> Option<SignedImageManifest> {
    if get_u32(bytes, SLOT_HEADER_MARKER_OFFSET) != SLOT_HEADER_COMMITTED
        || get_u32(bytes, 0) != SLOT_HEADER_MAGIC
        || get_u32(bytes, 4) != SLOT_HEADER_SCHEMA
        || sha256(&bytes[..SLOT_HEADER_DIGEST_OFFSET])
            != bytes[SLOT_HEADER_DIGEST_OFFSET..SLOT_HEADER_DIGEST_OFFSET + 32]
    {
        return None;
    }
    let mut measurement = [0u8; 32];
    measurement.copy_from_slice(&bytes[32..64]);
    let mut signature = [0u8; 64];
    signature.copy_from_slice(&bytes[64..128]);
    let manifest = SignedImageManifest {
        key_id: get_u32(bytes, 8),
        version: get_u32(bytes, 12),
        image_len: get_u32(bytes, 16),
        load_addr: get_u32(bytes, 20),
        entry_addr: get_u32(bytes, 24),
        stack_top: get_u32(bytes, 28),
        measurement,
        signature,
    };
    if manifest.signing_digest() != bytes[128..160] {
        return None;
    }
    Some(manifest)
}

fn decode_slot(value: u32) -> Option<Slot> {
    match value {
        0 => Some(Slot::A),
        1 => Some(Slot::B),
        _ => None,
    }
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn get_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().ok().unwrap_or([0; 4]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{verify_signed_boot, BootVectorPolicy, PinnedKeyPolicy, SignedImageManifest};
    use ed25519_dalek::{Signer, SigningKey};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum MediaError {
        Cut,
        Bounds,
        BitSet,
    }

    #[derive(Clone)]
    struct MemoryFlash {
        bytes: [u8; 4096],
        mutations: usize,
        cut_after: Option<usize>,
    }

    impl MemoryFlash {
        fn new() -> Self {
            Self {
                bytes: [0xFF; 4096],
                mutations: 0,
                cut_after: None,
            }
        }

        fn mutate(&mut self) -> Result<(), MediaError> {
            if self.cut_after == Some(self.mutations) {
                return Err(MediaError::Cut);
            }
            self.mutations += 1;
            Ok(())
        }
    }

    impl BootFlash for MemoryFlash {
        type Error = MediaError;

        fn capacity(&self) -> u32 {
            self.bytes.len() as u32
        }

        fn erase_size(&self) -> u32 {
            256
        }

        fn program_size(&self) -> u32 {
            4
        }

        fn read(&self, address: u32, output: &mut [u8]) -> Result<(), Self::Error> {
            let start = address as usize;
            let end = start.checked_add(output.len()).ok_or(MediaError::Bounds)?;
            let source = self.bytes.get(start..end).ok_or(MediaError::Bounds)?;
            output.copy_from_slice(source);
            Ok(())
        }

        fn erase(&mut self, address: u32, len: u32) -> Result<(), Self::Error> {
            self.mutate()?;
            let range = self
                .bytes
                .get_mut(address as usize..(address + len) as usize)
                .ok_or(MediaError::Bounds)?;
            range.fill(0xFF);
            Ok(())
        }

        fn program(&mut self, address: u32, bytes: &[u8]) -> Result<(), Self::Error> {
            self.mutate()?;
            let target = self
                .bytes
                .get_mut(address as usize..address as usize + bytes.len())
                .ok_or(MediaError::Bounds)?;
            if target
                .iter()
                .zip(bytes)
                .any(|(old, new)| (*old & *new) != *new)
            {
                return Err(MediaError::BitSet);
            }
            for (old, new) in target.iter_mut().zip(bytes) {
                *old &= *new;
            }
            Ok(())
        }
    }

    fn layout() -> BoardBootLayout<1> {
        BoardBootLayout {
            slots: [FlashRange::new(0, 1024), FlashRange::new(1024, 1024)],
            journal: [FlashRange::new(2048, 256), FlashRange::new(2304, 256)],
            protected: [FlashRange::new(3072, 1024)],
        }
    }

    fn verified(image: &[u8], slot: Slot, version: u32) -> VerifiedSignedImage {
        let signing = SigningKey::from_bytes(&[7; 32]);
        let keys = keys();
        let load_addr = layout().slot(slot).start + SLOT_HEADER_BYTES;
        let mut manifest = SignedImageManifest {
            key_id: 4,
            version,
            image_len: image.len() as u32,
            load_addr,
            entry_addr: load_addr | 1,
            stack_top: 0x2000_1000,
            measurement: sha256(image),
            signature: [0; 64],
        };
        manifest.signature = signing.sign(&manifest.signing_digest()).to_bytes();
        verify_signed_boot(
            image,
            &manifest,
            &keys,
            BootVectorPolicy::cortex_m(0, 2048, 0x2000_0000, 0x2000_2000),
            0,
        )
        .unwrap()
    }

    fn keys() -> PinnedKeyPolicy<1> {
        let signing = SigningKey::from_bytes(&[7; 32]);
        let mut keys = PinnedKeyPolicy::<1>::new();
        assert!(keys.pin(4, signing.verifying_key().to_bytes()));
        keys
    }

    fn vectors() -> BootVectorPolicy {
        BootVectorPolicy::cortex_m(0, 2048, 0x2000_0000, 0x2000_2000)
    }

    fn initialized() -> BoardBootAdapter<MemoryFlash, 1> {
        let image = [0xA5; 128];
        let token = verified(&image, Slot::A, 1);
        let mut adapter = BoardBootAdapter::new(MemoryFlash::new(), layout()).unwrap();
        adapter
            .initialize_active(Slot::A, &image, &token, &keys(), vectors())
            .unwrap();
        adapter
    }

    #[test]
    fn layout_rejects_every_bootloader_overlap_before_mutation() {
        let mut bad = layout();
        bad.slots[1] = FlashRange::new(3000, 512);
        assert_eq!(bad.validate(4096, 256, 4), Err(BootLayoutError::Misaligned));
        bad.slots[1] = FlashRange::new(3072, 512);
        assert_eq!(
            bad.validate(4096, 256, 4),
            Err(BootLayoutError::ProtectedOverlap)
        );
    }

    #[test]
    fn trial_confirm_and_reset_without_confirm_reverts() {
        let mut adapter = initialized();
        let image_b = [0x5A; 128];
        let token_b = verified(&image_b, Slot::B, 2);
        adapter.stage(Slot::B, &image_b, &token_b).unwrap();
        let trial = adapter.select_boot(&keys(), vectors()).unwrap();
        assert_eq!(
            (trial.slot, trial.kind),
            (Slot::B, BootSelectionKind::Trial)
        );
        let reverted = adapter.select_boot(&keys(), vectors()).unwrap();
        assert_eq!(
            (reverted.slot, reverted.kind),
            (Slot::A, BootSelectionKind::Reverted)
        );

        adapter.stage(Slot::B, &image_b, &token_b).unwrap();
        assert_eq!(
            adapter.select_boot(&keys(), vectors()).unwrap().slot,
            Slot::B
        );
        adapter.confirm(2, &keys(), vectors()).unwrap();
        let confirmed = adapter.select_boot(&keys(), vectors()).unwrap();
        assert_eq!(
            (confirmed.slot, confirmed.kind),
            (Slot::B, BootSelectionKind::Confirmed)
        );
    }

    #[test]
    fn verified_bytes_cannot_be_substituted_and_active_slot_is_immutable() {
        let mut adapter = initialized();
        let image_b = [0x5A; 128];
        let token_b = verified(&image_b, Slot::B, 2);
        let forged = [0x5B; 128];
        assert_eq!(
            adapter.stage(Slot::B, &forged, &token_b),
            Err(BootAdapterError::ImageDoesNotMatchToken)
        );
        assert_eq!(
            adapter.stage(Slot::A, &image_b, &token_b),
            Err(BootAdapterError::ActiveSlotWrite)
        );
    }

    #[test]
    fn reset_time_selection_rechecks_the_ed25519_signature() {
        let mut adapter = initialized();
        let image_b = [0x5A; 128];
        let token_b = verified(&image_b, Slot::B, 2);
        adapter.stage(Slot::B, &image_b, &token_b).unwrap();

        // Model an attacker who can rewrite both unkeyed checksums but cannot
        // produce a new Ed25519 signature.
        let mut forged = token_b.manifest();
        forged.entry_addr += 2;
        let mut header = encode_slot_header(forged);
        put_u32(
            &mut header,
            SLOT_HEADER_MARKER_OFFSET,
            SLOT_HEADER_COMMITTED,
        );
        adapter.flash.bytes[1024..1024 + header.len()].copy_from_slice(&header);
        let descriptor = SlotDescriptor {
            valid: true,
            plan: VerifiedBootPlan {
                entry_addr: forged.entry_addr,
                ..token_b.plan()
            },
            measurement: forged.measurement,
            manifest_digest: forged.signing_digest(),
        };
        assert!(!adapter
            .descriptor_authorized(Slot::B, descriptor, &keys(), vectors(), 1)
            .unwrap());
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "exhaustive native crash-cut gate; representative signed transitions still run under Miri"
    )]
    fn every_journal_power_cut_keeps_the_previous_record_bootable() {
        let baseline = initialized();
        let image_b = [0x5A; 128];
        let token_b = verified(&image_b, Slot::B, 2);

        let mut measuring = baseline;
        let before = measuring.flash.mutations;
        measuring.stage(Slot::B, &image_b, &token_b).unwrap();
        let mutations = measuring.flash.mutations - before;

        for cut in 0..mutations {
            let mut adapter = initialized();
            let start = adapter.flash.mutations;
            adapter.flash.cut_after = Some(start + cut);
            let _ = adapter.stage(Slot::B, &image_b, &token_b);
            adapter.flash.cut_after = None;
            let selected = adapter.select_boot(&keys(), vectors()).unwrap();
            assert!(
                selected.slot == Slot::A || selected.slot == Slot::B,
                "cut {cut} lost both images"
            );
            if selected.slot == Slot::B {
                assert_eq!(selected.kind, BootSelectionKind::Trial);
            }
        }
    }
}
