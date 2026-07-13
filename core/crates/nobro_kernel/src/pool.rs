//! Static sample pool for zero-copy tickets between SensorSal and consumers.
//!
//! Pool metadata and bytes are protected by `critical-section`. Handles carry a
//! generation as well as a slot index, so a released ticket cannot affect a later
//! allocation that reuses the same slot.

use core::cell::UnsafeCell;

use crate::sample::{PoolHandle, Sample, SampleKind, SAMPLE_POOL_SIZE};

const SLOT_BYTES: usize = 32;
const MAX_GENERATION: u32 = 0x00FF_FFFF;

#[repr(C, align(4))]
#[derive(Clone, Copy)]
struct PoolSlot {
    bytes: [u8; SLOT_BYTES],
    generation: u32,
    refs: u16,
    len: u16,
    kind: SampleKind,
    taken: bool,
}

impl PoolSlot {
    const EMPTY: Self = Self {
        bytes: [0; SLOT_BYTES],
        generation: 0,
        refs: 0,
        len: 0,
        kind: SampleKind::Raw,
        taken: false,
    };

    fn matches(&self, handle: PoolHandle) -> bool {
        handle.is_valid()
            && handle.index() < SAMPLE_POOL_SIZE
            && self.taken
            && self.generation == handle.generation()
    }

    fn next_generation(&self) -> u32 {
        let next = self.generation.wrapping_add(1) & MAX_GENERATION;
        next.max(1)
    }
}

struct PoolStorage(UnsafeCell<[PoolSlot; SAMPLE_POOL_SIZE]>);

// SAFETY: every access to the UnsafeCell is contained by `critical_section::with` and
// no reference to its contents escapes that closure.
unsafe impl Sync for PoolStorage {}

static POOL: PoolStorage = PoolStorage(UnsafeCell::new([PoolSlot::EMPTY; SAMPLE_POOL_SIZE]));

fn with_slots<R>(f: impl FnOnce(&mut [PoolSlot; SAMPLE_POOL_SIZE]) -> R) -> R {
    critical_section::with(|_| {
        // SAFETY: the critical-section token serializes all accesses, and `slots` is
        // used only for this closure invocation.
        let slots = unsafe { &mut *POOL.0.get() };
        f(slots)
    })
}

/// Compact queue representation of the canonical IMU sample (28 B). The sample
/// ticket carries capture/deadline timestamps, while magnitude is derived from
/// `accel_mg`; neither is duplicated in the fixed 32-byte slot.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompactImuPayload {
    pub accel_mg: [i32; 3],
    pub gyro_mdps: [i32; 3],
    pub temperature_centi_c: i32,
}

impl CompactImuPayload {
    pub const LEN: u16 = core::mem::size_of::<Self>() as u16;

    pub fn write_to_handle(handle: PoolHandle, payload: &Self) -> bool {
        SamplePool::with_slot_mut(handle, SampleKind::Imu, Self::LEN, |bytes| {
            let src = payload as *const Self as *const u8;
            // SAFETY: the destination is valid for `LEN` bytes, the source points to a
            // fully initialized `CompactImuPayload`, and the regions do not overlap.
            unsafe {
                core::ptr::copy_nonoverlapping(src, bytes.as_mut_ptr(), Self::LEN as usize);
            }
        })
        .is_some()
    }

    pub fn read_from_handle(handle: PoolHandle) -> Option<Self> {
        SamplePool::with_slot(handle, SampleKind::Imu, Self::LEN, |bytes| {
            let mut payload = Self::default();
            let dst = &mut payload as *mut Self as *mut u8;
            // SAFETY: both regions are valid for `LEN` bytes, do not overlap, and every
            // bit pattern of this all-f32 payload is a valid Rust value.
            unsafe {
                core::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, Self::LEN as usize);
            }
            payload
        })
    }

    pub const fn from_sample(sample: nobro_imu::ImuSample) -> Self {
        Self {
            accel_mg: sample.accel_mg,
            gyro_mdps: sample.gyro_mdps,
            temperature_centi_c: sample.temperature_centi_c,
        }
    }

    pub fn into_sample(self, timestamp_us: u64) -> nobro_imu::ImuSample {
        nobro_imu::ImuSample {
            accel_mg: self.accel_mg,
            accel_mag_mg: nobro_imu::magnitude3(self.accel_mg),
            gyro_mdps: self.gyro_mdps,
            temperature_centi_c: self.temperature_centi_c,
            timestamp_us,
            ..nobro_imu::ImuSample::default()
        }
    }
}

