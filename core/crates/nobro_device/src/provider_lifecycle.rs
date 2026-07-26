//! Portable provider-generation and cleanup receipts.
//!
//! Device backends keep their hardware-specific init, reset, and I/O logic.
//! This state machine supplies the common ownership boundary: a session is
//! generation-tagged, quiesce is explicit, release proves every declared
//! application-static object was cleaned, and recovery cannot revive an old
//! session.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderSession {
    generation: u32,
}

impl ProviderSession {
    pub const fn generation(self) -> u32 {
        self.generation
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProviderOwnedResources {
    pub leases: u16,
    pub callbacks: u16,
    pub application_static_objects: u16,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProviderLifecycleState {
    #[default]
    Down,
    Ready,
    Quiesced,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderLifecycleError {
    AlreadyMounted,
    NotMounted,
    NotQuiesced,
    StaleSession,
    CleanupMismatch,
    GenerationExhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderQuiesceReceipt {
    pub generation: u32,
    pub owned: ProviderOwnedResources,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderReleaseReceipt {
    pub invalidated_generation: u32,
    pub next_generation: u32,
    pub cleaned: ProviderOwnedResources,
    pub application_static_cleanup_complete: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderRecoveryReceipt {
    pub release: ProviderReleaseReceipt,
    pub session: ProviderSession,
}

/// Fixed-size lifecycle ledger shared by board and library provider bindings.
pub struct ProviderLifecycle {
    generation: u32,
    state: ProviderLifecycleState,
    owned: ProviderOwnedResources,
}

impl ProviderLifecycle {
    pub const fn new() -> Self {
        Self {
            generation: 1,
            state: ProviderLifecycleState::Down,
            owned: ProviderOwnedResources {
                leases: 0,
                callbacks: 0,
                application_static_objects: 0,
            },
        }
    }

    pub const fn state(&self) -> ProviderLifecycleState {
        self.state
    }

    pub const fn generation(&self) -> u32 {
        self.generation
    }

    pub fn mount(
        &mut self,
        owned: ProviderOwnedResources,
    ) -> Result<ProviderSession, ProviderLifecycleError> {
        if self.state != ProviderLifecycleState::Down {
            return Err(ProviderLifecycleError::AlreadyMounted);
        }
        self.owned = owned;
        self.state = ProviderLifecycleState::Ready;
        Ok(ProviderSession {
            generation: self.generation,
        })
    }

    /// Validate an I/O or callback completion against the current generation.
    pub fn validate(&self, session: ProviderSession) -> Result<(), ProviderLifecycleError> {
        if self.state == ProviderLifecycleState::Down {
            return Err(ProviderLifecycleError::NotMounted);
        }
        if session.generation != self.generation {
            return Err(ProviderLifecycleError::StaleSession);
        }
        if self.state != ProviderLifecycleState::Ready {
            return Err(ProviderLifecycleError::NotMounted);
        }
        Ok(())
    }

    pub fn quiesce(
        &mut self,
        session: ProviderSession,
    ) -> Result<ProviderQuiesceReceipt, ProviderLifecycleError> {
        self.validate(session)?;
        self.state = ProviderLifecycleState::Quiesced;
        Ok(ProviderQuiesceReceipt {
            generation: self.generation,
            owned: self.owned,
        })
    }

    pub fn resume(&mut self, session: ProviderSession) -> Result<(), ProviderLifecycleError> {
        if session.generation != self.generation {
            return Err(ProviderLifecycleError::StaleSession);
        }
        if self.state != ProviderLifecycleState::Quiesced {
            return Err(ProviderLifecycleError::NotQuiesced);
        }
        self.state = ProviderLifecycleState::Ready;
        Ok(())
    }

    /// Release a quiesced provider and invalidate every outstanding session.
    ///
    /// `cleaned` is supplied by the binding after it disables callbacks/DMA,
    /// revokes leases, and clears application-static registrations. A mismatch
    /// fails closed and leaves the provider quiesced for retry or escalation.
    pub fn release(
        &mut self,
        session: ProviderSession,
        cleaned: ProviderOwnedResources,
    ) -> Result<ProviderReleaseReceipt, ProviderLifecycleError> {
        if session.generation != self.generation {
            return Err(ProviderLifecycleError::StaleSession);
        }
        if self.state != ProviderLifecycleState::Quiesced {
            return Err(ProviderLifecycleError::NotQuiesced);
        }
        if cleaned != self.owned {
            return Err(ProviderLifecycleError::CleanupMismatch);
        }
        let invalidated_generation = self.generation;
        let next_generation = self
            .generation
            .checked_add(1)
            .ok_or(ProviderLifecycleError::GenerationExhausted)?;
        self.generation = next_generation;
        self.state = ProviderLifecycleState::Down;
        self.owned = ProviderOwnedResources::default();
        Ok(ProviderReleaseReceipt {
            invalidated_generation,
            next_generation,
            cleaned,
            application_static_cleanup_complete: true,
        })
    }

    /// Quiesce, release, and remount with a fresh generation.
    pub fn recover(
        &mut self,
        session: ProviderSession,
        cleaned: ProviderOwnedResources,
        remounted: ProviderOwnedResources,
    ) -> Result<ProviderRecoveryReceipt, ProviderLifecycleError> {
        if self.state == ProviderLifecycleState::Ready {
            self.quiesce(session)?;
        }
        let release = self.release(session, cleaned)?;
        let session = self.mount(remounted)?;
        Ok(ProviderRecoveryReceipt { release, session })
    }
}

impl Default for ProviderLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNED: ProviderOwnedResources = ProviderOwnedResources {
        leases: 2,
        callbacks: 1,
        application_static_objects: 3,
    };

    #[test]
    fn release_invalidates_generation_and_receipts_static_cleanup() {
        let mut lifecycle = ProviderLifecycle::new();
        let old = lifecycle.mount(OWNED).unwrap();
        let quiesced = lifecycle.quiesce(old).unwrap();
        assert_eq!(quiesced.owned, OWNED);
        let receipt = lifecycle.release(old, OWNED).unwrap();
        assert_eq!(receipt.invalidated_generation, old.generation());
        assert!(receipt.application_static_cleanup_complete);

        let current = lifecycle.mount(OWNED).unwrap();
        assert_ne!(current.generation(), old.generation());
        assert_eq!(
            lifecycle.validate(old),
            Err(ProviderLifecycleError::StaleSession)
        );
        assert_eq!(lifecycle.validate(current), Ok(()));
    }

    #[test]
    fn incomplete_cleanup_leaves_provider_quiesced() {
        let mut lifecycle = ProviderLifecycle::new();
        let session = lifecycle.mount(OWNED).unwrap();
        lifecycle.quiesce(session).unwrap();
        assert_eq!(
            lifecycle.release(
                session,
                ProviderOwnedResources {
                    application_static_objects: 2,
                    ..OWNED
                }
            ),
            Err(ProviderLifecycleError::CleanupMismatch)
        );
        assert_eq!(lifecycle.state(), ProviderLifecycleState::Quiesced);
        assert_eq!(lifecycle.generation(), session.generation());
    }

    #[test]
    fn recovery_remounts_with_a_fresh_session() {
        let mut lifecycle = ProviderLifecycle::new();
        let old = lifecycle.mount(OWNED).unwrap();
        let receipt = lifecycle.recover(old, OWNED, OWNED).unwrap();
        assert_eq!(receipt.release.invalidated_generation, old.generation());
        assert_eq!(lifecycle.validate(receipt.session), Ok(()));
        assert_eq!(
            lifecycle.validate(old),
            Err(ProviderLifecycleError::StaleSession)
        );
    }
}
