//! Per-execution-context stack guarding as a kernel service (MEM-01).
//!
//! The earlier `stack_guard_demo` proved the canary experiment for one main
//! stack on hardware; this module turns it into an attributable service: each
//! execution context (module) registers its stack region, the service paints
//! the region with a watermark pattern, and every sweep reports — per module —
//! whether the canary words at the stack's growth limit survived and how deep
//! the high-water mark reached. A broken canary is a [`StackFault`] the caller
//! routes into recovery as `KernelError::StackViolation`; the executor's
//! `enforce_stack_guards` does exactly that.
//!
//! Regions are described by address/length because the service inspects live
//! stacks that Rust references must not alias; all memory access is volatile
//! and confined to `paint`/scan internals. On the host, tests register plain
//! arrays as fake stacks.

use crate::ModuleId;

/// Fill byte for unused stack (distinctive, unlikely in real frames).
pub const WATERMARK_PATTERN: u8 = 0xC5;
/// Default number of canary bytes at the growth-limit end.
pub const DEFAULT_CANARY_BYTES: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StackRegion {
    /// Lowest address of the stack (the growth limit on descending stacks).
    pub base: usize,
    pub len: usize,
    pub canary_bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackGuardError {
    Full,
    Duplicate(ModuleId),
    /// Two logical contexts claimed overlapping physical stack memory.
    AliasedRegion(ModuleId),
    /// Zero-length region or canary not smaller than the region.
    InvalidRegion(ModuleId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StackStatus {
    pub module: ModuleId,
    pub canary_intact: bool,
    /// High-water mark: bytes of the region that have been written since paint.
    pub used_bytes: usize,
    pub len: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StackFault {
    pub module: ModuleId,
    pub used_bytes: usize,
    pub len: usize,
}

#[derive(Clone, Copy)]
struct GuardEntry {
    module: ModuleId,
    region: StackRegion,
}

/// Fixed-capacity registry of guarded stacks, one per execution context.
pub struct StackGuardTable<const N: usize> {
    entries: [Option<GuardEntry>; N],
}

impl<const N: usize> StackGuardTable<N> {
    pub const fn new() -> Self {
        Self { entries: [None; N] }
    }

    /// Register a module's stack region and paint its unused span.
    ///
    /// # Safety
    /// `region` must describe memory that is valid for volatile reads/writes
    /// for the whole lifetime of the table, and the painted span
    /// (`base .. base+len`) must lie strictly BELOW the current stack pointer
    /// of the context it guards (painting live frames corrupts them).
    pub unsafe fn register(
        &mut self,
        module: ModuleId,
        region: StackRegion,
    ) -> Result<(), StackGuardError> {
        if region.len == 0 || region.canary_bytes == 0 || region.canary_bytes >= region.len {
            return Err(StackGuardError::InvalidRegion(module));
        }
        if self
            .entries
            .iter()
            .flatten()
            .any(|entry| entry.module == module)
        {
            return Err(StackGuardError::Duplicate(module));
        }
        let Some(end) = region.base.checked_add(region.len) else {
            return Err(StackGuardError::InvalidRegion(module));
        };
        if self.entries.iter().flatten().any(|entry| {
            let entry_end = entry.region.base.saturating_add(entry.region.len);
            region.base < entry_end && entry.region.base < end
        }) {
            return Err(StackGuardError::AliasedRegion(module));
        }
        let Some(slot) = self.entries.iter_mut().find(|slot| slot.is_none()) else {
            return Err(StackGuardError::Full);
        };
        paint(region);
        *slot = Some(GuardEntry { module, region });
        Ok(())
    }

    /// Register the single MSP stack shared by cooperative tasks. It is
    /// attributed to the kernel execution context; per-task attribution is
    /// available only for P-SLICE tasks that actually own separate PSP stacks.
    ///
    /// # Safety
    /// Same memory-lifetime and below-current-SP contract as [`register`].
    pub unsafe fn register_shared_msp(
        &mut self,
        region: StackRegion,
    ) -> Result<(), StackGuardError> {
        self.register(ModuleId::Kernel, region)
    }

    /// Inspect one module's stack: canary integrity + high-water mark.
    pub fn status(&self, module: ModuleId) -> Option<StackStatus> {
        let entry = self
            .entries
            .iter()
            .flatten()
            .find(|entry| entry.module == module)?;
        Some(inspect(entry.module, entry.region))
    }

    /// Iterate over a point-in-time inspection of every registered stack.
    /// Watermarks are cumulative since registration or the last explicit
    /// repaint, so a campaign cannot accidentally understate an earlier peak.
    pub fn statuses(&self) -> impl Iterator<Item = StackStatus> + '_ {
        self.entries
            .iter()
            .flatten()
            .map(|entry| inspect(entry.module, entry.region))
    }

    pub fn len(&self) -> usize {
        self.entries.iter().flatten().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Sweep every guarded stack; the first broken canary is returned for
    /// recovery attribution (subsequent sweeps surface the rest).
    pub fn sweep(&self) -> Option<StackFault> {
        for entry in self.entries.iter().flatten() {
            let status = inspect(entry.module, entry.region);
            if !status.canary_intact {
                return Some(StackFault {
                    module: status.module,
                    used_bytes: status.used_bytes,
                    len: status.len,
                });
            }
        }
        None
    }

    /// Re-paint a module's region after recovery restarted the context.
    ///
    /// # Safety
    /// Same contract as [`register`](Self::register): the span must be below
    /// the context's live stack pointer.
    pub unsafe fn repaint(&self, module: ModuleId) -> bool {
        let Some(entry) = self
            .entries
            .iter()
            .flatten()
            .find(|entry| entry.module == module)
        else {
            return false;
        };
        paint(entry.region);
        true
    }
}

impl<const N: usize> Default for StackGuardTable<N> {
    fn default() -> Self {
        Self::new()
    }
}

unsafe fn paint(region: StackRegion) {
    let ptr = region.base as *mut u8;
    for offset in 0..region.len {
        ptr.add(offset).write_volatile(WATERMARK_PATTERN);
    }
}

fn inspect(module: ModuleId, region: StackRegion) -> StackStatus {
    let ptr = region.base as *const u8;
    let mut canary_intact = true;
    for offset in 0..region.canary_bytes {
        // SAFETY: the register/repaint contract guarantees the region stays
        // valid for volatile reads for the table's lifetime.
        if unsafe { ptr.add(offset).read_volatile() } != WATERMARK_PATTERN {
            canary_intact = false;
            break;
        }
    }
    // Scan upward for the first non-pattern byte: everything above it has been
    // touched by the descending stack.
    let mut first_touched = region.len;
    for offset in 0..region.len {
        // SAFETY: as above.
        if unsafe { ptr.add(offset).read_volatile() } != WATERMARK_PATTERN {
            first_touched = offset;
            break;
        }
    }
    StackStatus {
        module,
        canary_intact,
        used_bytes: region.len - first_touched,
        len: region.len,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region_of(buf: &mut [u8], canary: usize) -> StackRegion {
        StackRegion {
            base: buf.as_mut_ptr() as usize,
            len: buf.len(),
            canary_bytes: canary,
        }
    }

    #[test]
    fn paint_watermark_and_attribution() {
        let mut sensor_stack = [0u8; 64];
        let mut radio_stack = [0u8; 64];
        let mut table = StackGuardTable::<2>::new();
        unsafe {
            table
                .register(ModuleId::Sensor, region_of(&mut sensor_stack, 8))
                .unwrap();
            table
                .register(ModuleId::Radio, region_of(&mut radio_stack, 8))
                .unwrap();
        }

        // Freshly painted: intact, unused.
        let status = table.status(ModuleId::Sensor).unwrap();
        assert!(status.canary_intact);
        assert_eq!(status.used_bytes, 0);
        assert_eq!(table.sweep(), None);

        // The radio context "uses" its top 16 bytes (descending stack).
        for byte in &mut radio_stack[48..] {
            *byte = 0xAB;
        }
        let status = table.status(ModuleId::Radio).unwrap();
        assert!(status.canary_intact);
        assert_eq!(status.used_bytes, 16);
        assert_eq!(table.sweep(), None);

        // Overflow: the radio stack grows through its canary. The fault is
        // attributed to Radio; Sensor stays clean.
        for byte in radio_stack.iter_mut() {
            *byte = 0xAB;
        }
        assert_eq!(
            table.sweep(),
            Some(StackFault {
                module: ModuleId::Radio,
                used_bytes: 64,
                len: 64,
            })
        );
        assert!(table.status(ModuleId::Sensor).unwrap().canary_intact);

        // Recovery restarts the context and repaints; the guard re-arms.
        assert!(unsafe { table.repaint(ModuleId::Radio) });
        assert_eq!(table.sweep(), None);
    }

    #[test]
    fn invalid_regions_and_duplicates_are_rejected() {
        let mut buf = [0u8; 32];
        let mut table = StackGuardTable::<1>::new();
        unsafe {
            assert_eq!(
                table.register(ModuleId::Sensor, region_of(&mut buf, 32)),
                Err(StackGuardError::InvalidRegion(ModuleId::Sensor))
            );
            assert_eq!(
                table.register(
                    ModuleId::Sensor,
                    StackRegion {
                        base: usize::MAX - 3,
                        len: 8,
                        canary_bytes: 1,
                    },
                ),
                Err(StackGuardError::InvalidRegion(ModuleId::Sensor))
            );
            table
                .register(ModuleId::Sensor, region_of(&mut buf, 4))
                .unwrap();
            let mut other = [0u8; 32];
            assert_eq!(
                table.register(ModuleId::Sensor, region_of(&mut other, 4)),
                Err(StackGuardError::Duplicate(ModuleId::Sensor))
            );
        }
    }

    #[test]
    fn cooperative_stack_is_one_kernel_owned_region_and_overlap_is_rejected() {
        let mut stack = [0u8; 96];
        let mut table = StackGuardTable::<2>::new();
        unsafe {
            table.register_shared_msp(region_of(&mut stack, 8)).unwrap();
            assert_eq!(
                table.register(
                    ModuleId::Sensor,
                    StackRegion {
                        base: stack.as_mut_ptr().add(32) as usize,
                        len: 32,
                        canary_bytes: 8,
                    },
                ),
                Err(StackGuardError::AliasedRegion(ModuleId::Sensor))
            );
        }
        assert_eq!(table.status(ModuleId::Kernel).unwrap().len, 96);
    }
}
