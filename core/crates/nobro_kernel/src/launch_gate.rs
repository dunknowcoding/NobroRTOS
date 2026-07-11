//! Fail-closed authorization state for foreign modules and other ABI boundaries.

use core::sync::atomic::{AtomicU32, Ordering};

use crate::{Capability, CapabilitySet};

/// Stores the capabilities of the currently admitted foreign module.
///
/// A newly constructed or revoked gate denies every operation. Grants should
/// only be installed after the complete boot/admission path has succeeded.
#[derive(Debug)]
pub struct ModuleLaunchGate {
    granted: AtomicU32,
}

impl ModuleLaunchGate {
    pub const fn new() -> Self {
        Self {
            granted: AtomicU32::new(0),
        }
    }

    pub fn install(&self, granted: CapabilitySet) {
        self.granted.store(granted.bits(), Ordering::Release);
    }

    pub fn revoke(&self) {
        self.granted.store(0, Ordering::Release);
    }

    pub fn allows(&self, capability: Capability) -> bool {
        (self.granted.load(Ordering::Acquire) & capability.bit()) != 0
    }

    pub fn is_admitted(&self) -> bool {
        self.granted.load(Ordering::Acquire) != 0
    }
}

impl Default for ModuleLaunchGate {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_and_returns_to_fail_closed_state() {
        let gate = ModuleLaunchGate::new();
        assert!(!gate.is_admitted());
        assert!(!gate.allows(Capability::Bus0));

        gate.install(
            CapabilitySet::empty()
                .with(Capability::Bus0)
                .with(Capability::Timebase),
        );
        assert!(gate.is_admitted());
        assert!(gate.allows(Capability::Bus0));
        assert!(gate.allows(Capability::Timebase));
        assert!(!gate.allows(Capability::Radio));

        gate.revoke();
        assert!(!gate.is_admitted());
        assert!(!gate.allows(Capability::Bus0));
    }
}
