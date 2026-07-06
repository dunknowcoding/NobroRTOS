//! Wear-leveled key-value flash store (M181).
//!
//! Append-only records over two alternating pages: a `put` appends (no erase), `get`
//! scans for the latest record. When the active page fills, the latest value of each key
//! is compacted into the other page and the old page is erased - erases are spread
//! across both pages (wear leveling). Generic over [`Flash`], so the logic is host-tested
//! against a RAM mock with real flash semantics; the nRF NVMC backs it on hardware.
#![cfg_attr(not(test), no_std)]

/// Minimal flash abstraction: word-addressed pages that erase to 0xFFFF_FFFF and can
/// only be programmed while blank.
pub trait Flash {
    /// Words per page.
    const WORDS: usize;
    fn erase(&mut self, page: usize);
    fn write_word(&mut self, page: usize, word: usize, val: u32);
    fn read_word(&self, page: usize, word: usize) -> u32;
}

const BLANK: u32 = 0xFFFF_FFFF;
const HDR_TAG: u32 = 0x4E4B_5600; // "NKV"; epoch lives in the low byte
const REC_TAG: u16 = 0x4B57; // record marker in the key word's top half

/// KV store over exactly two pages of `F`. Records are 2 words: [tag|key, value].
pub struct KvStore<F: Flash> {
    flash: F,
    active: usize,
    next_word: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KvError {
    /// Both pages full of distinct keys - the store cannot compact further.
    Full,
}

impl<F: Flash> KvStore<F> {
    /// Mount the store: pick the active page by the larger epoch header (formatting
    /// blank flash as epoch 1 on page 0).
    pub fn mount(mut flash: F) -> Self {
        let e0 = Self::epoch(&flash, 0);
        let e1 = Self::epoch(&flash, 1);
        let active = match (e0, e1) {
            (None, None) => {
                flash.erase(0);
                flash.write_word(0, 0, HDR_TAG | 1);
                0
            }
            (Some(_), None) => 0,
            (None, Some(_)) => 1,
            (Some(a), Some(b)) => {
                if a >= b {
                    0
                } else {
                    1
                }
            }
        };
        let next_word = Self::find_append(&flash, active);
        Self {
            flash,
            active,
            next_word,
        }
    }

    fn epoch(flash: &F, page: usize) -> Option<u32> {
        let w = flash.read_word(page, 0);
        if w & 0xFFFF_FF00 == HDR_TAG {
            Some(w & 0xFF)
        } else {
            None
        }
    }

    fn find_append(flash: &F, page: usize) -> usize {
        let mut w = 1;
        while w + 1 < F::WORDS {
            if flash.read_word(page, w) == BLANK {
                return w;
            }
            w += 2;
        }
        F::WORDS
    }

    /// Latest value for `key` on the active page.
    pub fn get(&self, key: u16) -> Option<u32> {
        let mut found = None;
        let mut w = 1;
        while w + 1 < F::WORDS {
            let k = self.flash.read_word(self.active, w);
            if k == BLANK {
                break;
            }
            if (k >> 16) as u16 == REC_TAG && (k & 0xFFFF) as u16 == key {
                found = Some(self.flash.read_word(self.active, w + 1));
            }
            w += 2;
        }
        found
    }

    /// Append `key = val`, compacting into the other page when full.
    pub fn put(&mut self, key: u16, val: u32) -> Result<(), KvError> {
        if self.next_word + 1 >= F::WORDS {
            self.compact()?;
        }
        if self.next_word + 1 >= F::WORDS {
            return Err(KvError::Full);
        }
        self.flash.write_word(
            self.active,
            self.next_word,
            (u32::from(REC_TAG) << 16) | u32::from(key),
        );
        self.flash.write_word(self.active, self.next_word + 1, val);
        self.next_word += 2;
        Ok(())
    }

