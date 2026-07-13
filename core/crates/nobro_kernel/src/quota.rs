//! Fixed-capacity resource quota ledger.

use crate::{ModuleId, SystemBudget, SystemManifest};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuotaEntry {
    pub module: ModuleId,
    pub limit: SystemBudget,
    pub used: SystemBudget,
}

impl QuotaEntry {
    pub const fn new(module: ModuleId, limit: SystemBudget) -> Self {
        Self {
            module,
            limit,
            used: SystemBudget::ZERO,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuotaError {
    Full,
    DuplicateModule(ModuleId),
    MissingModule(ModuleId),
    Overflow(ModuleId),
    Exceeded {
        module: ModuleId,
        used: SystemBudget,
        limit: SystemBudget,
    },
    Underflow {
        module: ModuleId,
        used: SystemBudget,
        release: SystemBudget,
    },
}

#[derive(Debug)]
pub struct QuotaLedger<const N: usize> {
    entries: [Option<QuotaEntry>; N],
}

impl<const N: usize> QuotaLedger<N> {
    pub const fn new() -> Self {
        Self { entries: [None; N] }
    }

    pub fn register(&mut self, module: ModuleId, limit: SystemBudget) -> Result<(), QuotaError> {
        if self.find(module).is_some() {
            return Err(QuotaError::DuplicateModule(module));
        }

        let Some(slot) = self.entries.iter_mut().find(|slot| slot.is_none()) else {
            return Err(QuotaError::Full);
        };
        *slot = Some(QuotaEntry::new(module, limit));
        Ok(())
    }

    pub fn register_manifest<const M: usize>(
        &mut self,
        manifest: &SystemManifest<M>,
    ) -> Result<(), QuotaError> {
        for spec in manifest.iter() {
            self.register(spec.id, SystemBudget::from_memory(spec.memory))?;
        }
        Ok(())
    }

    pub fn reserve(&mut self, module: ModuleId, amount: SystemBudget) -> Result<(), QuotaError> {
        let Some(entry) = self.find_mut(module) else {
            return Err(QuotaError::MissingModule(module));
        };
        let Some(next) = entry.used.checked_add(amount) else {
            return Err(QuotaError::Overflow(module));
        };
        if !next.fits_within(entry.limit) {
            return Err(QuotaError::Exceeded {
                module,
                used: next,
                limit: entry.limit,
            });
        }

        entry.used = next;
        Ok(())
    }

    pub fn release(&mut self, module: ModuleId, amount: SystemBudget) -> Result<(), QuotaError> {
        let Some(entry) = self.find_mut(module) else {
            return Err(QuotaError::MissingModule(module));
        };
        let Some(next) = entry.used.checked_sub(amount) else {
            return Err(QuotaError::Underflow {
                module,
                used: entry.used,
                release: amount,
            });
        };

        entry.used = next;
        Ok(())
    }

    pub fn reset_usage(&mut self, module: ModuleId) -> Result<SystemBudget, QuotaError> {
        let Some(entry) = self.find_mut(module) else {
            return Err(QuotaError::MissingModule(module));
        };
        let released = entry.used;
        entry.used = SystemBudget::ZERO;
        Ok(released)
    }

    pub fn usage(&self, module: ModuleId) -> Option<SystemBudget> {
        self.find(module).map(|entry| entry.used)
    }

    pub fn limit(&self, module: ModuleId) -> Option<SystemBudget> {
        self.find(module).map(|entry| entry.limit)
    }

    pub fn available(&self, module: ModuleId) -> Option<SystemBudget> {
        let entry = self.find(module)?;
        entry.limit.checked_sub(entry.used)
    }

    pub fn total_used(&self) -> SystemBudget {
        let mut total = SystemBudget::ZERO;
        for entry in self.entries.iter().flatten() {
            total = total.checked_add(entry.used).unwrap_or(SystemBudget {
                flash_bytes: u32::MAX,
                ram_bytes: u32::MAX,
                pool_slots: u16::MAX,
            });
        }
        total
    }

    pub fn len(&self) -> usize {
        self.entries.iter().flatten().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn find(&self, module: ModuleId) -> Option<&QuotaEntry> {
        self.entries
            .iter()
            .flatten()
            .find(|entry| entry.module == module)
    }

    fn find_mut(&mut self, module: ModuleId) -> Option<&mut QuotaEntry> {
        self.entries
            .iter_mut()
            .flatten()
            .find(|entry| entry.module == module)
    }
}

impl<const N: usize> Default for QuotaLedger<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Capability, CapabilitySet, Criticality, DeadlineContract, MemoryBudget, ModuleSpec,
    };

    #[test]
    fn ledger_tracks_reserve_and_release_without_heap() {
        let mut ledger = QuotaLedger::<2>::new();
        ledger
            .register(ModuleId::Sensor, SystemBudget::new(1024, 256, 2))
            .unwrap();

        ledger
            .reserve(ModuleId::Sensor, SystemBudget::new(512, 128, 1))
            .unwrap();
        assert_eq!(
            ledger.usage(ModuleId::Sensor),
            Some(SystemBudget::new(512, 128, 1))
        );
        assert_eq!(
            ledger.available(ModuleId::Sensor),
            Some(SystemBudget::new(512, 128, 1))
        );

        ledger
            .release(ModuleId::Sensor, SystemBudget::new(128, 64, 1))
            .unwrap();
        assert_eq!(
            ledger.usage(ModuleId::Sensor),
            Some(SystemBudget::new(384, 64, 0))
        );
    }

    #[test]
    fn ledger_rejects_quota_overrun() {
        let mut ledger = QuotaLedger::<1>::new();
        ledger
            .register(ModuleId::Radio, SystemBudget::new(1024, 256, 1))
            .unwrap();

        assert_eq!(
            ledger.reserve(ModuleId::Radio, SystemBudget::new(512, 300, 0)),
            Err(QuotaError::Exceeded {
                module: ModuleId::Radio,
                used: SystemBudget::new(512, 300, 0),
                limit: SystemBudget::new(1024, 256, 1),
            })
        );
        assert_eq!(ledger.usage(ModuleId::Radio), Some(SystemBudget::ZERO));
    }

    #[test]
    fn multi_module_memory_budget_enforced_across_modules() {
        // Three modules share the system, each with its own budget. In-budget
        // reservations succeed and aggregate; one module overrunning its RAM limit is
        // rejected without disturbing the others' usage.
        let mut ledger = QuotaLedger::<3>::new();
        ledger
            .register(ModuleId::Sensor, SystemBudget::new(2048, 512, 4))
            .unwrap();
        ledger
            .register(ModuleId::Radio, SystemBudget::new(2048, 512, 4))
            .unwrap();
        ledger
            .register(ModuleId::Crypto, SystemBudget::new(2048, 512, 4))
            .unwrap();

        ledger
            .reserve(ModuleId::Sensor, SystemBudget::new(256, 200, 1))
            .unwrap();
        ledger
            .reserve(ModuleId::Radio, SystemBudget::new(256, 200, 1))
            .unwrap();
        ledger
            .reserve(ModuleId::Crypto, SystemBudget::new(256, 100, 1))
            .unwrap();

        // aggregate usage across all modules
        assert_eq!(ledger.total_used(), SystemBudget::new(768, 500, 3));

        // Crypto overrunning its own 512-byte RAM limit (100 + 500) is rejected; the
        // aggregate is unchanged.
        assert_eq!(
            ledger.reserve(ModuleId::Crypto, SystemBudget::new(0, 500, 0)),
            Err(QuotaError::Exceeded {
                module: ModuleId::Crypto,
                used: SystemBudget::new(256, 600, 1),
                limit: SystemBudget::new(2048, 512, 4),
            })
        );
        assert_eq!(ledger.total_used(), SystemBudget::new(768, 500, 3));
    }

    #[test]
    fn quota_holds_under_repeated_reservation_load() {
        // Many small reservations are admitted until the budget is exactly exhausted; the
        // next is rejected and usage never exceeds the limit.
        let mut ledger = QuotaLedger::<1>::new();
        ledger
            .register(ModuleId::Sensor, SystemBudget::new(10_000, 1_000, 0))
            .unwrap();
        for _ in 0..10 {
            ledger
                .reserve(ModuleId::Sensor, SystemBudget::new(0, 100, 0))
                .unwrap();
        }
        assert_eq!(
            ledger.usage(ModuleId::Sensor),
            Some(SystemBudget::new(0, 1_000, 0))
        );
        assert!(ledger
            .reserve(ModuleId::Sensor, SystemBudget::new(0, 100, 0))
            .is_err());
        assert_eq!(
            ledger.usage(ModuleId::Sensor),
            Some(SystemBudget::new(0, 1_000, 0))
        );
    }

    #[test]
    fn freed_quota_is_reallocated() {
        // One module reserves heavily then releases; the freed capacity drops from the
        // aggregate and becomes available again, so another module can claim fresh
        // capacity - dynamic reallocation over time.
        let mut ledger = QuotaLedger::<2>::new();
        ledger
            .register(ModuleId::Sensor, SystemBudget::new(4_096, 2_048, 4))
            .unwrap();
        ledger
            .register(ModuleId::Radio, SystemBudget::new(4_096, 2_048, 4))
            .unwrap();

        ledger
            .reserve(ModuleId::Sensor, SystemBudget::new(0, 2_000, 0))
            .unwrap();
        assert_eq!(ledger.total_used(), SystemBudget::new(0, 2_000, 0));
        ledger
            .release(ModuleId::Sensor, SystemBudget::new(0, 1_800, 0))
            .unwrap();
        assert_eq!(ledger.total_used(), SystemBudget::new(0, 200, 0));
        assert_eq!(
            ledger.available(ModuleId::Sensor),
            Some(SystemBudget::new(4_096, 1_848, 4))
        );
        ledger
            .reserve(ModuleId::Radio, SystemBudget::new(0, 1_500, 0))
            .unwrap();
        assert_eq!(ledger.total_used(), SystemBudget::new(0, 1_700, 0));
    }

    #[test]
    fn ledger_rejects_release_underflow() {
        let mut ledger = QuotaLedger::<1>::new();
        ledger
            .register(ModuleId::Bus, SystemBudget::new(128, 64, 0))
            .unwrap();

        assert_eq!(
            ledger.release(ModuleId::Bus, SystemBudget::new(1, 0, 0)),
            Err(QuotaError::Underflow {
                module: ModuleId::Bus,
                used: SystemBudget::ZERO,
                release: SystemBudget::new(1, 0, 0),
            })
        );
    }

    #[test]
    fn ledger_can_reset_module_usage() {
        let mut ledger = QuotaLedger::<1>::new();
        ledger
            .register(ModuleId::Sensor, SystemBudget::new(1024, 256, 2))
            .unwrap();
        ledger
            .reserve(ModuleId::Sensor, SystemBudget::new(512, 128, 1))
            .unwrap();

        assert_eq!(
            ledger.reset_usage(ModuleId::Sensor),
            Ok(SystemBudget::new(512, 128, 1))
        );
        assert_eq!(ledger.usage(ModuleId::Sensor), Some(SystemBudget::ZERO));
        assert_eq!(ledger.total_used(), SystemBudget::ZERO);
    }

    #[test]
    fn ledger_can_be_seeded_from_manifest() {
        let mut manifest = SystemManifest::<2>::new();
        manifest
            .add(
                ModuleSpec::new(ModuleId::Kernel, Criticality::HardRealtime)
                    .owns(CapabilitySet::empty().with(Capability::Timebase))
                    .memory(MemoryBudget::new(1024, 512, 1))
                    .deadline(DeadlineContract::new(1000, 10)),
            )
            .unwrap();
        manifest
            .add(
                ModuleSpec::new(ModuleId::App(7), Criticality::User)
                    .requires(CapabilitySet::empty().with(Capability::Timebase))
                    .memory(MemoryBudget::new(2048, 256, 0)),
            )
            .unwrap();

        let mut ledger = QuotaLedger::<2>::new();
        ledger.register_manifest(&manifest).unwrap();

        assert_eq!(ledger.len(), 2);
        assert_eq!(
            ledger.limit(ModuleId::App(7)),
            Some(SystemBudget::new(2048, 256, 0))
        );
    }
}