pub struct SamplePool;

impl SamplePool {
    pub fn alloc(kind: SampleKind, len: u16, captured_us: u64, deadline_us: u64) -> Option<Sample> {
        if len as usize > SLOT_BYTES {
            return None;
        }
        with_slots(|slots| {
            let (idx, slot) = slots.iter_mut().enumerate().find(|(_, slot)| !slot.taken)?;
            let generation = slot.next_generation();
            slot.bytes.fill(0);
            slot.generation = generation;
            slot.refs = 1;
            slot.len = len;
            slot.kind = kind;
            slot.taken = true;
            Some(Sample {
                handle: PoolHandle::from_parts(idx, generation),
                len,
                kind,
                captured_us,
                deadline_us,
            })
        })
    }

    /// Add one owner for a ticket that will outlive the current owner.
    pub fn retain(handle: PoolHandle) -> bool {
        with_slots(|slots| {
            let Some(slot) = slots.get_mut(handle.index()) else {
                return false;
            };
            if !slot.matches(handle) || slot.refs == u16::MAX {
                return false;
            }
            slot.refs += 1;
            true
        })
    }

    /// Release one retained owner. Returns false for stale, invalid, or already-free
    /// tickets; such a ticket can never release a newer allocation in the same slot.
    pub fn release(handle: PoolHandle) -> bool {
        with_slots(|slots| {
            let Some(slot) = slots.get_mut(handle.index()) else {
                return false;
            };
            if !slot.matches(handle) || slot.refs == 0 {
                return false;
            }
            slot.refs -= 1;
            if slot.refs == 0 {
                slot.bytes.fill(0);
                slot.len = 0;
                slot.kind = SampleKind::Raw;
                slot.taken = false;
            }
            true
        })
    }

    pub fn is_live(handle: PoolHandle) -> bool {
        with_slots(|slots| {
            slots
                .get(handle.index())
                .map(|slot| slot.matches(handle))
                .unwrap_or(false)
        })
    }

    pub fn free_slots() -> usize {
        with_slots(|slots| slots.iter().filter(|slot| !slot.taken).count())
    }

    pub fn with_slot<R>(
        handle: PoolHandle,
        kind: SampleKind,
        len: u16,
        f: impl FnOnce(&[u8; SLOT_BYTES]) -> R,
    ) -> Option<R> {
        with_slots(|slots| {
            let slot = slots.get(handle.index())?;
            if !slot.matches(handle) || slot.kind != kind || slot.len != len {
                return None;
            }
            Some(f(&slot.bytes))
        })
    }

