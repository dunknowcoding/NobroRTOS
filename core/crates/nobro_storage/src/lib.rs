//! Power-fail-safe, wear-leveled key-value flash store.
#![cfg_attr(not(test), no_std)]

/// Minimal fallible flash abstraction. Implementations must preserve normal flash
/// semantics: erase to all ones and program bits only from one to zero.
pub trait Flash {
    type Error;
    const WORDS: usize;
    fn erase(&mut self, page: usize) -> Result<(), Self::Error>;
    fn write_word(&mut self, page: usize, word: usize, val: u32) -> Result<(), Self::Error>;
    fn read_word(&self, page: usize, word: usize) -> u32;
}

/// Optional erase-health accounting supplied by a concrete flash provider.
///
/// The storage algorithms remain usable with [`Flash`] alone. This extension
/// makes rated endurance and known-bad pages observable without inventing values
/// for devices that cannot report them.
pub trait FlashEndurance: Flash {
    fn erase_cycles(&self, page: usize) -> u32;
    fn page_healthy(&self, page: usize) -> bool;
    fn rated_erase_cycles(&self) -> Option<u32>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnduranceReceipt {
    pub page_erases: [u32; 2],
    pub page_healthy: [bool; 2],
    pub erase_imbalance: u32,
    pub rated_erase_cycles: Option<u32>,
    pub minimum_remaining_cycles: Option<u32>,
}

fn endurance_receipt<F: FlashEndurance>(flash: &F) -> EnduranceReceipt {
    let page_erases = [flash.erase_cycles(0), flash.erase_cycles(1)];
    let rated = flash.rated_erase_cycles();
    EnduranceReceipt {
        page_erases,
        page_healthy: [flash.page_healthy(0), flash.page_healthy(1)],
        erase_imbalance: page_erases[0].abs_diff(page_erases[1]),
        rated_erase_cycles: rated,
        minimum_remaining_cycles: rated
            .map(|cycles| cycles.saturating_sub(page_erases[0].max(page_erases[1]))),
    }
}

const BLANK: u32 = u32::MAX;
const PAGE_MAGIC: u32 = 0x4E4B_5632; // "NKV2"
const PAGE_COMMITTED: u32 = 0x434F_4D54; // "COMT", written last
const REC_TAG: u16 = 0x4B57;
const HEADER_WORDS: usize = 3; // magic, generation, commit
const RECORD_WORDS: usize = 3; // tagged key, value, checksum written last

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KvError<E> {
    Full,
    Flash(E),
}

pub struct KvStore<F: Flash> {
    flash: F,
    active: usize,
    next_word: usize,
    generation: u32,
}

const BLOB_MAGIC: u32 = 0x4E42_4C42; // "NBLB"
const BLOB_COMMITTED: u32 = 0x424C_4F42; // "BLOB", written last
const BLOB_HEADER_WORDS: usize = 5; // magic, generation, byte length, checksum, commit

/// Alternating-page transactional storage for one bounded byte image.
///
/// The inactive page is erased and populated before its commit word is written. Mount
/// ignores every uncommitted or checksum-invalid page, so a reset at any program/erase
/// boundary exposes either the complete old image or the complete new image.
pub struct BlobStore<F: Flash> {
    flash: F,
    active: Option<usize>,
    generation: u32,
    len: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlobCommitReceipt {
    pub generation: u32,
    pub active_page: usize,
    pub image_bytes: usize,
    pub program_words: usize,
    pub erase_attempts: u8,
    /// The new image is committed even when obsolete-page cleanup must be
    /// retried by a later maintenance operation.
    pub cleanup_pending: bool,
}

impl<F: Flash> BlobStore<F> {
    pub const fn capacity() -> usize {
        F::WORDS.saturating_sub(BLOB_HEADER_WORDS) * 4
    }

    fn flash<T>(result: Result<T, F::Error>) -> Result<T, KvError<F::Error>> {
        result.map_err(KvError::Flash)
    }

    fn checksum_bytes(generation: u32, len: usize, bytes: impl Iterator<Item = u8>) -> u32 {
        let mut hash = 0x811C_9DC5u32;
        for byte in generation
            .to_le_bytes()
            .into_iter()
            .chain((len as u32).to_le_bytes())
            .chain(bytes)
        {
            hash = (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193);
        }
        hash
    }

    fn valid_page(flash: &F, page: usize) -> Option<(u32, usize)> {
        if F::WORDS < BLOB_HEADER_WORDS
            || flash.read_word(page, 0) != BLOB_MAGIC
            || flash.read_word(page, 4) != BLOB_COMMITTED
        {
            return None;
        }
        let generation = flash.read_word(page, 1);
        let len = flash.read_word(page, 2) as usize;
        if len > Self::capacity() {
            return None;
        }
        let bytes = (0..len).map(|offset| {
            let word = flash.read_word(page, BLOB_HEADER_WORDS + offset / 4);
            word.to_le_bytes()[offset % 4]
        });
        (flash.read_word(page, 3) == Self::checksum_bytes(generation, len, bytes))
            .then_some((generation, len))
    }

