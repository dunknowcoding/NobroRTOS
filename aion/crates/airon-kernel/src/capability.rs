//! Runtime capability grants derived from the static system manifest.

use crate::{Capability, CapabilitySet, ModuleId, SystemManifest};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapabilityGrant {
    pub module: ModuleId,
    pub granted: CapabilitySet,
}

impl CapabilityGrant {
    pub const fn new(module: ModuleId, granted: CapabilitySet) -> Self {
        Self { module, granted }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityGrantError {
    Full,
    Duplicate(ModuleId),
    Missing(ModuleId),
    Denied {
        module: ModuleId,
        capability: Capability,
    },
}

pub struct CapabilityGrantTable<const N: usize> {
    grants: [Option<CapabilityGrant>; N],
}

impl<const N: usize> CapabilityGrantTable<N> {
    pub const fn new() -> Self {
        Self { grants: [None; N] }
    }

    pub fn register(
        &mut self,
        module: ModuleId,
        granted: CapabilitySet,
    ) -> Result<(), CapabilityGrantError> {
        if self.find(module).is_some() {
            return Err(CapabilityGrantError::Duplicate(module));
        }

        let Some(slot) = self.grants.iter_mut().find(|slot| slot.is_none()) else {
            return Err(CapabilityGrantError::Full);
        };
        *slot = Some(CapabilityGrant::new(module, granted));
        Ok(())
    }

    pub fn from_manifest<const M: usize>(
        manifest: &SystemManifest<M>,
    ) -> Result<Self, CapabilityGrantError> {
        let mut table = Self::new();
        for spec in manifest.iter() {
            table.register(spec.id, spec.requires.union(spec.owns))?;
        }
        Ok(table)
    }

    pub fn authorize(
        &self,
        module: ModuleId,
        capability: Capability,
    ) -> Result<(), CapabilityGrantError> {
        let Some(grant) = self.find(module) else {
            return Err(CapabilityGrantError::Missing(module));
        };
        if grant.granted.contains(capability) {
            Ok(())
        } else {
            Err(CapabilityGrantError::Denied { module, capability })
        }
    }

    pub fn granted(&self, module: ModuleId) -> Option<CapabilitySet> {
        self.find(module).map(|grant| grant.granted)
    }

    pub fn len(&self) -> usize {
        self.grants.iter().flatten().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn find(&self, module: ModuleId) -> Option<&CapabilityGrant> {
        self.grants
            .iter()
            .flatten()
            .find(|grant| grant.module == module)
    }
}

impl<const N: usize> Default for CapabilityGrantTable<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        kernel_owned_capabilities, Criticality, DeadlineContract, FaultThresholds, MemoryBudget,
        ModuleSpec, SystemManifest,
    };

    fn kernel_spec() -> ModuleSpec {
        ModuleSpec::new(ModuleId::Kernel, Criticality::HardRealtime)
            .owns(kernel_owned_capabilities())
            .memory(MemoryBudget::new(16 * 1024, 4 * 1024, 4))
            .deadline(DeadlineContract::new(20_000, 10))
            .fault_thresholds(FaultThresholds {
                notify_after: 2,
                reboot_after: 4,
            })
    }

    fn sensor_spec() -> ModuleSpec {
        ModuleSpec::new(ModuleId::Sensor, Criticality::Driver)
            .requires(
                CapabilitySet::empty()
                    .with(Capability::Bus0)
                    .with(Capability::SamplePool),
            )
            .owns(CapabilitySet::empty().with(Capability::Bus0))
            .memory(MemoryBudget::new(8 * 1024, 2 * 1024, 2))
    }

    #[test]
    fn grant_table_authorizes_declared_capabilities() {
        let manifest = SystemManifest::<2>::from_specs(&[kernel_spec(), sensor_spec()]).unwrap();
        let grants = CapabilityGrantTable::<2>::from_manifest(&manifest).unwrap();

        assert_eq!(grants.len(), 2);
        assert_eq!(grants.authorize(ModuleId::Sensor, Capability::Bus0), Ok(()));
        assert_eq!(
            grants.authorize(ModuleId::Sensor, Capability::SamplePool),
            Ok(())
        );
    }

    #[test]
    fn grant_table_denies_undeclared_capability() {
        let manifest = SystemManifest::<2>::from_specs(&[kernel_spec(), sensor_spec()]).unwrap();
        let grants = CapabilityGrantTable::<2>::from_manifest(&manifest).unwrap();

        assert_eq!(
            grants.authorize(ModuleId::Sensor, Capability::Radio),
            Err(CapabilityGrantError::Denied {
                module: ModuleId::Sensor,
                capability: Capability::Radio,
            })
        );
    }

    #[test]
    fn grant_table_reports_missing_module() {
        let grants = CapabilityGrantTable::<1>::new();

        assert_eq!(
            grants.authorize(ModuleId::App(4), Capability::HostReport),
            Err(CapabilityGrantError::Missing(ModuleId::App(4)))
        );
    }

    #[test]
    fn grant_table_preserves_duplicate_errors() {
        let mut grants = CapabilityGrantTable::<2>::new();
        grants
            .register(ModuleId::Kernel, kernel_owned_capabilities())
            .unwrap();

        assert_eq!(
            grants.register(ModuleId::Kernel, CapabilitySet::empty()),
            Err(CapabilityGrantError::Duplicate(ModuleId::Kernel))
        );
    }
}
