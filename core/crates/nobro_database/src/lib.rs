//! Fixed-capacity structured data store for NobroRTOS (database-style operations,
//! scoped for MCUs).
//!
//! Persistence uses an explicit [`RecordCodec`]. Rust object representations are never
//! copied to or from storage, so padding, compiler layout, endianness, references, and
//! invalid bit patterns cannot leak through the safe API.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

/// Errors a table operation can produce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DbError {
    /// The arena or output image is full.
    Full,
    /// The key already exists (insert) or does not exist (update/delete).
    Key,
    /// A persistence image failed structural, checksum, schema, or record validation.
    BadImage,
}

/// Stable persistence contract for one table record.
///
/// `SCHEMA_ID` changes whenever the encoded field meaning changes. `ENCODED_LEN` is the
/// exact wire size; implementations must fill/read exactly that many bytes and reject
/// values outside the type's validity rules.
pub trait RecordCodec: Copy + Default {
    const SCHEMA_ID: u32;
    const ENCODED_LEN: usize;

    fn encode(&self, out: &mut [u8]) -> bool;
    fn decode(input: &[u8]) -> Option<Self>;
}

macro_rules! integer_codec {
    ($ty:ty, $schema:expr) => {
        impl RecordCodec for $ty {
            const SCHEMA_ID: u32 = $schema;
            const ENCODED_LEN: usize = core::mem::size_of::<Self>();

            fn encode(&self, out: &mut [u8]) -> bool {
                if out.len() != Self::ENCODED_LEN {
                    return false;
                }
                out.copy_from_slice(&self.to_le_bytes());
                true
            }

            fn decode(input: &[u8]) -> Option<Self> {
                Some(Self::from_le_bytes(input.try_into().ok()?))
            }
        }
    };
}

integer_codec!(u16, 0x4E42_5532); // "NBU2"
integer_codec!(u32, 0x4E42_5534); // "NBU4"
integer_codec!(u64, 0x4E42_5538); // "NBU8"
integer_codec!(i16, 0x4E42_4932); // "NBI2"
integer_codec!(i32, 0x4E42_4934); // "NBI4"
integer_codec!(i64, 0x4E42_4938); // "NBI8"

/// A typed table: `N` rows of `(u32 key, V)` where `V` has a stable codec.
pub struct Table<V: RecordCodec, const N: usize> {
    keys: [u32; N],
    vals: [V; N],
    used: [bool; N],
    len: usize,
}

impl<V: RecordCodec, const N: usize> Default for Table<V, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V: RecordCodec, const N: usize> Table<V, N> {
    pub fn new() -> Self {
        Self {
            keys: [0; N],
            vals: [V::default(); N],
            used: [false; N],
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    fn slot_of(&self, key: u32) -> Option<usize> {
        (0..N).find(|&i| self.used[i] && self.keys[i] == key)
    }

    pub fn insert(&mut self, key: u32, val: V) -> Result<(), DbError> {
        if self.slot_of(key).is_some() {
            return Err(DbError::Key);
        }
        let slot = (0..N).find(|&i| !self.used[i]).ok_or(DbError::Full)?;
        self.keys[slot] = key;
        self.vals[slot] = val;
        self.used[slot] = true;
        self.len += 1;
        Ok(())
    }

    pub fn get(&self, key: u32) -> Option<V> {
        self.slot_of(key).map(|i| self.vals[i])
    }

    pub fn update(&mut self, key: u32, val: V) -> Result<(), DbError> {
        let i = self.slot_of(key).ok_or(DbError::Key)?;
        self.vals[i] = val;
        Ok(())
    }

    pub fn upsert(&mut self, key: u32, val: V) -> Result<(), DbError> {
        match self.update(key, val) {
            Err(DbError::Key) => self.insert(key, val),
            result => result,
        }
    }

    pub fn delete(&mut self, key: u32) -> Result<(), DbError> {
        let i = self.slot_of(key).ok_or(DbError::Key)?;
        self.vals[i] = V::default();
        self.keys[i] = 0;
        self.used[i] = false;
        self.len -= 1;
        Ok(())
    }

    pub fn iter(&self) -> impl Iterator<Item = (u32, V)> + '_ {
        (0..N)
            .filter(|&i| self.used[i])
            .map(move |i| (self.keys[i], self.vals[i]))
    }

    pub fn select<'a>(
        &'a self,
        pred: impl Fn(&V) -> bool + 'a,
    ) -> impl Iterator<Item = (u32, V)> + 'a {
        self.iter().filter(move |(_, value)| pred(value))
    }

    pub fn next_key(&self, from: u32) -> Option<u32> {
        self.iter()
            .map(|(key, _)| key)
            .filter(|&key| key >= from)
            .min()
    }

    pub fn count(&self, pred: impl Fn(&V) -> bool) -> usize {
        self.select(pred).count()
    }
}