    pub fn mount(flash: F) -> Self {
        let p0 = Self::valid_page(&flash, 0);
        let p1 = Self::valid_page(&flash, 1);
        let selected = match (p0, p1) {
            (None, None) => None,
            (Some(a), None) => Some((0, a)),
            (None, Some(b)) => Some((1, b)),
            (Some(a), Some(b)) => {
                if KvStore::<F>::generation_is_newer(b.0, a.0) {
                    Some((1, b))
                } else {
                    Some((0, a))
                }
            }
        };
        let (active, generation, len) = selected
            .map(|(page, (generation, len))| (Some(page), generation, len))
            .unwrap_or((None, 0, 0));
        Self {
            flash,
            active,
            generation,
            len,
        }
    }

    pub fn read(&self, out: &mut [u8]) -> Result<Option<usize>, KvError<F::Error>> {
        let Some(page) = self.active else {
            return Ok(None);
        };
        if out.len() < self.len {
            return Err(KvError::Full);
        }
        for (offset, byte) in out[..self.len].iter_mut().enumerate() {
            let word = self.flash.read_word(page, BLOB_HEADER_WORDS + offset / 4);
            *byte = word.to_le_bytes()[offset % 4];
        }
        Ok(Some(self.len))
    }

    pub fn replace(&mut self, image: &[u8]) -> Result<(), KvError<F::Error>> {
        self.replace_with_receipt(image).map(|_| ())
    }

    pub fn replace_with_receipt(
        &mut self,
        image: &[u8],
    ) -> Result<BlobCommitReceipt, KvError<F::Error>> {
        if image.len() > Self::capacity() || image.len() > u32::MAX as usize {
            return Err(KvError::Full);
        }
        let new = self.active.map_or(0, |page| 1 - page);
        let generation = self.generation.wrapping_add(1);
        let checksum = Self::checksum_bytes(generation, image.len(), image.iter().copied());

        Self::flash(self.flash.erase(new))?;
        Self::flash(self.flash.write_word(new, 0, BLOB_MAGIC))?;
        Self::flash(self.flash.write_word(new, 1, generation))?;
        Self::flash(self.flash.write_word(new, 2, image.len() as u32))?;
        Self::flash(self.flash.write_word(new, 3, checksum))?;
        for (word_offset, chunk) in image.chunks(4).enumerate() {
            let mut bytes = [0xFF; 4];
            bytes[..chunk.len()].copy_from_slice(chunk);
            Self::flash(self.flash.write_word(
                new,
                BLOB_HEADER_WORDS + word_offset,
                u32::from_le_bytes(bytes),
            ))?;
        }
        Self::flash(self.flash.write_word(new, 4, BLOB_COMMITTED))?;

        let old = self.active;
        self.active = Some(new);
        self.generation = generation;
        self.len = image.len();
        let cleanup_pending = old.is_some_and(|old| self.flash.erase(old).is_err());
        Ok(BlobCommitReceipt {
            generation,
            active_page: new,
            image_bytes: image.len(),
            program_words: BLOB_HEADER_WORDS + image.len().div_ceil(4),
            erase_attempts: 1 + u8::from(old.is_some()),
            cleanup_pending,
        })
    }

    pub const fn generation(&self) -> u32 {
        self.generation
    }

    pub const fn active_page(&self) -> Option<usize> {
        self.active
    }

    /// Erases a still-valid obsolete page after a committed replacement.
    ///
    /// Returns `Ok(true)` only when an obsolete committed page was reclaimed.
    /// Blank, corrupt, or already-clean pages are left untouched so maintenance
    /// does not consume an erase cycle unnecessarily.
    pub fn retry_obsolete_cleanup(&mut self) -> Result<bool, KvError<F::Error>> {
        let Some(active) = self.active else {
            return Ok(false);
        };
        let obsolete = 1 - active;
        if Self::valid_page(&self.flash, obsolete).is_none() {
            return Ok(false);
        }
        Self::flash(self.flash.erase(obsolete))?;
        Ok(true)
    }

    pub fn into_flash(self) -> F {
        self.flash
    }
}

impl<F: FlashEndurance> BlobStore<F> {
    pub fn endurance(&self) -> EnduranceReceipt {
        endurance_receipt(&self.flash)
    }
}

const FILE_SYSTEM_MAGIC: u32 = 0x4E46_5331; // "NFS1"
const FILE_SYSTEM_VERSION: u8 = 1;
const FILE_SYSTEM_HEADER_BYTES: usize = 13;
const FILE_RECORD_HEADER_BYTES: usize = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileSystemError<E> {
    InvalidConfig,
    InvalidName,
    WorkspaceTooSmall,
    Full,
    NotFound,
    Corrupt,
    Flash(E),
}

impl<E> From<KvError<E>> for FileSystemError<E> {
    fn from(error: KvError<E>) -> Self {
        match error {
            KvError::Full => Self::WorkspaceTooSmall,
            KvError::Flash(error) => Self::Flash(error),
        }
    }
}

pub struct FileSystemMountError<F: Flash> {
    flash: F,
    error: FileSystemError<F::Error>,
}

impl<F: Flash> FileSystemMountError<F> {
    pub const fn error(&self) -> &FileSystemError<F::Error> {
        &self.error
    }

