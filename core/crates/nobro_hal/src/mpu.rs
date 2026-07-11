//! ARMv7-M MPU kernel profile (MEM-02): deny-by-default region plans with
//! host-testable encoding and attributable fault-frame capture.
//!
//! The earlier `mpu_guard_demo` proved one read-only region and a recovering
//! MemManage handler on hardware. This module generalizes it into a service:
//!
//! - [`MpuRegionSpec`] validates and encodes PMSAv7 regions (power-of-two size,
//!   base alignment, access/attribute bits) as pure functions — the region math
//!   is unit-tested on the host, so a bad plan fails in CI, not in a fault loop.
//! - [`KernelMpuPlan`] composes regions into a profile. `deny_by_default`
//!   plans run with `PRIVDEFENA = 0` (anything uncovered faults), and
//!   validation refuses a plan that would brick the core (no executable code
//!   region or no writable RAM region).
//! - [`MpuFaultRecord::decode_mem_manage`] turns CFSR/MMFAR plus the stacked
//!   PC and the executor's [`ExecutionSentinel`] module code into an
//!   attributable record the MemManage handler stores for the host — closing
//!   the "fault, but whose?" gap.
//!
//! `install`/`disable` touch the real MPU only on ARM targets; every other
//! function is portable. The no-MPU profile is simply not installing a plan —
//! nothing else in the kernel changes shape.

