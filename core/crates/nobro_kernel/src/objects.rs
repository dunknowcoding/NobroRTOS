//! Automatic per-module kernel-object accounting.
//!
//! Every mailbox slot, alarm, and KV entry a module holds is charged against its
//! manifest [`ObjectQuota`](crate::ObjectQuota) when the object is created and
//! released when it is consumed, cancelled, or cleaned up — the caller cannot skip
//! the accounting because the runtime's own operation paths perform it. The kernel
//! module is exempt: kernel-origin bookkeeping must never be starved by policy.

use crate::{ModuleId, ObjectQuota};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectKind {
    MailboxSlot,
    Alarm,
    KvEntry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectQuotaError {
    MissingModule(ModuleId),
    Exceeded {
        module: ModuleId,
        kind: ObjectKind,
        limit: u8,
    },
    /// A release found no matching charge — a reconciliation invariant failure.
    Underflow {
        module: ModuleId,
        kind: ObjectKind,
    },
    /// Cleanup finished but charges remained — a resource-leak invariant failure.
    Leak {
        module: ModuleId,
        mailbox_slots: u8,
        alarms: u8,
        kv_entries: u8,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectUsage {
    pub mailbox_slots: u8,
    pub alarms: u8,
    pub kv_entries: u8,
    /// Accumulated measured execution time (charged by the executor).
    pub cpu_us: u64,
}

impl ObjectUsage {
    pub const ZERO: Self = Self {
        mailbox_slots: 0,
        alarms: 0,
        kv_entries: 0,
        cpu_us: 0,
    };
}

#[derive(Clone, Copy, Debug)]
struct ObjectEntry {
    module: ModuleId,
    quota: ObjectQuota,
    usage: ObjectUsage,
}

/// Fixed-capacity ledger of kernel-object charges, one entry per admitted module.
#[derive(Debug)]
pub struct ObjectLedger<const N: usize> {
    entries: [Option<ObjectEntry>; N],
}

impl<const N: usize> ObjectLedger<N> {
    pub const fn new() -> Self {
        Self { entries: [None; N] }
    }

    pub fn register(&mut self, module: ModuleId, quota: ObjectQuota) {
        if let Some(entry) = self.find_mut(module) {
            entry.quota = quota;
            return;
        }
        if let Some(slot) = self.entries.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(ObjectEntry {
                module,
                quota,
                usage: ObjectUsage::ZERO,
            });
        }
    }

    pub fn charge(&mut self, module: ModuleId, kind: ObjectKind) -> Result<(), ObjectQuotaError> {
        if module == ModuleId::Kernel {
            return Ok(());
        }
        let Some(entry) = self.find_mut(module) else {
            return Err(ObjectQuotaError::MissingModule(module));
        };
        let (used, limit) = Self::select(entry, kind);
        if *used >= limit {
            return Err(ObjectQuotaError::Exceeded {
                module,
                kind,
                limit,
            });
        }
        *used += 1;
        Ok(())
    }

    pub fn release(&mut self, module: ModuleId, kind: ObjectKind) -> Result<(), ObjectQuotaError> {
        if module == ModuleId::Kernel {
            return Ok(());
        }
        let Some(entry) = self.find_mut(module) else {
            return Err(ObjectQuotaError::MissingModule(module));
        };
        let (used, _) = Self::select(entry, kind);
        if *used == 0 {
            return Err(ObjectQuotaError::Underflow { module, kind });
        }
        *used -= 1;
        Ok(())
    }

    /// Accumulate measured execution time. Saturating: CPU accounting is
    /// evidence, and losing precision must never fault the executor.
    pub fn charge_cpu(&mut self, module: ModuleId, duration_us: u32) {
        if let Some(entry) = self.find_mut(module) {
            entry.usage.cpu_us = entry.usage.cpu_us.saturating_add(u64::from(duration_us));
        }
    }

    pub fn usage(&self, module: ModuleId) -> Option<ObjectUsage> {
        self.find(module).map(|entry| entry.usage)
    }

    pub fn quota(&self, module: ModuleId) -> Option<ObjectQuota> {
        self.find(module).map(|entry| entry.quota)
    }

    /// Verify a module holds no objects (post-cleanup invariant). CPU time is
    /// history, not a held resource, so it does not count as a leak.
    pub fn verify_clear(&self, module: ModuleId) -> Result<(), ObjectQuotaError> {
        let Some(entry) = self.find(module) else {
            return Ok(());
        };
        let usage = entry.usage;
        if usage.mailbox_slots != 0 || usage.alarms != 0 || usage.kv_entries != 0 {
            return Err(ObjectQuotaError::Leak {
                module,
                mailbox_slots: usage.mailbox_slots,
                alarms: usage.alarms,
                kv_entries: usage.kv_entries,
            });
        }
        Ok(())
    }

    fn select(entry: &mut ObjectEntry, kind: ObjectKind) -> (&mut u8, u8) {
        match kind {
            ObjectKind::MailboxSlot => (&mut entry.usage.mailbox_slots, entry.quota.mailbox_slots),
            ObjectKind::Alarm => (&mut entry.usage.alarms, entry.quota.alarms),
            ObjectKind::KvEntry => (&mut entry.usage.kv_entries, entry.quota.kv_entries),
        }
    }

    fn find(&self, module: ModuleId) -> Option<&ObjectEntry> {
        self.entries
            .iter()
            .flatten()
            .find(|entry| entry.module == module)
    }

    fn find_mut(&mut self, module: ModuleId) -> Option<&mut ObjectEntry> {
        self.entries
            .iter_mut()
            .flatten()
            .find(|entry| entry.module == module)
    }
}

impl<const N: usize> Default for ObjectLedger<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_charges_to_the_limit_then_rejects() {
        let mut ledger = ObjectLedger::<2>::new();
        ledger.register(ModuleId::Sensor, ObjectQuota::new(2, 1, 1));

        assert_eq!(
            ledger.charge(ModuleId::Sensor, ObjectKind::MailboxSlot),
            Ok(())
        );
        assert_eq!(
            ledger.charge(ModuleId::Sensor, ObjectKind::MailboxSlot),
            Ok(())
        );
        assert_eq!(
            ledger.charge(ModuleId::Sensor, ObjectKind::MailboxSlot),
            Err(ObjectQuotaError::Exceeded {
                module: ModuleId::Sensor,
                kind: ObjectKind::MailboxSlot,
                limit: 2,
            })
        );
        assert_eq!(
            ledger.release(ModuleId::Sensor, ObjectKind::MailboxSlot),
            Ok(())
        );
        assert_eq!(
            ledger.charge(ModuleId::Sensor, ObjectKind::MailboxSlot),
            Ok(())
        );
    }

    #[test]
    fn kernel_is_exempt_and_release_underflow_is_an_invariant_failure() {
        let mut ledger = ObjectLedger::<1>::new();
        ledger.register(ModuleId::Radio, ObjectQuota::new(1, 1, 1));

        for _ in 0..100 {
            assert_eq!(ledger.charge(ModuleId::Kernel, ObjectKind::Alarm), Ok(()));
        }
        assert_eq!(
            ledger.release(ModuleId::Radio, ObjectKind::Alarm),
            Err(ObjectQuotaError::Underflow {
                module: ModuleId::Radio,
                kind: ObjectKind::Alarm,
            })
        );
    }

    #[test]
    fn verify_clear_reports_leaks() {
        let mut ledger = ObjectLedger::<1>::new();
        ledger.register(ModuleId::Crypto, ObjectQuota::DEFAULT);
        ledger
            .charge(ModuleId::Crypto, ObjectKind::KvEntry)
            .unwrap();
        assert_eq!(
            ledger.verify_clear(ModuleId::Crypto),
            Err(ObjectQuotaError::Leak {
                module: ModuleId::Crypto,
                mailbox_slots: 0,
                alarms: 0,
                kv_entries: 1,
            })
        );
        ledger
            .release(ModuleId::Crypto, ObjectKind::KvEntry)
            .unwrap();
        ledger.charge_cpu(ModuleId::Crypto, 250);
        assert_eq!(ledger.verify_clear(ModuleId::Crypto), Ok(()));
        assert_eq!(ledger.usage(ModuleId::Crypto).unwrap().cpu_us, 250);
    }
}