    pub fn into_flash(self) -> F {
        self.flash
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileMetadata<'a> {
    pub name: &'a [u8],
    pub len: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileCommitReceipt {
    pub generation: u32,
    /// The new image is committed even when erasing the obsolete page must be
    /// retried on a later successful write.
    pub cleanup_pending: bool,
}

#[derive(Clone, Copy)]
struct FileEntry<const NAME_BYTES: usize, const DATA_BYTES: usize> {
    used: bool,
    name_len: usize,
    data_len: usize,
    name: [u8; NAME_BYTES],
    data: [u8; DATA_BYTES],
}

impl<const NAME_BYTES: usize, const DATA_BYTES: usize> FileEntry<NAME_BYTES, DATA_BYTES> {
    const EMPTY: Self = Self {
        used: false,
        name_len: 0,
        data_len: 0,
        name: [0; NAME_BYTES],
        data: [0; DATA_BYTES],
    };

    fn name(&self) -> &[u8] {
        &self.name[..self.name_len]
    }

    fn data(&self) -> &[u8] {
        &self.data[..self.data_len]
    }
}

/// Fixed-capacity transactional filesystem over [`BlobStore`].
///
/// Names, file data, and the directory are stored in compile-time arrays. Each
/// mutation serializes into caller-owned scratch storage and atomically replaces
/// the complete image, so a power loss exposes either the old or new directory.
pub struct AtomicFileSystem<
    F: Flash,
    const FILES: usize,
    const NAME_BYTES: usize,
    const DATA_BYTES: usize,
> {
    store: BlobStore<F>,
    entries: [FileEntry<NAME_BYTES, DATA_BYTES>; FILES],
}

impl<F: Flash, const FILES: usize, const NAME_BYTES: usize, const DATA_BYTES: usize>
    AtomicFileSystem<F, FILES, NAME_BYTES, DATA_BYTES>
{
    pub const fn image_bytes() -> usize {
        FILE_SYSTEM_HEADER_BYTES.saturating_add(
            FILES.saturating_mul(
                FILE_RECORD_HEADER_BYTES
                    .saturating_add(NAME_BYTES)
                    .saturating_add(DATA_BYTES),
            ),
        )
    }

    fn config_valid() -> bool {
        FILES != 0
            && FILES <= u16::MAX as usize
            && NAME_BYTES != 0
            && NAME_BYTES <= u16::MAX as usize
            && DATA_BYTES <= u32::MAX as usize
            && Self::image_bytes() <= BlobStore::<F>::capacity()
    }

    fn valid_name(name: &[u8]) -> bool {
        !name.is_empty()
            && name.len() <= NAME_BYTES
            && name
                .iter()
                .all(|byte| *byte >= 0x20 && *byte != b'/' && *byte != b'\\')
    }

    fn put_u16(output: &mut [u8], offset: &mut usize, value: u16) {
        output[*offset..*offset + 2].copy_from_slice(&value.to_le_bytes());
        *offset += 2;
    }

    fn put_u32(output: &mut [u8], offset: &mut usize, value: u32) {
        output[*offset..*offset + 4].copy_from_slice(&value.to_le_bytes());
        *offset += 4;
    }

    fn take_u16(input: &[u8], offset: &mut usize) -> u16 {
        let value = u16::from_le_bytes([input[*offset], input[*offset + 1]]);
        *offset += 2;
        value
    }

    fn take_u32(input: &[u8], offset: &mut usize) -> u32 {
        let value = u32::from_le_bytes([
            input[*offset],
            input[*offset + 1],
            input[*offset + 2],
            input[*offset + 3],
        ]);
        *offset += 4;
        value
    }

    fn decode(
        input: &[u8],
    ) -> Result<[FileEntry<NAME_BYTES, DATA_BYTES>; FILES], FileSystemError<F::Error>> {
        if input.len() != Self::image_bytes() {
            return Err(FileSystemError::Corrupt);
        }
        let mut offset = 0;
        let magic = Self::take_u32(input, &mut offset);
        let version = input[offset];
        offset += 1;
        let files = usize::from(Self::take_u16(input, &mut offset));
        let name_bytes = usize::from(Self::take_u16(input, &mut offset));
        let data_bytes = Self::take_u32(input, &mut offset) as usize;
        if magic != FILE_SYSTEM_MAGIC
            || version != FILE_SYSTEM_VERSION
            || files != FILES
            || name_bytes != NAME_BYTES
            || data_bytes != DATA_BYTES
        {
            return Err(FileSystemError::Corrupt);
        }

        let mut entries = [FileEntry::EMPTY; FILES];
        let mut index = 0;
        while index < FILES {
            let used = input[offset];
            offset += 1;
            let name_len = usize::from(Self::take_u16(input, &mut offset));
            let data_len = Self::take_u32(input, &mut offset) as usize;
            let name_start = offset;
            offset += NAME_BYTES;
            let data_start = offset;
            offset += DATA_BYTES;
            if used > 1 || name_len > NAME_BYTES || data_len > DATA_BYTES {
                return Err(FileSystemError::Corrupt);
            }
            if used == 0 {
                if name_len != 0 || data_len != 0 {
                    return Err(FileSystemError::Corrupt);
                }
            } else {
                let name = &input[name_start..name_start + name_len];
                if !Self::valid_name(name) {
                    return Err(FileSystemError::Corrupt);
                }
                entries[index].used = true;
                entries[index].name_len = name_len;
                entries[index].data_len = data_len;
                entries[index].name[..name_len].copy_from_slice(name);
                entries[index].data[..data_len]
                    .copy_from_slice(&input[data_start..data_start + data_len]);
                if entries[..index]
                    .iter()
                    .any(|entry| entry.used && entry.name() == name)
                {
                    return Err(FileSystemError::Corrupt);
                }
            }
            index += 1;
        }
        Ok(entries)
    }

    fn encode(&self, output: &mut [u8]) -> Result<usize, FileSystemError<F::Error>> {
        let size = Self::image_bytes();
        if output.len() < size {
            return Err(FileSystemError::WorkspaceTooSmall);
        }
        output[..size].fill(0);
        let mut offset = 0;
        Self::put_u32(output, &mut offset, FILE_SYSTEM_MAGIC);
        output[offset] = FILE_SYSTEM_VERSION;
        offset += 1;
        Self::put_u16(output, &mut offset, FILES as u16);
        Self::put_u16(output, &mut offset, NAME_BYTES as u16);
        Self::put_u32(output, &mut offset, DATA_BYTES as u32);
        for entry in &self.entries {
            output[offset] = u8::from(entry.used);
            offset += 1;
            Self::put_u16(output, &mut offset, entry.name_len as u16);
            Self::put_u32(output, &mut offset, entry.data_len as u32);
            output[offset..offset + entry.name_len].copy_from_slice(entry.name());
            offset += NAME_BYTES;
            output[offset..offset + entry.data_len].copy_from_slice(entry.data());
            offset += DATA_BYTES;
        }
        Ok(size)
    }

    pub fn mount(flash: F, scratch: &mut [u8]) -> Result<Self, FileSystemMountError<F>> {
        let store = BlobStore::mount(flash);
        if !Self::config_valid() {
            return Err(FileSystemMountError {
                flash: store.into_flash(),
                error: FileSystemError::InvalidConfig,
            });
        }
        let entries = match store.read(scratch) {
            Ok(None) => [FileEntry::EMPTY; FILES],
            Ok(Some(len)) => match Self::decode(&scratch[..len]) {
                Ok(entries) => entries,
                Err(error) => {
                    return Err(FileSystemMountError {
                        flash: store.into_flash(),
                        error,
                    });
                }
            },
            Err(error) => {
                return Err(FileSystemMountError {
                    flash: store.into_flash(),
                    error: error.into(),
                });
            }
        };
        Ok(Self { store, entries })
    }

    fn find(&self, name: &[u8]) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.used && entry.name() == name)
    }

