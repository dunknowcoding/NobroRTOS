//! Fixed-capacity kernel key-value store contract.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KvKey(pub u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KvValue {
    Bool(bool),
    U32(u32),
    I32(i32),
    Bytes { len: u8, data: [u8; 8] },
}

impl KvValue {
    pub const fn bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() > 8 {
            return None;
        }

        let mut data = [0u8; 8];
        let mut idx = 0;
        while idx < bytes.len() {
            data[idx] = bytes[idx];
            idx += 1;
        }
        Some(Self::Bytes {
            len: bytes.len() as u8,
            data,
        })
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes { len, data } => Some(&data[..usize::from(*len)]),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KvEntry {
    pub key: KvKey,
    pub value: KvValue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KvError {
    Full,
    Missing(KvKey),
}

pub struct KvStore<const N: usize> {
    entries: [Option<KvEntry>; N],
    writes: u32,
    deletes: u32,
}

impl<const N: usize> KvStore<N> {
    pub const fn new() -> Self {
        Self {
            entries: [None; N],
            writes: 0,
            deletes: 0,
        }
    }

    pub fn set(&mut self, key: KvKey, value: KvValue) -> Result<(), KvError> {
        if let Some(entry) = self.find_mut(key) {
            entry.value = value;
            self.writes = self.writes.saturating_add(1);
            return Ok(());
        }

        let Some(slot) = self.entries.iter_mut().find(|slot| slot.is_none()) else {
            return Err(KvError::Full);
        };
        *slot = Some(KvEntry { key, value });
        self.writes = self.writes.saturating_add(1);
        Ok(())
    }

    pub fn get(&self, key: KvKey) -> Option<KvValue> {
        self.entries
            .iter()
            .flatten()
            .find(|entry| entry.key == key)
            .map(|entry| entry.value)
    }

    pub fn delete(&mut self, key: KvKey) -> Result<KvValue, KvError> {
        let Some(idx) = self.index_of(key) else {
            return Err(KvError::Missing(key));
        };
        let value = self.entries[idx]
            .take()
            .map(|entry| entry.value)
            .ok_or(KvError::Missing(key))?;
        self.deletes = self.deletes.saturating_add(1);
        Ok(value)
    }

    pub fn contains(&self, key: KvKey) -> bool {
        self.get(key).is_some()
    }

    pub fn copy_entries(&self, out: &mut [KvEntry]) -> usize {
        let mut copied = 0;
        for entry in self.entries.iter().flatten() {
            if copied >= out.len() {
                break;
            }
            out[copied] = *entry;
            copied += 1;
        }
        copied
    }

    pub fn len(&self) -> usize {
        self.entries.iter().flatten().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub const fn writes(&self) -> u32 {
        self.writes
    }

    pub const fn deletes(&self) -> u32 {
        self.deletes
    }

    fn find_mut(&mut self, key: KvKey) -> Option<&mut KvEntry> {
        self.entries
            .iter_mut()
            .flatten()
            .find(|entry| entry.key == key)
    }

    fn index_of(&self, key: KvKey) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.map(|entry| entry.key == key).unwrap_or(false))
    }
}

impl<const N: usize> Default for KvStore<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kv_store_sets_gets_and_overwrites_values() {
        let mut kv = KvStore::<2>::new();

        kv.set(KvKey(1), KvValue::U32(42)).unwrap();
        kv.set(KvKey(1), KvValue::U32(84)).unwrap();

        assert_eq!(kv.get(KvKey(1)), Some(KvValue::U32(84)));
        assert_eq!(kv.len(), 1);
        assert_eq!(kv.writes(), 2);
    }

    #[test]
    fn kv_store_rejects_full_table_without_overwrite() {
        let mut kv = KvStore::<1>::new();
        kv.set(KvKey(1), KvValue::Bool(true)).unwrap();

        assert_eq!(kv.set(KvKey(2), KvValue::Bool(false)), Err(KvError::Full));
        assert_eq!(kv.get(KvKey(1)), Some(KvValue::Bool(true)));
        assert_eq!(kv.get(KvKey(2)), None);
    }

    #[test]
    fn kv_store_deletes_existing_value() {
        let mut kv = KvStore::<2>::new();
        kv.set(KvKey(7), KvValue::I32(-4)).unwrap();

        assert_eq!(kv.delete(KvKey(7)), Ok(KvValue::I32(-4)));
        assert_eq!(kv.delete(KvKey(7)), Err(KvError::Missing(KvKey(7))));
        assert!(kv.is_empty());
        assert_eq!(kv.deletes(), 1);
    }

    #[test]
    fn kv_value_bytes_are_bounded() {
        let value = KvValue::bytes(b"AIRON").unwrap();

        assert_eq!(value.as_bytes(), Some(&b"AIRON"[..]));
        assert_eq!(KvValue::bytes(b"too-long!"), None);
    }

    #[test]
    fn kv_store_copies_entries_to_host_buffer() {
        let mut kv = KvStore::<3>::new();
        kv.set(KvKey(1), KvValue::Bool(true)).unwrap();
        kv.set(KvKey(2), KvValue::U32(9)).unwrap();
        let mut out = [KvEntry {
            key: KvKey(0),
            value: KvValue::Bool(false),
        }; 1];

        assert_eq!(kv.copy_entries(&mut out), 1);
        assert_eq!(out[0].key, KvKey(1));
    }
}