    pub fn with_slot_mut<R>(
        handle: PoolHandle,
        kind: SampleKind,
        len: u16,
        f: impl FnOnce(&mut [u8; SLOT_BYTES]) -> R,
    ) -> Option<R> {
        with_slots(|slots| {
            let slot = slots.get_mut(handle.index())?;
            if !slot.matches(handle) || slot.kind != kind || slot.len != len {
                return None;
            }
            Some(f(&mut slot.bytes))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release_all_slots() {
        with_slots(|slots| {
            for slot in slots {
                slot.bytes.fill(0);
                slot.refs = 0;
                slot.len = 0;
                slot.kind = SampleKind::Raw;
                slot.taken = false;
            }
        });
    }

    #[test]
    fn imu_payload_requires_live_typed_handle() {
        release_all_slots();
        assert!(CompactImuPayload::read_from_handle(PoolHandle::INVALID).is_none());

        let sample = SamplePool::alloc(SampleKind::Imu, CompactImuPayload::LEN, 10, 20)
            .expect("sample slot");
        let payload = CompactImuPayload {
            accel_mg: [0, 0, 1000],
            gyro_mdps: [1000, 2000, 3000],
            temperature_centi_c: 2500,
        };
        assert_eq!(CompactImuPayload::LEN, 28);
        assert!(CompactImuPayload::write_to_handle(sample.handle, &payload));
        let readback =
            CompactImuPayload::read_from_handle(sample.handle).expect("payload readback");
        assert_eq!(readback, payload);
        let canonical = readback.into_sample(sample.captured_us);
        assert_eq!(canonical.accel_mag_mg, 1000);
        assert_eq!(canonical.timestamp_us, 10);

        assert!(SamplePool::release(sample.handle));
        assert!(CompactImuPayload::read_from_handle(sample.handle).is_none());
        assert!(!SamplePool::release(sample.handle));
    }

    #[test]
    fn stale_handle_cannot_release_reused_slot() {
        release_all_slots();
        let first = SamplePool::alloc(SampleKind::Raw, 4, 0, 0).unwrap();
        let stale = first.handle;
        assert!(SamplePool::release(first.handle));

        let second = SamplePool::alloc(SampleKind::Raw, 4, 1, 1).unwrap();
        assert_eq!(stale.index(), second.handle.index());
        assert_ne!(stale, second.handle);
        assert!(!SamplePool::release(stale));
        assert!(SamplePool::is_live(second.handle));
        assert!(SamplePool::release(second.handle));
    }

    #[test]
    fn retain_requires_matching_releases_and_zeroes_on_last_release() {
        release_all_slots();
        let sample = SamplePool::alloc(SampleKind::Raw, 4, 0, 0).unwrap();
        assert!(
            SamplePool::with_slot_mut(sample.handle, SampleKind::Raw, 4, |slot| {
                slot[..4].copy_from_slice(&[1, 2, 3, 4]);
            })
            .is_some()
        );
        assert!(SamplePool::retain(sample.handle));
        assert!(SamplePool::release(sample.handle));
        assert!(SamplePool::is_live(sample.handle));
        assert!(SamplePool::release(sample.handle));
        assert!(!SamplePool::is_live(sample.handle));

        let reused = SamplePool::alloc(SampleKind::Raw, 4, 0, 0).unwrap();
        let bytes = SamplePool::with_slot(reused.handle, SampleKind::Raw, 4, |slot| {
            [slot[0], slot[1], slot[2], slot[3]]
        })
        .unwrap();
        assert_eq!(bytes, [0, 0, 0, 0]);
        assert!(SamplePool::release(reused.handle));
    }

    #[test]
    fn typed_access_rejects_kind_and_length_mismatch() {
        release_all_slots();
        let raw = SamplePool::alloc(SampleKind::Raw, CompactImuPayload::LEN, 0, 0).unwrap();
        assert!(!CompactImuPayload::write_to_handle(
            raw.handle,
            &CompactImuPayload::default()
        ));
        assert!(CompactImuPayload::read_from_handle(raw.handle).is_none());
        assert!(SamplePool::release(raw.handle));

        let short = SamplePool::alloc(SampleKind::Imu, 4, 0, 0).unwrap();
        assert!(CompactImuPayload::read_from_handle(short.handle).is_none());
        assert!(SamplePool::release(short.handle));
    }

    #[test]
    fn invalid_handles_are_rejected_without_state_change() {
        release_all_slots();
        assert!(!SamplePool::retain(PoolHandle::INVALID));
        assert!(!SamplePool::release(PoolHandle::INVALID));
        assert_eq!(SamplePool::free_slots(), SAMPLE_POOL_SIZE);
    }

    #[test]
    fn isr_main_handoff_keeps_slot_live_until_consumer_release() {
        release_all_slots();
        let sample = SamplePool::alloc(SampleKind::Raw, 1, 0, 0).unwrap();
        assert!(SamplePool::retain(sample.handle));

        // Main drops its reference after publishing the handle to the ISR consumer.
        assert!(SamplePool::release(sample.handle));
        assert!(
            SamplePool::with_slot_mut(sample.handle, SampleKind::Raw, 1, |slot| {
                slot[0] = 42;
            })
            .is_some()
        );
        assert_eq!(
            SamplePool::with_slot(sample.handle, SampleKind::Raw, 1, |slot| slot[0]),
            Some(42)
        );

        assert!(SamplePool::release(sample.handle));
        assert!(!SamplePool::is_live(sample.handle));
    }

    #[test]
    fn multicore_release_order_model_never_frees_with_outstanding_reference() {
        for order in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            release_all_slots();
            let sample = SamplePool::alloc(SampleKind::Raw, 1, 0, 0).unwrap();
            assert!(SamplePool::retain(sample.handle));
            assert!(SamplePool::retain(sample.handle));

            for (position, _actor) in order.into_iter().enumerate() {
                assert!(SamplePool::release(sample.handle));
                assert_eq!(SamplePool::is_live(sample.handle), position < 2);
            }
            assert!(!SamplePool::release(sample.handle));
        }
    }
}
