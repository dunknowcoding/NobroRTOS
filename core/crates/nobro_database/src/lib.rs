//! Fixed-capacity structured data store for NobroRTOS (database-style operations,
//! scoped for MCUs).
//!
//! A [`Table`] holds `(key, record)` rows in a compile-time-sized arena with CRUD,
//! predicate queries, and ordered scans - no heap, no allocator, O(N) worst case with N
//! known at build time. [`Table::to_image`]/[`Table::from_image`] give a deterministic
//! byte image (with a checksum) so a table can be persisted to flash through any storage
//! backend and recovered after reboot.
#![cfg_attr(not(test), no_std)]

/// Errors a table operation can produce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DbError {
    /// The arena is full.
    Full,
    /// The key already exists (insert) or does not exist (update/delete).
    Key,
    /// A persistence image failed validation.
    BadImage,
}

/// A typed table: `N` rows of `(u32 key, V)` where `V` is plain copyable data.
pub struct Table<V: Copy + Default, const N: usize> {
    keys: [u32; N],
    vals: [V; N],
    used: [bool; N],
    len: usize,
}

impl<V: Copy + Default, const N: usize> Default for Table<V, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V: Copy + Default, const N: usize> Table<V, N> {
    pub fn new() -> Self {
        Table { keys: [0; N], vals: [V::default(); N], used: [false; N], len: 0 }
    }

    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn capacity(&self) -> usize {
        N
    }

    fn slot_of(&self, key: u32) -> Option<usize> {
        (0..N).find(|&i| self.used[i] && self.keys[i] == key)
    }

    /// Insert a new row; rejects duplicate keys.
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

    /// Update an existing row in place.
    pub fn update(&mut self, key: u32, val: V) -> Result<(), DbError> {
        let i = self.slot_of(key).ok_or(DbError::Key)?;
        self.vals[i] = val;
        Ok(())
    }

    /// Insert-or-update.
    pub fn upsert(&mut self, key: u32, val: V) -> Result<(), DbError> {
        match self.update(key, val) {
            Err(DbError::Key) => self.insert(key, val),
            r => r,
        }
    }

    pub fn delete(&mut self, key: u32) -> Result<(), DbError> {
        let i = self.slot_of(key).ok_or(DbError::Key)?;
        self.used[i] = false;
        self.len -= 1;
        Ok(())
    }

    /// Iterate live rows in slot order.
    pub fn iter(&self) -> impl Iterator<Item = (u32, V)> + '_ {
        (0..N).filter(|&i| self.used[i]).map(move |i| (self.keys[i], self.vals[i]))
    }

    /// Query: rows whose record satisfies `pred` (a WHERE clause).
    pub fn select<'a>(
        &'a self,
        pred: impl Fn(&V) -> bool + 'a,
    ) -> impl Iterator<Item = (u32, V)> + 'a {
        self.iter().filter(move |(_, v)| pred(v))
    }

    /// The row with the smallest key >= `from` (ordered scans without a sort).
    pub fn next_key(&self, from: u32) -> Option<u32> {
        self.iter().map(|(k, _)| k).filter(|&k| k >= from).min()
    }

    /// Count rows matching a predicate.
    pub fn count(&self, pred: impl Fn(&V) -> bool) -> usize {
        self.select(pred).count()
    }
}

// -------------------------------------------------------------- persistence image

const IMAGE_MAGIC: u32 = 0x4E42_4442; // "NBDB"

fn fnv1a(bytes: &[u8]) -> u32 {
    let mut h: u32 = 0x811C_9DC5;
    for &b in bytes {
        h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
    }
    h
}

impl<V: Copy + Default, const N: usize> Table<V, N> {
    /// Serialize live rows into `out` as a checksummed image. Returns bytes written.
    /// Layout: magic u32 | row_count u32 | rows (key u32 + raw V) | fnv1a u32.
    ///
    /// `V` must be plain data (no references); the image is only readable by the same
    /// firmware build that wrote it (same `V` layout), which is the intended contract
    /// for on-device persistence.
    pub fn to_image(&self, out: &mut [u8]) -> Result<usize, DbError> {
        let vsize = core::mem::size_of::<V>();
        let need = 8 + self.len * (4 + vsize) + 4;
        if out.len() < need {
            return Err(DbError::Full);
        }
        out[0..4].copy_from_slice(&IMAGE_MAGIC.to_le_bytes());
        out[4..8].copy_from_slice(&(self.len as u32).to_le_bytes());
        let mut pos = 8;
        for (k, v) in self.iter() {
            out[pos..pos + 4].copy_from_slice(&k.to_le_bytes());
            pos += 4;
            let vbytes = unsafe {
                core::slice::from_raw_parts((&v as *const V).cast::<u8>(), vsize)
            };
            out[pos..pos + vsize].copy_from_slice(vbytes);
            pos += vsize;
        }
        let crc = fnv1a(&out[..pos]);
        out[pos..pos + 4].copy_from_slice(&crc.to_le_bytes());
        Ok(pos + 4)
    }