    /// Copy each distinct key's latest value into the other page, bump the epoch, and
    /// erase the old page (spreading erases across both pages).
    fn compact(&mut self) -> Result<(), KvError> {
        let old = self.active;
        let new = 1 - old;
        let epoch = Self::epoch(&self.flash, old).unwrap_or(0);
        self.flash.erase(new);
        self.flash
            .write_word(new, 0, HDR_TAG | ((epoch + 1) & 0xFF));

        let mut dst = 1;
        let mut w = 1;
        while w + 1 < F::WORDS {
            let k = self.flash.read_word(old, w);
            if k == BLANK {
                break;
            }
            if (k >> 16) as u16 == REC_TAG {
                // keep only the LAST record for this key (skip superseded entries)
                let mut later = w + 2;
                let mut superseded = false;
                while later + 1 < F::WORDS {
                    let k2 = self.flash.read_word(old, later);
                    if k2 == BLANK {
                        break;
                    }
                    if k2 == k {
                        superseded = true;
                        break;
                    }
                    later += 2;
                }
                if !superseded {
                    if dst + 1 >= F::WORDS {
                        return Err(KvError::Full);
                    }
                    self.flash.write_word(new, dst, k);
                    let v = self.flash.read_word(old, w + 1);
                    self.flash.write_word(new, dst + 1, v);
                    dst += 2;
                }
            }
            w += 2;
        }
        self.flash.erase(old);
        self.active = new;
        self.next_word = dst;
        Ok(())
    }

    pub fn active_page(&self) -> usize {
        self.active
    }

    /// Consume the store, returning the backing flash (for remount tests).
    pub fn into_flash(self) -> F {
        self.flash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RAM mock with real flash semantics: erase -> 0xFF, program only while blank.
    struct MockFlash {
        pages: [[u32; 32]; 2],
        erases: [u32; 2],
    }

    impl MockFlash {
        fn new() -> Self {
            Self {
                pages: [[BLANK; 32]; 2],
                erases: [0; 2],
            }
        }
    }

    impl Flash for MockFlash {
        const WORDS: usize = 32;
        fn erase(&mut self, page: usize) {
            self.pages[page] = [BLANK; 32];
            self.erases[page] += 1;
        }
        fn write_word(&mut self, page: usize, word: usize, val: u32) {
            assert_eq!(
                self.pages[page][word], BLANK,
                "program over non-blank flash"
            );
            self.pages[page][word] = val;
        }
        fn read_word(&self, page: usize, word: usize) -> u32 {
            self.pages[page][word]
        }
    }

    #[test]
    fn put_get_latest_wins_and_survives_remount() {
        let mut kv = KvStore::mount(MockFlash::new());
        kv.put(7, 100).unwrap();
        kv.put(8, 200).unwrap();
        kv.put(7, 111).unwrap(); // overwrite: append, latest wins
        assert_eq!(kv.get(7), Some(111));
        assert_eq!(kv.get(8), Some(200));
        assert_eq!(kv.get(9), None);
        // remount on the same flash: state persists
        let kv2 = KvStore::mount(kv.into_flash());
        assert_eq!(kv2.get(7), Some(111));
        assert_eq!(kv2.get(8), Some(200));
    }

    #[test]
    fn compaction_preserves_latest_and_wear_levels() {
        let mut kv = KvStore::mount(MockFlash::new());
        // 32 words = header + 15 records; 40 puts over 3 keys forces compactions
        for i in 0..40u32 {
            kv.put((i % 3) as u16, 1000 + i).unwrap();
        }
        assert_eq!(kv.get(0), Some(1039)); // last write of key 0 was i=39
        assert_eq!(kv.get(1), Some(1037));
        assert_eq!(kv.get(2), Some(1038));
        // wear leveling: compaction erased BOTH pages at least once
        let f = kv.into_flash();
        assert!(
            f.erases[0] >= 1 && f.erases[1] >= 1,
            "erases {:?}",
            f.erases
        );
    }

    #[test]
    fn full_store_of_distinct_keys_reports_full() {
        let mut kv = KvStore::mount(MockFlash::new());
        // 15 record slots per page; 15 distinct keys fill it, the 16th cannot compact away
        for k in 0..15u16 {
            kv.put(k, u32::from(k)).unwrap();
        }
        assert_eq!(kv.put(99, 1), Err(KvError::Full));
    }
}