    fn commit(
        &mut self,
        scratch: &mut [u8],
    ) -> Result<FileCommitReceipt, FileSystemError<F::Error>> {
        let len = self.encode(scratch)?;
        self.store
            .replace_with_receipt(&scratch[..len])
            .map(|receipt| FileCommitReceipt {
                generation: receipt.generation,
                cleanup_pending: receipt.cleanup_pending,
            })
            .map_err(Into::into)
    }

    pub fn write(
        &mut self,
        name: &[u8],
        data: &[u8],
        scratch: &mut [u8],
    ) -> Result<FileCommitReceipt, FileSystemError<F::Error>> {
        if !Self::valid_name(name) {
            return Err(FileSystemError::InvalidName);
        }
        if data.len() > DATA_BYTES {
            return Err(FileSystemError::Full);
        }
        let index = self
            .find(name)
            .or_else(|| self.entries.iter().position(|entry| !entry.used))
            .ok_or(FileSystemError::Full)?;
        let previous = self.entries[index];
        let entry = &mut self.entries[index];
        *entry = FileEntry::EMPTY;
        entry.used = true;
        entry.name_len = name.len();
        entry.data_len = data.len();
        entry.name[..name.len()].copy_from_slice(name);
        entry.data[..data.len()].copy_from_slice(data);
        match self.commit(scratch) {
            Ok(receipt) => Ok(receipt),
            Err(error) => {
                self.entries[index] = previous;
                Err(error)
            }
        }
    }

    pub fn remove(
        &mut self,
        name: &[u8],
        scratch: &mut [u8],
    ) -> Result<FileCommitReceipt, FileSystemError<F::Error>> {
        if !Self::valid_name(name) {
            return Err(FileSystemError::InvalidName);
        }
        let index = self.find(name).ok_or(FileSystemError::NotFound)?;
        let previous = self.entries[index];
        self.entries[index] = FileEntry::EMPTY;
        match self.commit(scratch) {
            Ok(receipt) => Ok(receipt),
            Err(error) => {
                self.entries[index] = previous;
                Err(error)
            }
        }
    }

    pub fn read(
        &self,
        name: &[u8],
        output: &mut [u8],
    ) -> Result<Option<usize>, FileSystemError<F::Error>> {
        if !Self::valid_name(name) {
            return Err(FileSystemError::InvalidName);
        }
        let Some(index) = self.find(name) else {
            return Ok(None);
        };
        let data = self.entries[index].data();
        if output.len() < data.len() {
            return Err(FileSystemError::Full);
        }
        output[..data.len()].copy_from_slice(data);
        Ok(Some(data.len()))
    }

    pub fn metadata(&self, slot: usize) -> Option<FileMetadata<'_>> {
        self.entries.get(slot).and_then(|entry| {
            entry.used.then_some(FileMetadata {
                name: entry.name(),
                len: entry.data_len,
            })
        })
    }

