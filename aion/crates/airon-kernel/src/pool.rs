//! Static sample pool for zero-copy tickets between SensorSal and consumers.

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use crate::sample::{PoolHandle, Sample, SampleKind, SAMPLE_POOL_SIZE};

const SLOT_BYTES: usize = 32;

#[repr(C, align(4))]
#[derive(Clone, Copy)]
struct PoolSlot {
    bytes: [u8; SLOT_BYTES],
}

static mut POOL: [PoolSlot; SAMPLE_POOL_SIZE] = [PoolSlot {
    bytes: [0; SLOT_BYTES],
}; SAMPLE_POOL_SIZE];
static TAKEN: [AtomicBool; SAMPLE_POOL_SIZE] = [
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
];
static REFS: [AtomicU8; SAMPLE_POOL_SIZE] = [
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
];

/// IMU payload layout (matches NiusIMU engineering units, 24 B).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ImuPayload {
    pub accel_g: [f32; 3],
    pub gyro_dps: [f32; 3],
}

impl ImuPayload {
    pub const LEN: u16 = core::mem::size_of::<Self>() as u16;

    pub fn write_to_slot(slot: usize, payload: &Self) -> bool {
        if slot >= SAMPLE_POOL_SIZE || !TAKEN[slot].load(Ordering::Acquire) {
            return false;
        }
        unsafe {
            let dst = POOL[slot].bytes.as_mut_ptr() as *mut ImuPayload;
            core::ptr::write(dst, *payload);
        }
        true
    }

    pub fn write_to_handle(handle: PoolHandle, payload: &Self) -> bool {
        Self::write_to_slot(handle.0 as usize, payload)
    }

    pub fn read_from_handle(handle: PoolHandle) -> Option<Self> {
        let idx = handle.0 as usize;
        if idx >= SAMPLE_POOL_SIZE || !TAKEN[idx].load(Ordering::Acquire) {
            return None;
        }
        unsafe {
            let src = POOL[idx].bytes.as_ptr() as *const ImuPayload;
            Some(core::ptr::read(src))
        }
    }
}

pub struct SamplePool;

impl SamplePool {
    pub fn alloc(kind: SampleKind, len: u16, captured_us: u64, deadline_us: u64) -> Option<Sample> {
        if len as usize > SLOT_BYTES {
            return None;
        }
        for (idx, taken) in TAKEN.iter().enumerate() {
            if taken
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                REFS[idx].store(1, Ordering::Release);
                return Some(Sample {
                    handle: PoolHandle(idx as u16),
                    len,
                    kind,
                    captured_us,
                    deadline_us,
                });
            }
        }
        None
    }

    pub fn release(handle: PoolHandle) {
        let idx = handle.0 as usize;
        if idx >= SAMPLE_POOL_SIZE {
            return;
        }
        loop {
            let prev = REFS[idx].load(Ordering::Acquire);
            if prev == 0 {
                return;
            }
            let next = prev - 1;
            if REFS[idx]
                .compare_exchange(prev, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                if next == 0 {
                    TAKEN[idx].store(false, Ordering::Release);
                }
                return;
            }
        }
    }

    pub fn slot_mut(idx: usize) -> Option<&'static mut [u8; SLOT_BYTES]> {
        if idx >= SAMPLE_POOL_SIZE || !TAKEN[idx].load(Ordering::Acquire) {
            return None;
        }
        unsafe { Some(&mut POOL[idx].bytes) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imu_payload_requires_allocated_handle() {
        assert!(ImuPayload::read_from_handle(PoolHandle(0)).is_none());

        let sample =
            SamplePool::alloc(SampleKind::Imu, ImuPayload::LEN, 10, 20).expect("sample slot");
        let payload = ImuPayload {
            accel_g: [0.0, 0.0, 1.0],
            gyro_dps: [1.0, 2.0, 3.0],
        };
        assert!(ImuPayload::write_to_handle(sample.handle, &payload));
        let readback = ImuPayload::read_from_handle(sample.handle).expect("payload readback");
        assert_eq!(readback.accel_g, payload.accel_g);
        assert_eq!(readback.gyro_dps, payload.gyro_dps);

        SamplePool::release(sample.handle);
        assert!(ImuPayload::read_from_handle(sample.handle).is_none());
    }

    #[test]
    fn release_invalid_or_free_handle_is_noop() {
        SamplePool::release(PoolHandle::INVALID);
        SamplePool::release(PoolHandle(0));
        assert!(SamplePool::slot_mut(0).is_none());
    }
}
