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

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum MockError {
        Injected,
        Programmed,
    }

    #[derive(Clone)]
    struct MockFlash {
        pages: [[u32; 32]; 2],
        erases: [u32; 2],
        writes_until_failure: Option<usize>,
    }

    impl MockFlash {
        fn new() -> Self {
            Self {
                pages: [[BLANK; 32]; 2],
                erases: [0; 2],
                writes_until_failure: None,
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
            self.pages[page] = [BLANK; 32];
            self.erases[page] += 1;
            Ok(())
        }

        fn write_word(&mut self, page: usize, word: usize, val: u32) -> Result<(), Self::Error> {
            self.maybe_fail()?;
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
}