    /// Rebuild a table from an image produced by [`Table::to_image`].
    pub fn from_image(image: &[u8]) -> Result<Self, DbError> {
        let vsize = core::mem::size_of::<V>();
        if image.len() < 12 || image[0..4] != IMAGE_MAGIC.to_le_bytes() {
            return Err(DbError::BadImage);
        }
        let count = u32::from_le_bytes(image[4..8].try_into().unwrap()) as usize;
        let body = 8 + count * (4 + vsize);
        if count > N || image.len() < body + 4 {
            return Err(DbError::BadImage);
        }
        let crc = u32::from_le_bytes(image[body..body + 4].try_into().unwrap());
        if fnv1a(&image[..body]) != crc {
            return Err(DbError::BadImage);
        }
        let mut t = Self::new();
        let mut pos = 8;
        for _ in 0..count {
            let key = u32::from_le_bytes(image[pos..pos + 4].try_into().unwrap());
            pos += 4;
            let mut v = V::default();
            unsafe {
                core::slice::from_raw_parts_mut((&mut v as *mut V).cast::<u8>(), vsize)
                    .copy_from_slice(&image[pos..pos + vsize]);
            }
            pos += vsize;
            t.insert(key, v).map_err(|_| DbError::BadImage)?;
        }
        Ok(t)
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

    #[test]
    fn crud_and_capacity() {
        let mut t: Table<Reading, 4> = Table::new();
        assert!(t.insert(7, Reading { celsius_milli: 21_500, ok: true }).is_ok());
        assert_eq!(t.insert(7, Reading::default()), Err(DbError::Key)); // dup
        assert_eq!(t.get(7).unwrap().celsius_milli, 21_500);
        assert!(t.update(7, Reading { celsius_milli: 22_000, ok: true }).is_ok());
        assert_eq!(t.get(7).unwrap().celsius_milli, 22_000);
        assert!(t.upsert(9, Reading { celsius_milli: 5_000, ok: false }).is_ok());
        assert_eq!(t.len(), 2);
        assert!(t.delete(9).is_ok());
        assert_eq!(t.delete(9), Err(DbError::Key));
        for k in 0..3 {
            t.insert(100 + k, Reading::default()).unwrap();
        }
        assert_eq!(t.insert(999, Reading::default()), Err(DbError::Full));
    }

    #[test]
    fn queries_and_ordered_scan() {
        let mut t: Table<Reading, 8> = Table::new();
        for (k, c) in [(3u32, 10_000i32), (1, 30_000), (5, 20_000)] {
            t.insert(k, Reading { celsius_milli: c, ok: c < 25_000 }).unwrap();
        }
        assert_eq!(t.count(|r| r.ok), 2);
        let hot: Vec<u32> = t.select(|r| r.celsius_milli >= 20_000).map(|(k, _)| k).collect();
        assert_eq!(hot.len(), 2);
        // ordered walk: 1 -> 3 -> 5
        assert_eq!(t.next_key(0), Some(1));
        assert_eq!(t.next_key(2), Some(3));
        assert_eq!(t.next_key(4), Some(5));
        assert_eq!(t.next_key(6), None);
    }

    #[test]
    fn image_roundtrip_and_corruption_detected() {
        let mut t: Table<Reading, 4> = Table::new();
        t.insert(1, Reading { celsius_milli: 1000, ok: true }).unwrap();
        t.insert(2, Reading { celsius_milli: 2000, ok: false }).unwrap();
        let mut buf = [0u8; 128];
        let n = t.to_image(&mut buf).unwrap();
        let back: Table<Reading, 4> = Table::from_image(&buf[..n]).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back.get(2).unwrap().celsius_milli, 2000);
        // flip a payload bit -> checksum must reject
        buf[10] ^= 1;
        assert!(matches!(
            Table::<Reading, 4>::from_image(&buf[..n]),
            Err(DbError::BadImage)
        ));
    }
}