    pub fn file_count(&self) -> usize {
        self.entries.iter().filter(|entry| entry.used).count()
    }

    pub const fn generation(&self) -> u32 {
        self.store.generation()
    }

    pub fn into_flash(self) -> F {
        self.store.into_flash()
    }
}

impl<F: Flash> KvStore<F> {
    fn flash<T>(result: Result<T, F::Error>) -> Result<T, KvError<F::Error>> {
        result.map_err(KvError::Flash)
    }

    pub fn mount(mut flash: F) -> Result<Self, KvError<F::Error>> {
        let p0 = Self::committed_generation(&flash, 0);
        let p1 = Self::committed_generation(&flash, 1);
        let (active, generation) = match (p0, p1) {
            (None, None) => {
                Self::format_page(&mut flash, 0, 1)?;
                (0, 1)
            }
            (Some(a), None) => (0, a),
            (None, Some(b)) => (1, b),
            (Some(a), Some(b)) => {
                if Self::generation_is_newer(b, a) {
                    (1, b)
                } else {
                    (0, a)
                }
            }
        };
        let next_word = Self::find_append(&flash, active);
        Ok(Self {
            flash,
            active,
            next_word,
            generation,
        })
    }

    fn generation_is_newer(candidate: u32, current: u32) -> bool {
        let distance = candidate.wrapping_sub(current);
        distance != 0 && distance < 0x8000_0000
    }

    fn committed_generation(flash: &F, page: usize) -> Option<u32> {
        (flash.read_word(page, 0) == PAGE_MAGIC && flash.read_word(page, 2) == PAGE_COMMITTED)
            .then(|| flash.read_word(page, 1))
    }

    fn format_page(flash: &mut F, page: usize, generation: u32) -> Result<(), KvError<F::Error>> {
        Self::flash(flash.erase(page))?;
        Self::flash(flash.write_word(page, 0, PAGE_MAGIC))?;
        Self::flash(flash.write_word(page, 1, generation))?;
        Self::flash(flash.write_word(page, 2, PAGE_COMMITTED))?;
        Ok(())
    }

    fn record_checksum(tagged_key: u32, value: u32) -> u32 {
        let mut hash = 0x811C_9DC5u32;
        for byte in tagged_key
            .to_le_bytes()
            .into_iter()
            .chain(value.to_le_bytes())
        {
            hash = (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193);
        }
        hash
    }

    fn valid_record(flash: &F, page: usize, word: usize) -> Option<(u16, u32)> {
        if word + 2 >= F::WORDS {
            return None;
        }
        let tagged_key = flash.read_word(page, word);
        let value = flash.read_word(page, word + 1);
        let checksum = flash.read_word(page, word + 2);
        if (tagged_key >> 16) as u16 != REC_TAG
            || checksum == BLANK
            || checksum != Self::record_checksum(tagged_key, value)
        {
            return None;
        }
        Some((tagged_key as u16, value))
    }

    fn find_append(flash: &F, page: usize) -> usize {
        let mut word = HEADER_WORDS;
        while word + 2 < F::WORDS {
            if (0..RECORD_WORDS).all(|offset| flash.read_word(page, word + offset) == BLANK) {
                return word;
            }
            word += RECORD_WORDS;
        }
        F::WORDS
    }

    fn append_to(
        flash: &mut F,
        page: usize,
        word: usize,
        key: u16,
        value: u32,
    ) -> Result<(), KvError<F::Error>> {
        let tagged_key = (u32::from(REC_TAG) << 16) | u32::from(key);
        Self::flash(flash.write_word(page, word, tagged_key))?;
        Self::flash(flash.write_word(page, word + 1, value))?;
        // Commit the record last. Torn key/value writes are ignored on mount/read.
        Self::flash(flash.write_word(page, word + 2, Self::record_checksum(tagged_key, value)))?;
        Ok(())
    }

    pub fn get(&self, key: u16) -> Option<u32> {
        let mut found = None;
        let mut word = HEADER_WORDS;
        while word + 2 < F::WORDS {
            if let Some((record_key, value)) = Self::valid_record(&self.flash, self.active, word) {
                if record_key == key {
                    found = Some(value);
                }
            }
            word += RECORD_WORDS;
        }
        found
    }

    pub fn put(&mut self, key: u16, value: u32) -> Result<(), KvError<F::Error>> {
        if self.next_word + 2 >= F::WORDS {
            self.compact()?;
        }
        if self.next_word + 2 >= F::WORDS {
            return Err(KvError::Full);
        }
        Self::append_to(&mut self.flash, self.active, self.next_word, key, value)?;
        self.next_word += RECORD_WORDS;
        Ok(())
    }

