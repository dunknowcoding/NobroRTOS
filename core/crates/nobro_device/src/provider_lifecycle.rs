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
    OperationActive,
    NoActiveOperation,
    StaleOperation,
    InvalidDeadline,
    InvalidProgress,
    DeadlineExpired,
    ReceiptExhausted,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderOperation {
    generation: u32,
    id: u32,
    deadline_us: u64,
    total_units: u32,
}

impl ProviderOperation {
    pub const fn generation(self) -> u32 {
        self.generation
    }

    pub const fn id(self) -> u32 {
        self.id
    }

    pub const fn deadline_us(self) -> u64 {
        self.deadline_us
    }

    pub const fn total_units(self) -> u32 {
        self.total_units
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderProgress {
    pub generation: u32,
    pub operation_id: u32,
    pub completed_units: u32,
    pub total_units: u32,
}

impl ProviderProgress {
    pub const fn complete(self) -> bool {
        self.completed_units == self.total_units
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderCancellationReceipt {
    pub generation: u32,
    pub operation_id: u32,
    pub completed_units: u32,
    pub total_units: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderFaultKind {
    Backend,
    Deadline,
    Protocol,
    ResetRequested,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderFaultReceipt {
    pub sequence: u32,
    pub generation: u32,
    pub operation_id: u32,
    pub completed_units: u32,
    pub kind: ProviderFaultKind,
    pub observed_at_us: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderResetReceipt {
    pub fault: ProviderFaultReceipt,
    pub recovery: ProviderRecoveryReceipt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ActiveOperation {
    token: ProviderOperation,
    completed_units: u32,
}

/// Fixed-size lifecycle ledger shared by board and library provider bindings.
pub struct ProviderLifecycle {
    generation: u32,
    state: ProviderLifecycleState,
    owned: ProviderOwnedResources,
    next_operation_id: u32,
    next_fault_sequence: u32,
    operation: Option<ActiveOperation>,
    last_fault: Option<ProviderFaultReceipt>,
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
            next_operation_id: 1,
            next_fault_sequence: 1,
            operation: None,
            last_fault: None,
        }
    }

    pub const fn state(&self) -> ProviderLifecycleState {
        self.state
    }

    pub const fn generation(&self) -> u32 {
        self.generation
    }

    pub const fn last_fault_receipt(&self) -> Option<ProviderFaultReceipt> {
        self.last_fault
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
        if self.operation.is_some() {
            return Err(ProviderLifecycleError::OperationActive);
        }
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
        if self.operation.is_some() {
            return Err(ProviderLifecycleError::OperationActive);
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
        self.operation = None;
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

    /// Begin one bounded operation owned by the current provider generation.
    pub fn begin_operation(
        &mut self,
        session: ProviderSession,
        now_us: u64,
        deadline_us: u64,
        total_units: u32,
    ) -> Result<ProviderOperation, ProviderLifecycleError> {
        self.validate(session)?;
        if self.operation.is_some() {
            return Err(ProviderLifecycleError::OperationActive);
        }
        if deadline_us <= now_us {
            return Err(ProviderLifecycleError::InvalidDeadline);
        }
        if total_units == 0 {
            return Err(ProviderLifecycleError::InvalidProgress);
        }
        let id = self.next_operation_id;
        self.next_operation_id = id
            .checked_add(1)
            .ok_or(ProviderLifecycleError::ReceiptExhausted)?;
        let token = ProviderOperation {
            generation: self.generation,
            id,
            deadline_us,
            total_units,
        };
        self.operation = Some(ActiveOperation {
            token,
            completed_units: 0,
        });
        Ok(token)
    }

    /// Publish monotonic partial progress. A deadline fault is receipted and
    /// invalidates the active operation before the error is returned.
    pub fn advance_operation(
        &mut self,
        session: ProviderSession,
        operation: ProviderOperation,
        completed_units: u32,
        now_us: u64,
    ) -> Result<ProviderProgress, ProviderLifecycleError> {
        self.validate_operation(session, operation)?;
        if now_us > operation.deadline_us {
            self.record_fault(session, ProviderFaultKind::Deadline, now_us)?;
            return Err(ProviderLifecycleError::DeadlineExpired);
        }
        let active = self
            .operation
            .as_mut()
            .ok_or(ProviderLifecycleError::NoActiveOperation)?;
        if completed_units < active.completed_units || completed_units > operation.total_units {
            return Err(ProviderLifecycleError::InvalidProgress);
        }
        active.completed_units = completed_units;
        Ok(ProviderProgress {
            generation: operation.generation,
            operation_id: operation.id,
            completed_units,
            total_units: operation.total_units,
        })
    }

    /// Finish a fully progressed operation and release its operation slot.
    pub fn finish_operation(
        &mut self,
        session: ProviderSession,
        operation: ProviderOperation,
        now_us: u64,
    ) -> Result<ProviderProgress, ProviderLifecycleError> {
        let progress = self.advance_operation(session, operation, operation.total_units, now_us)?;
        self.operation = None;
        Ok(progress)
    }

    /// Cancel an operation while preserving an exact partial-progress receipt.
    pub fn cancel_operation(
        &mut self,
        session: ProviderSession,
        operation: ProviderOperation,
    ) -> Result<ProviderCancellationReceipt, ProviderLifecycleError> {
        self.validate_operation(session, operation)?;
        let active = self
            .operation
            .take()
            .ok_or(ProviderLifecycleError::NoActiveOperation)?;
        Ok(ProviderCancellationReceipt {
            generation: operation.generation,
            operation_id: operation.id,
            completed_units: active.completed_units,
            total_units: operation.total_units,
        })
    }

    /// Record a typed provider fault and invalidate any active operation.
    pub fn record_fault(
        &mut self,
        session: ProviderSession,
        kind: ProviderFaultKind,
        observed_at_us: u64,
    ) -> Result<ProviderFaultReceipt, ProviderLifecycleError> {
        if session.generation != self.generation {
            return Err(ProviderLifecycleError::StaleSession);
        }
        if self.state == ProviderLifecycleState::Down {
            return Err(ProviderLifecycleError::NotMounted);
        }
        let sequence = self.next_fault_sequence;
        self.next_fault_sequence = sequence
            .checked_add(1)
            .ok_or(ProviderLifecycleError::ReceiptExhausted)?;
        let active = self.operation.take();
        let receipt = ProviderFaultReceipt {
            sequence,
            generation: self.generation,
            operation_id: active.map(|value| value.token.id).unwrap_or(0),
            completed_units: active.map(|value| value.completed_units).unwrap_or(0),
            kind,
            observed_at_us,
        };
        self.last_fault = Some(receipt);
        Ok(receipt)
    }

    /// Fault, quiesce, release, and remount in one generation-invalidating reset.
    pub fn reset(
        &mut self,
        session: ProviderSession,
        cleaned: ProviderOwnedResources,
        remounted: ProviderOwnedResources,
        observed_at_us: u64,
    ) -> Result<ProviderResetReceipt, ProviderLifecycleError> {
        let fault =
            self.record_fault(session, ProviderFaultKind::ResetRequested, observed_at_us)?;
        let recovery = self.recover(session, cleaned, remounted)?;
        Ok(ProviderResetReceipt { fault, recovery })
    }

    fn validate_operation(
        &self,
        session: ProviderSession,
        operation: ProviderOperation,
    ) -> Result<(), ProviderLifecycleError> {
        self.validate(session)?;
        if operation.generation != self.generation {
            return Err(ProviderLifecycleError::StaleOperation);
        }
        let active = self
            .operation
            .ok_or(ProviderLifecycleError::NoActiveOperation)?;
        if active.token != operation {
            return Err(ProviderLifecycleError::StaleOperation);
        }
        Ok(())
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

    #[test]
    fn operation_progress_is_monotonic_and_cancellation_is_receipted() {
        let mut lifecycle = ProviderLifecycle::new();
        let session = lifecycle.mount(OWNED).unwrap();
        let operation = lifecycle.begin_operation(session, 100, 200, 8).unwrap();
        let progress = lifecycle
            .advance_operation(session, operation, 3, 150)
            .unwrap();
        assert_eq!(progress.completed_units, 3);
        assert!(!progress.complete());
        assert_eq!(
            lifecycle.advance_operation(session, operation, 2, 160),
            Err(ProviderLifecycleError::InvalidProgress)
        );
        let cancelled = lifecycle.cancel_operation(session, operation).unwrap();
        assert_eq!(cancelled.completed_units, 3);
        assert_eq!(cancelled.total_units, 8);
        assert_eq!(
            lifecycle.advance_operation(session, operation, 4, 170),
            Err(ProviderLifecycleError::NoActiveOperation)
        );
    }

    #[test]
    fn deadline_fault_invalidates_operation_and_preserves_partial_progress() {
        let mut lifecycle = ProviderLifecycle::new();
        let session = lifecycle.mount(OWNED).unwrap();
        let operation = lifecycle.begin_operation(session, 100, 200, 8).unwrap();
        lifecycle
            .advance_operation(session, operation, 5, 150)
            .unwrap();
        assert_eq!(
            lifecycle.advance_operation(session, operation, 6, 201),
            Err(ProviderLifecycleError::DeadlineExpired)
        );
        let fault = lifecycle.last_fault_receipt().unwrap();
        assert_eq!(fault.kind, ProviderFaultKind::Deadline);
        assert_eq!(fault.operation_id, operation.id());
        assert_eq!(fault.completed_units, 5);
        assert_eq!(
            lifecycle.cancel_operation(session, operation),
            Err(ProviderLifecycleError::NoActiveOperation)
        );
    }

    #[test]
    fn reset_faults_cleans_and_remounts_with_fresh_generation() {
        let mut lifecycle = ProviderLifecycle::new();
        let old = lifecycle.mount(OWNED).unwrap();
        let operation = lifecycle.begin_operation(old, 10, 100, 4).unwrap();
        lifecycle.advance_operation(old, operation, 2, 20).unwrap();

        let reset = lifecycle.reset(old, OWNED, OWNED, 21).unwrap();
        assert_eq!(reset.fault.kind, ProviderFaultKind::ResetRequested);
        assert_eq!(reset.fault.completed_units, 2);
        assert_eq!(
            lifecycle.validate(old),
            Err(ProviderLifecycleError::StaleSession)
        );
        assert_eq!(lifecycle.validate(reset.recovery.session), Ok(()));
        assert!(reset.recovery.release.application_static_cleanup_complete);
    }

    #[test]
    fn quiesce_rejects_an_uncancelled_operation() {
        let mut lifecycle = ProviderLifecycle::new();
        let session = lifecycle.mount(OWNED).unwrap();
        let operation = lifecycle.begin_operation(session, 10, 100, 1).unwrap();
        assert_eq!(
            lifecycle.quiesce(session),
            Err(ProviderLifecycleError::OperationActive)
        );
        lifecycle.cancel_operation(session, operation).unwrap();
        assert!(lifecycle.quiesce(session).is_ok());
    }

    #[test]
    fn invalid_and_stale_operation_tokens_fail_closed() {
        let mut lifecycle = ProviderLifecycle::new();
        let session = lifecycle.mount(OWNED).unwrap();
        assert_eq!(
            lifecycle.begin_operation(session, 10, 10, 1),
            Err(ProviderLifecycleError::InvalidDeadline)
        );
        assert_eq!(
            lifecycle.begin_operation(session, 10, 20, 0),
            Err(ProviderLifecycleError::InvalidProgress)
        );

        let first = lifecycle.begin_operation(session, 10, 20, 1).unwrap();
        lifecycle.cancel_operation(session, first).unwrap();
        let second = lifecycle.begin_operation(session, 20, 30, 1).unwrap();
        assert_eq!(
            lifecycle.advance_operation(session, first, 1, 21),
            Err(ProviderLifecycleError::StaleOperation)
        );
        assert_eq!(
            lifecycle
                .cancel_operation(session, second)
                .unwrap()
                .completed_units,
            0
        );
    }
}