/// PMSAv7 register addresses (armv7-m System Control Space); only touched by
/// the ARM-gated install/disable paths.
#[cfg(target_arch = "arm")]
const MPU_TYPE: u32 = 0xE000_ED90;
#[cfg(target_arch = "arm")]
const MPU_CTRL: u32 = 0xE000_ED94;
#[cfg(target_arch = "arm")]
const MPU_RNR: u32 = 0xE000_ED98;
#[cfg(target_arch = "arm")]
const MPU_RBAR: u32 = 0xE000_ED9C;
#[cfg(target_arch = "arm")]
const MPU_RASR: u32 = 0xE000_EDA0;
#[cfg(target_arch = "arm")]
const SHCSR: u32 = 0xE000_ED24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MpuAccess {
    /// No access for any privilege level — guard bands, deny regions.
    NoAccess,
    /// Read-only for all privilege levels.
    ReadOnly,
    /// Full read/write access.
    ReadWrite,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MpuRegionSpec {
    /// Region base; must be aligned to `size_bytes`.
    pub base: u32,
    /// Power of two, 32 bytes .. 4 GiB.
    pub size_bytes: u64,
    pub access: MpuAccess,
    /// Instruction fetch allowed (code regions only).
    pub executable: bool,
    /// Device (strongly-ordered-ish) instead of normal memory attributes.
    pub device: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MpuPlanError {
    /// Size not a power of two or outside 32 B .. 4 GiB.
    InvalidSize {
        index: usize,
    },
    /// Base not aligned to the region size.
    Misaligned {
        index: usize,
    },
    TooManyRegions {
        regions: usize,
        supported: usize,
    },
    /// A deny-by-default plan without executable code coverage would fault on
    /// the next instruction fetch.
    NoExecutableRegion,
    /// A deny-by-default plan without writable RAM would fault on the next
    /// stack push.
    NoWritableRegion,
}

impl MpuRegionSpec {
    pub const fn code(base: u32, size_bytes: u64) -> Self {
        Self {
            base,
            size_bytes,
            access: MpuAccess::ReadOnly,
            executable: true,
            device: false,
        }
    }

    pub const fn ram(base: u32, size_bytes: u64) -> Self {
        Self {
            base,
            size_bytes,
            access: MpuAccess::ReadWrite,
            executable: false,
            device: false,
        }
    }

    pub const fn peripherals(base: u32, size_bytes: u64) -> Self {
        Self {
            base,
            size_bytes,
            access: MpuAccess::ReadWrite,
            executable: false,
            device: true,
        }
    }

    /// A no-access guard band (e.g. the bottom of a module stack): any touch
    /// raises MemManage before the overflow corrupts a neighbor.
    pub const fn guard(base: u32, size_bytes: u64) -> Self {
        Self {
            base,
            size_bytes,
            access: MpuAccess::NoAccess,
            executable: false,
            device: false,
        }
    }

    fn validate(&self, index: usize) -> Result<(), MpuPlanError> {
        if !self.size_bytes.is_power_of_two() || self.size_bytes < 32 || self.size_bytes > 1 << 32 {
            return Err(MpuPlanError::InvalidSize { index });
        }
        let mask = (self.size_bytes - 1) as u32;
        if self.base & mask != 0 {
            return Err(MpuPlanError::Misaligned { index });
        }
        Ok(())
    }

    /// Encode as (RBAR, RASR) for region `index`. Pure — host-tested.
    pub fn encode(&self, index: usize) -> Result<(u32, u32), MpuPlanError> {
        self.validate(index)?;
        let rbar = self.base | (1 << 4) | (index as u32 & 0xF);
        let ap = match self.access {
            MpuAccess::NoAccess => 0b000,
            MpuAccess::ReadOnly => 0b110,
            MpuAccess::ReadWrite => 0b011,
        };
        let xn = u32::from(!self.executable);
        // Normal memory: S,C,B; device memory: S,B (shareable device).
        let attrs = if self.device { 0b101 } else { 0b111 };
        // SIZE field: region bytes = 2^(SIZE+1).
        let size_field = (63 - self.size_bytes.leading_zeros()) - 1;
        let rasr = (xn << 28) | (ap << 24) | (attrs << 16) | (size_field << 1) | 1;
        Ok((rbar, rasr))
    }
}

/// A composed MPU profile. `N` is the plan capacity, not the hardware's —
/// hardware region count is checked against `MPU_TYPE` at install.
#[derive(Clone, Copy, Debug)]
pub struct KernelMpuPlan<const N: usize> {
    regions: [Option<MpuRegionSpec>; N],
    len: usize,
    /// `true`: PRIVDEFENA off — everything not covered by a region faults.
    pub deny_by_default: bool,
}

impl<const N: usize> KernelMpuPlan<N> {
    pub const fn new(deny_by_default: bool) -> Self {
        Self {
            regions: [None; N],
            len: 0,
            deny_by_default,
        }
    }

    pub fn add(&mut self, region: MpuRegionSpec) -> Result<usize, MpuPlanError> {
        if self.len == N {
            return Err(MpuPlanError::TooManyRegions {
                regions: self.len + 1,
                supported: N,
            });
        }
        region.validate(self.len)?;
        let index = self.len;
        self.regions[index] = Some(region);
        self.len += 1;
        Ok(index)
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Full-plan validation, host-runnable: every region encodes, and a
    /// deny-by-default plan covers code execution and writable RAM.
    pub fn validate(&self) -> Result<(), MpuPlanError> {
        let mut executable = false;
        let mut writable = false;
        for (index, region) in self.regions.iter().flatten().enumerate() {
            region.encode(index)?;
            executable |= region.executable && region.access != MpuAccess::NoAccess;
            writable |= region.access == MpuAccess::ReadWrite && !region.device;
        }
        if self.deny_by_default {
            if !executable {
                return Err(MpuPlanError::NoExecutableRegion);
            }
            if !writable {
                return Err(MpuPlanError::NoWritableRegion);
            }
        }
        Ok(())
    }

    /// Program and enable the MPU with this plan (ARM only). Unused hardware
    /// regions are disabled so stale entries cannot linger.
    ///
    /// # Safety
    /// The plan must keep the currently executing code, the active stack, and
    /// the vector table accessible, or the core faults immediately. Call with
    /// interrupts quiesced.
    #[cfg(target_arch = "arm")]
    pub unsafe fn install(&self) -> Result<(), MpuPlanError> {
        self.validate()?;
        let supported = ((reg_read(MPU_TYPE) >> 8) & 0xFF) as usize;
        if self.len > supported {
            return Err(MpuPlanError::TooManyRegions {
                regions: self.len,
                supported,
            });
        }
        reg_write(MPU_CTRL, 0);
        barrier();
        for index in 0..supported {
            reg_write(MPU_RNR, index as u32);
            match self.regions.get(index).copied().flatten() {
                Some(region) => {
                    let (rbar, rasr) = region.encode(index)?;
                    reg_write(MPU_RBAR, rbar);
                    reg_write(MPU_RASR, rasr);
                }
                None => {
                    reg_write(MPU_RASR, 0);
                    reg_write(MPU_RBAR, (1 << 4) | (index as u32 & 0xF));
                }
            }
        }
        // Enable the MemManage fault so violations are attributable instead of
        // escalating straight to HardFault.
        reg_write(SHCSR, reg_read(SHCSR) | (1 << 16));
        let privdefena = if self.deny_by_default { 0 } else { 1 << 2 };
        reg_write(MPU_CTRL, 1 | privdefena);
        barrier();
        Ok(())
    }

    /// Turn the MPU off (recovery escape hatch — the no-MPU profile).
    ///
    /// # Safety
    /// Removes all protection; only call from a fault handler or controlled
    /// maintenance path.
    #[cfg(target_arch = "arm")]
    pub unsafe fn disable() {
        reg_write(MPU_CTRL, 0);
        barrier();
    }
}

#[cfg(target_arch = "arm")]
unsafe fn reg_read(addr: u32) -> u32 {
    core::ptr::read_volatile(addr as *const u32)
}

#[cfg(target_arch = "arm")]
unsafe fn reg_write(addr: u32, value: u32) {
    core::ptr::write_volatile(addr as *mut u32, value);
}

#[cfg(target_arch = "arm")]
fn barrier() {
    unsafe {
        core::arch::asm!("dsb", "isb", options(nostack, preserves_flags));
    }
}

/// Attributable capture of one MemManage fault: what faulted, where, and which
/// module was in flight (from the executor's `ExecutionSentinel`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MpuFaultRecord {
    pub cfsr: u32,
    pub fault_address: u32,
    pub fault_address_valid: bool,
    pub stacked_pc: u32,
    /// `module_code` of the in-flight poll (0 = kernel/idle context).
    pub module_code: u32,
    pub data_access: bool,
    pub instruction_access: bool,
    pub stacking_error: bool,
}

impl MpuFaultRecord {
    /// Decode the MemManage half of CFSR. Pure — host-tested; the ARM handler
    /// just feeds registers in and stores the record.
    pub const fn decode_mem_manage(
        cfsr: u32,
        mmfar: u32,
        stacked_pc: u32,
        module_code: u32,
    ) -> Self {
        let mmfsr = cfsr & 0xFF;
        Self {
            cfsr,
            fault_address: mmfar,
            fault_address_valid: mmfsr & (1 << 7) != 0,
            stacked_pc,
            module_code,
            data_access: mmfsr & (1 << 1) != 0,
            instruction_access: mmfsr & 1 != 0,
            stacking_error: mmfsr & ((1 << 3) | (1 << 4)) != 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_encoding_matches_the_hardware_proven_demo() {
        // mpu_guard_demo's proven region: 256 B read-only SRAM, XN, at `base`.
        let region = MpuRegionSpec {
            base: 0x2000_0100,
            size_bytes: 256,
            access: MpuAccess::ReadOnly,
            executable: false,
            device: false,
        };
        let (rbar, rasr) = region.encode(0).unwrap();
        assert_eq!(rbar, 0x2000_0100 | (1 << 4));
        assert_eq!(
            rasr,
            (1 << 28) | (0b110 << 24) | (0b111 << 16) | (7 << 1) | 1
        );
    }

    #[test]
    fn invalid_regions_are_rejected_on_the_host() {
        assert_eq!(
            MpuRegionSpec::ram(0x2000_0000, 48).encode(0),
            Err(MpuPlanError::InvalidSize { index: 0 })
        );
        assert_eq!(
            MpuRegionSpec::ram(0x2000_0010, 256).encode(1),
            Err(MpuPlanError::Misaligned { index: 1 })
        );
        assert_eq!(
            MpuRegionSpec::ram(0x2000_0000, 16).encode(0),
            Err(MpuPlanError::InvalidSize { index: 0 })
        );
    }

    #[test]
    fn deny_by_default_plans_must_not_brick_the_core() {
        let mut plan = KernelMpuPlan::<4>::new(true);
        plan.add(MpuRegionSpec::ram(0x2000_0000, 64 * 1024))
            .unwrap();
        // RAM only: instruction fetch would fault everywhere.
        assert_eq!(plan.validate(), Err(MpuPlanError::NoExecutableRegion));
        plan.add(MpuRegionSpec::code(0x0000_0000, 1024 * 1024))
            .unwrap();
        plan.add(MpuRegionSpec::peripherals(0x4000_0000, 0x2000_0000))
            .unwrap();
        plan.add(MpuRegionSpec::guard(0x2000_4000, 256)).unwrap();
        assert_eq!(plan.validate(), Ok(()));
        assert_eq!(plan.len(), 4);
        // A permissive (PRIVDEFENA) plan may be sparse.
        let sparse = KernelMpuPlan::<1>::new(false);
        assert_eq!(sparse.validate(), Ok(()));
    }

    #[test]
    fn fault_record_decodes_and_attributes() {
        // DACCVIOL | MMARVALID with a live module code from the sentinel.
        let record =
            MpuFaultRecord::decode_mem_manage((1 << 7) | (1 << 1), 0x2000_4010, 0x0002_6ABC, 5);
        assert!(record.data_access);
        assert!(record.fault_address_valid);
        assert_eq!(record.fault_address, 0x2000_4010);
        assert_eq!(record.module_code, 5);
        assert!(!record.instruction_access);
        assert!(!record.stacking_error);

        // Stacking error without a valid MMFAR.
        let record = MpuFaultRecord::decode_mem_manage(1 << 4, 0, 0, 0);
        assert!(record.stacking_error);
        assert!(!record.fault_address_valid);
    }
}