    fn compact(&mut self) -> Result<(), KvError<F::Error>> {
        let old = self.active;
        let new = 1 - old;
        let new_generation = self.generation.wrapping_add(1);
        Self::flash(self.flash.erase(new))?;
        Self::flash(self.flash.write_word(new, 0, PAGE_MAGIC))?;
        Self::flash(self.flash.write_word(new, 1, new_generation))?;

        let mut dst = HEADER_WORDS;
        let mut word = HEADER_WORDS;
        while word + 2 < F::WORDS {
            if let Some((key, value)) = Self::valid_record(&self.flash, old, word) {
                let mut later = word + RECORD_WORDS;
                let mut superseded = false;
                while later + 2 < F::WORDS {
                    if let Some((later_key, _)) = Self::valid_record(&self.flash, old, later) {
                        if later_key == key {
                            superseded = true;
                            break;
                        }
                    }
                    later += RECORD_WORDS;
                }
                if !superseded {
                    if dst + 2 >= F::WORDS {
                        return Err(KvError::Full);
                    }
                    Self::append_to(&mut self.flash, new, dst, key, value)?;
                    dst += RECORD_WORDS;
                }
            }
            word += RECORD_WORDS;
        }

        // Page commit is the atomic selection point. Until this word exists, mount
        // ignores the new page. After it exists, either page contains a full dataset.
        Self::flash(self.flash.write_word(new, 2, PAGE_COMMITTED))?;
        self.active = new;
        self.next_word = dst;
        self.generation = new_generation;
        // Failure here is reported, but the newly committed page remains mountable.
        Self::flash(self.flash.erase(old))?;
        Ok(())
    }

    pub const fn active_page(&self) -> usize {
        self.active
    }