const IMAGE_MAGIC: u32 = 0x4E42_4442; // "NBDB"
const IMAGE_VERSION: u32 = 2;
const HEADER_LEN: usize = 20;
const CHECKSUM_LEN: usize = 4;

fn fnv1a(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811C_9DC5;
    for &byte in bytes {
        hash = (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193);
    }
    hash
}

fn read_u32(input: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        input.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

impl<V: RecordCodec, const N: usize> Table<V, N> {
    /// Serialize live rows into a stable, checksummed image.
    ///
    /// Layout: magic | image-version | schema-id | record-size | row-count | rows
    /// (`key:u32` + encoded record) | FNV-1a checksum. The checksum detects accidental
    /// corruption; it is not an authentication tag.
    pub fn to_image(&self, out: &mut [u8]) -> Result<usize, DbError> {
        let row_len = 4usize.checked_add(V::ENCODED_LEN).ok_or(DbError::Full)?;
        let rows_len = self.len.checked_mul(row_len).ok_or(DbError::Full)?;
        let body_len = HEADER_LEN.checked_add(rows_len).ok_or(DbError::Full)?;
        let total_len = body_len.checked_add(CHECKSUM_LEN).ok_or(DbError::Full)?;
        if out.len() < total_len
            || V::ENCODED_LEN > u32::MAX as usize
            || self.len > u32::MAX as usize
        {
            return Err(DbError::Full);
        }

        out[..total_len].fill(0);
        out[0..4].copy_from_slice(&IMAGE_MAGIC.to_le_bytes());
        out[4..8].copy_from_slice(&IMAGE_VERSION.to_le_bytes());
        out[8..12].copy_from_slice(&V::SCHEMA_ID.to_le_bytes());
        out[12..16].copy_from_slice(&(V::ENCODED_LEN as u32).to_le_bytes());
        out[16..20].copy_from_slice(&(self.len as u32).to_le_bytes());

        let mut pos = HEADER_LEN;
        for (key, value) in self.iter() {
            out[pos..pos + 4].copy_from_slice(&key.to_le_bytes());
            pos += 4;
            if !value.encode(&mut out[pos..pos + V::ENCODED_LEN]) {
                return Err(DbError::BadImage);
            }
            pos += V::ENCODED_LEN;
        }
        let checksum = fnv1a(&out[..pos]);
        out[pos..pos + CHECKSUM_LEN].copy_from_slice(&checksum.to_le_bytes());
        Ok(total_len)
    }

    /// Rebuild a table from a stable image after validating every structural field,
    /// checksum, key, and record through `V::decode`.
    pub fn from_image(image: &[u8]) -> Result<Self, DbError> {
        if image.len() < HEADER_LEN + CHECKSUM_LEN
            || read_u32(image, 0) != Some(IMAGE_MAGIC)
            || read_u32(image, 4) != Some(IMAGE_VERSION)
            || read_u32(image, 8) != Some(V::SCHEMA_ID)
            || read_u32(image, 12) != u32::try_from(V::ENCODED_LEN).ok()
        {
            return Err(DbError::BadImage);
        }

        let count = read_u32(image, 16).ok_or(DbError::BadImage)? as usize;
        if count > N {
            return Err(DbError::BadImage);
        }
        let row_len = 4usize
            .checked_add(V::ENCODED_LEN)
            .ok_or(DbError::BadImage)?;
        let rows_len = count.checked_mul(row_len).ok_or(DbError::BadImage)?;
        let body_len = HEADER_LEN.checked_add(rows_len).ok_or(DbError::BadImage)?;
        let total_len = body_len
            .checked_add(CHECKSUM_LEN)
            .ok_or(DbError::BadImage)?;
        if image.len() < total_len {
            return Err(DbError::BadImage);
        }
        let stored = read_u32(image, body_len).ok_or(DbError::BadImage)?;
        if fnv1a(&image[..body_len]) != stored {
            return Err(DbError::BadImage);
        }

        let mut table = Self::new();
        let mut pos = HEADER_LEN;
        for _ in 0..count {
            let key = read_u32(image, pos).ok_or(DbError::BadImage)?;
            pos += 4;
            let record = V::decode(
                image
                    .get(pos..pos + V::ENCODED_LEN)
                    .ok_or(DbError::BadImage)?,
            )
            .ok_or(DbError::BadImage)?;
            pos += V::ENCODED_LEN;
            table.insert(key, record).map_err(|_| DbError::BadImage)?;
        }
        Ok(table)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Default, PartialEq, Debug)]
    struct Reading {
        celsius_milli: i32,
        ok: bool,
    }

    impl RecordCodec for Reading {
        const SCHEMA_ID: u32 = 0x5445_4D50; // "TEMP"
        const ENCODED_LEN: usize = 5;

        fn encode(&self, out: &mut [u8]) -> bool {
            if out.len() != Self::ENCODED_LEN {
                return false;
            }
            out[..4].copy_from_slice(&self.celsius_milli.to_le_bytes());
            out[4] = u8::from(self.ok);
            true
        }

        fn decode(input: &[u8]) -> Option<Self> {
            if input.len() != Self::ENCODED_LEN || input[4] > 1 {
                return None;
            }
            Some(Self {
                celsius_milli: i32::from_le_bytes(input[..4].try_into().ok()?),
                ok: input[4] == 1,
            })
        }
    }

    #[test]
    fn crud_and_capacity() {
        let mut table: Table<Reading, 4> = Table::new();
        table
            .insert(
                7,
                Reading {
                    celsius_milli: 21_500,
                    ok: true,
                },
            )
            .unwrap();
        assert_eq!(table.insert(7, Reading::default()), Err(DbError::Key));
        assert_eq!(table.get(7).unwrap().celsius_milli, 21_500);
        table
            .update(
                7,
                Reading {
                    celsius_milli: 22_000,
                    ok: true,
                },
            )
            .unwrap();
        table.upsert(9, Reading::default()).unwrap();
        assert_eq!(table.len(), 2);
        assert_eq!(table.delete(9), Ok(()));
        assert_eq!(table.delete(9), Err(DbError::Key));
        for key in 0..3 {
            table.insert(100 + key, Reading::default()).unwrap();
        }
        assert_eq!(table.insert(999, Reading::default()), Err(DbError::Full));
    }

    #[test]
    fn queries_and_ordered_scan() {
        let mut table: Table<Reading, 8> = Table::new();
        for (key, celsius_milli) in [(3u32, 10_000i32), (1, 30_000), (5, 20_000)] {
            table
                .insert(
                    key,
                    Reading {
                        celsius_milli,
                        ok: celsius_milli < 25_000,
                    },
                )
                .unwrap();
        }
        assert_eq!(table.count(|reading| reading.ok), 2);
        assert_eq!(table.next_key(0), Some(1));
        assert_eq!(table.next_key(2), Some(3));
        assert_eq!(table.next_key(4), Some(5));
        assert_eq!(table.next_key(6), None);
    }

    #[test]
    fn stable_image_roundtrip_and_corruption_detection() {
        let mut table: Table<Reading, 4> = Table::new();
        table
            .insert(
                1,
                Reading {
                    celsius_milli: 1000,
                    ok: true,
                },
            )
            .unwrap();
        table.insert(2, Reading::default()).unwrap();
        let mut image = [0u8; 128];
        let len = table.to_image(&mut image).unwrap();
        let restored = Table::<Reading, 4>::from_image(&image[..len]).unwrap();
        assert_eq!(restored.len(), 2);
        assert_eq!(restored.get(1), table.get(1));

        image[HEADER_LEN + 1] ^= 1;
        assert_eq!(
            Table::<Reading, 4>::from_image(&image[..len]).err(),
            Some(DbError::BadImage)
        );
    }

    #[test]
    fn hostile_but_checksummed_record_is_rejected_by_codec() {
        let mut table: Table<Reading, 1> = Table::new();
        table.insert(1, Reading::default()).unwrap();
        let mut image = [0u8; 64];
        let len = table.to_image(&mut image).unwrap();
        let bool_offset = HEADER_LEN + 4 + 4;
        image[bool_offset] = 2;
        let checksum_offset = len - CHECKSUM_LEN;
        let checksum = fnv1a(&image[..checksum_offset]);
        image[checksum_offset..len].copy_from_slice(&checksum.to_le_bytes());
        assert_eq!(
            Table::<Reading, 1>::from_image(&image[..len]).err(),
            Some(DbError::BadImage)
        );
    }

    #[test]
    fn schema_size_count_and_duplicate_keys_are_rejected() {
        let mut table: Table<Reading, 2> = Table::new();
        table.insert(1, Reading::default()).unwrap();
        table.insert(2, Reading::default()).unwrap();
        let mut image = [0u8; 96];
        let len = table.to_image(&mut image).unwrap();

        let mut wrong_schema = image;
        wrong_schema[8] ^= 1;
        assert!(Table::<Reading, 2>::from_image(&wrong_schema[..len]).is_err());

        let mut duplicate = image;
        let second_key = HEADER_LEN + 4 + Reading::ENCODED_LEN;
        duplicate[second_key..second_key + 4].copy_from_slice(&1u32.to_le_bytes());
        let checksum_offset = len - CHECKSUM_LEN;
        let checksum = fnv1a(&duplicate[..checksum_offset]);
        duplicate[checksum_offset..len].copy_from_slice(&checksum.to_le_bytes());
        assert!(Table::<Reading, 2>::from_image(&duplicate[..len]).is_err());

        let mut too_many = image;
        too_many[16..20].copy_from_slice(&3u32.to_le_bytes());
        assert!(Table::<Reading, 2>::from_image(&too_many[..len]).is_err());
    }
}