    pub fn into_flash(self) -> F {
        self.flash
    }
}

impl<F: FlashEndurance> KvStore<F> {
    pub fn endurance(&self) -> EnduranceReceipt {
        endurance_receipt(&self.flash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum MockError {
        Injected,
        Programmed,
        BadPage,
    }

    #[derive(Clone)]
    struct MockFlash {
        pages: [[u32; 32]; 2],
        erases: [u32; 2],
        writes_until_failure: Option<usize>,
        bad_erase: [bool; 2],
        bad_program: [bool; 2],
    }

    impl MockFlash {
        fn new() -> Self {
            Self {
                pages: [[BLANK; 32]; 2],
                erases: [0; 2],
                writes_until_failure: None,
                bad_erase: [false; 2],
                bad_program: [false; 2],
            }
        }

        fn maybe_fail(&mut self) -> Result<(), MockError> {
            let Some(remaining) = self.writes_until_failure.as_mut() else {
                return Ok(());
            };
            if *remaining == 0 {
                return Err(MockError::Injected);
            }
            *remaining -= 1;
            Ok(())
        }
    }

    impl Flash for MockFlash {
        type Error = MockError;
        const WORDS: usize = 32;

        fn erase(&mut self, page: usize) -> Result<(), Self::Error> {
            self.maybe_fail()?;
            if self.bad_erase[page] {
                return Err(MockError::BadPage);
            }
            self.pages[page] = [BLANK; 32];
            self.erases[page] += 1;
            Ok(())
        }

        fn write_word(&mut self, page: usize, word: usize, val: u32) -> Result<(), Self::Error> {
            self.maybe_fail()?;
            if self.bad_program[page] {
                return Err(MockError::BadPage);
            }
            if self.pages[page][word] != BLANK {
                return Err(MockError::Programmed);
            }
            self.pages[page][word] = val;
            Ok(())
        }

        fn read_word(&self, page: usize, word: usize) -> u32 {
            self.pages[page][word]
        }
    }

    impl FlashEndurance for MockFlash {
        fn erase_cycles(&self, page: usize) -> u32 {
            self.erases[page]
        }

        fn page_healthy(&self, page: usize) -> bool {
            !self.bad_erase[page] && !self.bad_program[page]
        }

        fn rated_erase_cycles(&self) -> Option<u32> {
            Some(10_000)
        }
    }

    #[test]
    fn put_get_latest_wins_and_survives_remount() {
        let mut kv = KvStore::mount(MockFlash::new()).unwrap();
        kv.put(7, 100).unwrap();
        kv.put(8, 200).unwrap();
        kv.put(7, 111).unwrap();
        assert_eq!(kv.get(7), Some(111));
        let kv = KvStore::mount(kv.into_flash()).unwrap();
        assert_eq!(kv.get(7), Some(111));
        assert_eq!(kv.get(8), Some(200));
    }

    #[test]
    fn compaction_preserves_latest_and_wear_levels() {
        let mut kv = KvStore::mount(MockFlash::new()).unwrap();
        for i in 0..40u32 {
            kv.put((i % 3) as u16, 1000 + i).unwrap();
        }
        assert_eq!(kv.get(0), Some(1039));
        assert_eq!(kv.get(1), Some(1037));
        assert_eq!(kv.get(2), Some(1038));
        let flash = kv.into_flash();
        assert!(flash.erases[0] > 0 && flash.erases[1] > 0);
    }

    #[test]
    fn torn_record_is_ignored() {
        let mut kv = KvStore::mount(MockFlash::new()).unwrap();
        kv.put(1, 10).unwrap();
        let mut flash = kv.into_flash();
        let word = KvStore::<MockFlash>::find_append(&flash, 0);
        flash
            .write_word(0, word, (u32::from(REC_TAG) << 16) | 1)
            .unwrap();
        flash.write_word(0, word + 1, 99).unwrap();
        let kv = KvStore::mount(flash).unwrap();
        assert_eq!(kv.get(1), Some(10));
    }

    #[test]
    fn every_compaction_failure_point_preserves_a_committed_dataset() {
        let mut baseline = KvStore::mount(MockFlash::new()).unwrap();
        for i in 0..9u16 {
            baseline.put(i % 2, u32::from(i)).unwrap();
        }
        let baseline_flash = baseline.into_flash();

        for cutoff in 0..20 {
            let mut flash = baseline_flash.clone();
            flash.writes_until_failure = Some(cutoff);
            let mut kv = KvStore::mount(flash).unwrap();
            let _ = kv.put(9, 99);
            let mut flash = kv.into_flash();
            flash.writes_until_failure = None;
            let remounted = KvStore::mount(flash).unwrap();
            assert_eq!(remounted.get(0), Some(8));
            assert_eq!(remounted.get(1), Some(7));
            assert!(matches!(remounted.get(9), None | Some(99)));
        }
    }

    #[test]
    fn generation_selection_is_wrap_aware() {
        assert!(KvStore::<MockFlash>::generation_is_newer(0, u32::MAX));
        assert!(!KvStore::<MockFlash>::generation_is_newer(u32::MAX, 0));
    }

    #[test]
    fn generation_wrap_commits_and_remounts_newer_kv_page() {
        let mut flash = MockFlash::new();
        KvStore::<MockFlash>::format_page(&mut flash, 0, u32::MAX).unwrap();
        KvStore::<MockFlash>::append_to(&mut flash, 0, HEADER_WORDS, 7, 11).unwrap();
        let mut kv = KvStore::mount(flash).unwrap();
        assert_eq!(kv.generation, u32::MAX);
        kv.compact().unwrap();
        assert_eq!(kv.generation, 0);
        let remounted = KvStore::mount(kv.into_flash()).unwrap();
        assert_eq!(remounted.generation, 0);
        assert_eq!(remounted.get(7), Some(11));
    }

    #[test]
    fn blob_survives_multiple_mount_replace_cycles() {
        let mut store = BlobStore::mount(MockFlash::new());
        for generation in 1..=12u8 {
            let image = [generation; 41];
            store.replace(&image).unwrap();
            store = BlobStore::mount(store.into_flash());
            let mut recovered = [0; 64];
            assert_eq!(store.read(&mut recovered), Ok(Some(image.len())));
            assert_eq!(&recovered[..image.len()], &image);
            assert_eq!(store.generation(), u32::from(generation));
        }
    }

    #[test]
    fn every_blob_failure_point_preserves_old_or_new_complete_image() {
        let old = [0x35; 37];
        let new = [0xA7; 43];
        let mut baseline = BlobStore::mount(MockFlash::new());
        baseline.replace(&old).unwrap();
        let baseline_flash = baseline.into_flash();

        for cutoff in 0..24 {
            let mut flash = baseline_flash.clone();
            flash.writes_until_failure = Some(cutoff);
            let mut store = BlobStore::mount(flash);
            let _ = store.replace(&new);
            let mut flash = store.into_flash();
            flash.writes_until_failure = None;
            let remounted = BlobStore::mount(flash);
            let mut recovered = [0; 64];
            let len = remounted.read(&mut recovered).unwrap().unwrap();
            assert!(
                (len == old.len() && recovered[..len] == old)
                    || (len == new.len() && recovered[..len] == new)
            );
        }
    }

    #[test]
    fn blob_commit_receipt_distinguishes_cleanup_failure_and_bad_target_page() {
        let old = [0x31; 17];
        let new = [0x72; 19];
        let mut store = BlobStore::mount(MockFlash::new());
        store.replace(&old).unwrap();
        let mut flash = store.into_flash();
        flash.bad_erase[0] = true;
        let mut store = BlobStore::mount(flash);
        let receipt = store.replace_with_receipt(&new).unwrap();
        assert!(receipt.cleanup_pending);
        assert_eq!(receipt.generation, 2);
        let mut flash = store.into_flash();
        flash.bad_erase[0] = false;
        let mut store = BlobStore::mount(flash);
        let mut output = [0; 32];
        assert_eq!(store.read(&mut output), Ok(Some(new.len())));
        assert_eq!(&output[..new.len()], &new);
        assert_eq!(store.retry_obsolete_cleanup(), Ok(true));
        assert_eq!(store.retry_obsolete_cleanup(), Ok(false));

        let mut flash = store.into_flash();
        flash.bad_erase[0] = true;
        let mut store = BlobStore::mount(flash);
        assert_eq!(
            store.replace_with_receipt(&[0x99; 5]),
            Err(KvError::Flash(MockError::BadPage))
        );
        let mut flash = store.into_flash();
        flash.bad_erase[0] = false;
        let store = BlobStore::mount(flash);
        let len = store.read(&mut output).unwrap().unwrap();
        assert_eq!(&output[..len], &new);
    }

    #[test]
    fn permanent_program_failure_preserves_the_committed_blob() {
        let old = [0x41; 13];
        let mut store = BlobStore::mount(MockFlash::new());
        store.replace(&old).unwrap();
        let mut flash = store.into_flash();
        flash.bad_program[1] = true;
        let mut store = BlobStore::mount(flash);
        assert_eq!(
            store.replace_with_receipt(&[0x82; 9]),
            Err(KvError::Flash(MockError::BadPage))
        );
        let store = BlobStore::mount(store.into_flash());
        let mut output = [0; 16];
        let len = store.read(&mut output).unwrap().unwrap();
        assert_eq!(&output[..len], &old);
    }

    #[test]
    fn blob_generation_wrap_selects_complete_new_image() {
        let old = [0x11; 4];
        let new = [0x22; 7];
        let mut flash = MockFlash::new();
        flash.erase(0).unwrap();
        flash.write_word(0, 0, BLOB_MAGIC).unwrap();
        flash.write_word(0, 1, u32::MAX).unwrap();
        flash.write_word(0, 2, old.len() as u32).unwrap();
        let checksum =
            BlobStore::<MockFlash>::checksum_bytes(u32::MAX, old.len(), old.iter().copied());
        flash.write_word(0, 3, checksum).unwrap();
        flash
            .write_word(0, BLOB_HEADER_WORDS, u32::from_le_bytes(old))
            .unwrap();
        flash.write_word(0, 4, BLOB_COMMITTED).unwrap();
        let mut store = BlobStore::mount(flash);
        assert_eq!(store.generation(), u32::MAX);
        store.replace(&new).unwrap();
        let store = BlobStore::mount(store.into_flash());
        assert_eq!(store.generation(), 0);
        let mut output = [0; 16];
        let len = store.read(&mut output).unwrap().unwrap();
        assert_eq!(&output[..len], &new);
    }

    type TestFiles = AtomicFileSystem<MockFlash, 2, 8, 16>;

    #[test]
    fn filesystem_write_remove_and_remount_preserve_directory() {
        let mut scratch = [0u8; TestFiles::image_bytes()];
        let mut files = TestFiles::mount(MockFlash::new(), &mut scratch)
            .ok()
            .unwrap();
        assert_eq!(files.file_count(), 0);
        let first = files.write(b"config", b"alpha", &mut scratch).unwrap();
        assert_eq!(first.generation, 1);
        assert!(!first.cleanup_pending);
        files.write(b"log", b"one", &mut scratch).unwrap();
        files.write(b"config", b"beta", &mut scratch).unwrap();
        assert_eq!(files.file_count(), 2);
        assert_eq!(
            files.metadata(0),
            Some(FileMetadata {
                name: b"config",
                len: 4,
            })
        );

        let mut output = [0u8; 16];
        assert_eq!(files.read(b"config", &mut output), Ok(Some(4)));
        assert_eq!(&output[..4], b"beta");
        files.remove(b"log", &mut scratch).unwrap();
        assert_eq!(files.read(b"log", &mut output), Ok(None));

        let files = TestFiles::mount(files.into_flash(), &mut scratch)
            .ok()
            .unwrap();
        assert_eq!(files.file_count(), 1);
        assert_eq!(files.read(b"config", &mut output), Ok(Some(4)));
        assert_eq!(&output[..4], b"beta");
    }

    #[test]
    fn every_filesystem_failure_point_recovers_old_or_new_file() {
        let mut scratch = [0u8; TestFiles::image_bytes()];
        let mut baseline = TestFiles::mount(MockFlash::new(), &mut scratch)
            .ok()
            .unwrap();
        baseline.write(b"config", b"old", &mut scratch).unwrap();
        let baseline_flash = baseline.into_flash();

        for cutoff in 0..36 {
            let mut flash = baseline_flash.clone();
            flash.writes_until_failure = Some(cutoff);
            let mut files = TestFiles::mount(flash, &mut scratch).ok().unwrap();
            let _ = files.write(b"config", b"new-value", &mut scratch);
            let mut flash = files.into_flash();
            flash.writes_until_failure = None;
            let files = TestFiles::mount(flash, &mut scratch).ok().unwrap();
            let mut output = [0u8; 16];
            let len = files.read(b"config", &mut output).unwrap().unwrap();
            assert!(
                (len == 3 && &output[..len] == b"old")
                    || (len == 9 && &output[..len] == b"new-value")
            );
        }
    }

    #[test]
    fn filesystem_alternates_pages_without_wear_hotspot() {
        let mut scratch = [0u8; TestFiles::image_bytes()];
        let mut files = TestFiles::mount(MockFlash::new(), &mut scratch)
            .ok()
            .unwrap();
        for value in 0..12u8 {
            files.write(b"counter", &[value], &mut scratch).unwrap();
        }
        let flash = files.into_flash();
        assert!(flash.erases[0] > 0 && flash.erases[1] > 0);
        assert!(flash.erases[0].abs_diff(flash.erases[1]) <= 1);
    }

    #[test]
    fn long_churn_reports_balanced_rated_endurance_without_raw_measurement_claims() {
        let mut kv = KvStore::mount(MockFlash::new()).unwrap();
        let mut model = [None; 8];
        let mut state = 0xC001_CAFEu32;
        for step in 0..1_000u32 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let key = ((state >> 16) as usize) % model.len();
            let value = state ^ step;
            kv.put(key as u16, value).unwrap();
            model[key] = Some(value);
            if step % 37 == 0 {
                kv = KvStore::mount(kv.into_flash()).unwrap();
                for (key, expected) in model.iter().enumerate() {
                    assert_eq!(kv.get(key as u16), *expected);
                }
            }
        }
        let endurance = kv.endurance();
        assert_eq!(endurance.rated_erase_cycles, Some(10_000));
        assert!(endurance.page_healthy.into_iter().all(|healthy| healthy));
        assert!(endurance.erase_imbalance <= 1);
        assert!(endurance.minimum_remaining_cycles.unwrap() < 10_000);
    }
}
