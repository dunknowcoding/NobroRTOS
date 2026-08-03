//! Bounded lifecycle controller for vendor-owned Wi-Fi, BLE, and IP work.
//!
//! The controller does not claim ownership of a vendor scheduler or heap. It
//! records those resources, owns operation identities/deadlines, and requires a
//! cancel/quiesce proof before reuse. A missed cancel deadline escalates to an
//! explicit backend reset and service rebind; lifecycle generation then rejects
//! every completion from the previous vendor instance.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VendorResourceProvenance {
    pub backend_id: &'static str,
    pub worker_tasks: u16,
    pub static_ram_bytes: u32,
    pub max_heap_bytes: Option<u32>,
    pub scheduler_owned_by_vendor: bool,
    pub heap_owned_by_vendor: bool,
}

impl VendorResourceProvenance {
    pub const fn valid(self) -> bool {
        if self.backend_id.is_empty() {
            return false;
        }
        if self.heap_owned_by_vendor {
            matches!(self.max_heap_bytes, Some(bytes) if bytes != 0)
        } else {
            self.max_heap_bytes.is_none()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VendorOperationKind {
    WifiJoin,
    WifiScan,
    BleAdvertise,
    BleGatt,
    Dns,
    SocketConnect,
    Transmit,
    Receive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VendorOperationId {
    pub slot: u16,
    pub generation: u32,
    pub lifecycle_generation: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VendorPoll {
    Pending,
    Complete,
}

pub trait VendorSessionBackend {
    type Error;

    fn provenance(&self) -> VendorResourceProvenance;
    fn begin(
        &mut self,
        operation: VendorOperationId,
        kind: VendorOperationKind,
        deadline_us: u64,
    ) -> Result<(), Self::Error>;
    fn poll(&mut self, operation: VendorOperationId) -> Result<VendorPoll, Self::Error>;
    fn cancel(&mut self, operation: VendorOperationId) -> Result<(), Self::Error>;
    fn quiesced(&mut self, operation: VendorOperationId) -> Result<bool, Self::Error>;
    fn reset(&mut self) -> Result<(), Self::Error>;
    fn rebind(&mut self) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VendorOperationAdmission {
    pub operation: VendorOperationId,
    pub kind: VendorOperationKind,
    pub deadline_us: u64,
    pub cancel_grace_us: u32,
    pub resources: VendorResourceProvenance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VendorOperationOutcome {
    Pending,
    CancelRequested,
    Completed,
    CancelledAndQuiesced,
    ResetAndRebound,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VendorOperationReceipt {
    pub operation: VendorOperationId,
    pub kind: VendorOperationKind,
    pub outcome: VendorOperationOutcome,
    pub observed_at_us: u64,
    pub lifecycle_generation: u32,
    pub resources: VendorResourceProvenance,
}

#[derive(Debug, PartialEq, Eq)]
pub enum VendorSessionError<E> {
    InvalidProvenance,
    InvalidDeadline,
    Full,
    IdentityExhausted,
    LifecycleExhausted,
    ResetRequired,
    RebindRequired,
    StaleOperation,
    Backend(E),
}

pub struct VendorSessionMountError<B: VendorSessionBackend> {
    backend: B,
    error: VendorSessionError<B::Error>,
}

impl<B> core::fmt::Debug for VendorSessionMountError<B>
where
    B: VendorSessionBackend,
    B::Error: core::fmt::Debug,
{
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("VendorSessionMountError")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl<B: VendorSessionBackend> VendorSessionMountError<B> {
    pub const fn error(&self) -> &VendorSessionError<B::Error> {
        &self.error
    }

    pub fn into_backend(self) -> B {
        self.backend
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OperationState {
    Vacant,
    Active,
    Cancelling { cancel_deadline_us: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecoveryState {
    Ready,
    ResetRequired,
    RebindRequired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OperationSlot {
    generation: u32,
    kind: VendorOperationKind,
    deadline_us: u64,
    cancel_grace_us: u32,
    state: OperationState,
}

impl OperationSlot {
    const EMPTY: Self = Self {
        generation: 0,
        kind: VendorOperationKind::WifiJoin,
        deadline_us: 0,
        cancel_grace_us: 0,
        state: OperationState::Vacant,
    };
}

pub struct VendorSessionController<B: VendorSessionBackend, const N: usize> {
    backend: B,
    resources: VendorResourceProvenance,
    lifecycle_generation: u32,
    recovery_state: RecoveryState,
    slots: [OperationSlot; N],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VendorRebindReceipt {
    pub lifecycle_generation: u32,
    pub rebound_at_us: u64,
    pub resources: VendorResourceProvenance,
}

impl<B: VendorSessionBackend, const N: usize> VendorSessionController<B, N> {
    pub fn mount(backend: B) -> Result<Self, VendorSessionMountError<B>> {
        let resources = backend.provenance();
        if !resources.valid() {
            return Err(VendorSessionMountError {
                backend,
                error: VendorSessionError::InvalidProvenance,
            });
        }
        Ok(Self {
            backend,
            resources,
            lifecycle_generation: 1,
            recovery_state: RecoveryState::Ready,
            slots: [OperationSlot::EMPTY; N],
        })
    }

    pub const fn resources(&self) -> VendorResourceProvenance {
        self.resources
    }

    pub const fn lifecycle_generation(&self) -> u32 {
        self.lifecycle_generation
    }

    pub const fn rebind_required(&self) -> bool {
        matches!(self.recovery_state, RecoveryState::RebindRequired)
    }

    pub const fn reset_required(&self) -> bool {
        matches!(self.recovery_state, RecoveryState::ResetRequired)
    }

    pub const fn recovery_required(&self) -> bool {
        !matches!(self.recovery_state, RecoveryState::Ready)
    }

    pub fn begin(
        &mut self,
        kind: VendorOperationKind,
        now_us: u64,
        deadline_us: u64,
        cancel_grace_us: u32,
    ) -> Result<VendorOperationAdmission, VendorSessionError<B::Error>> {
        self.require_ready()?;
        if self.lifecycle_generation == u32::MAX {
            return Err(VendorSessionError::LifecycleExhausted);
        }
        if deadline_us <= now_us || cancel_grace_us == 0 {
            return Err(VendorSessionError::InvalidDeadline);
        }
        let Some((index, slot)) =
            self.slots.iter_mut().enumerate().find(|(_, slot)| {
                slot.state == OperationState::Vacant && slot.generation < u32::MAX
            })
        else {
            return Err(
                if self
                    .slots
                    .iter()
                    .any(|slot| slot.state == OperationState::Vacant)
                {
                    VendorSessionError::IdentityExhausted
                } else {
                    VendorSessionError::Full
                },
            );
        };
        let operation = VendorOperationId {
            slot: u16::try_from(index).map_err(|_| VendorSessionError::Full)?,
            generation: slot.generation + 1,
            lifecycle_generation: self.lifecycle_generation,
        };
        // Burn the identity even when the backend rejects begin: a vendor
        // callback may already have observed it before reporting failure.
        slot.generation = operation.generation;
        slot.kind = kind;
        slot.deadline_us = deadline_us;
        slot.cancel_grace_us = cancel_grace_us;
        slot.state = OperationState::Active;
        if let Err(error) = self.backend.begin(operation, kind, deadline_us) {
            // A fallible vendor begin may have submitted work before reporting
            // failure. Invalidate every callback identity and require an exact
            // reset/rebind instead of silently releasing this slot.
            self.invalidate_for_reset()?;
            return Err(VendorSessionError::Backend(error));
        }
        Ok(VendorOperationAdmission {
            operation,
            kind,
            deadline_us,
            cancel_grace_us,
            resources: self.resources,
        })
    }

    pub fn cancel(
        &mut self,
        operation: VendorOperationId,
        now_us: u64,
    ) -> Result<VendorOperationReceipt, VendorSessionError<B::Error>> {
        self.require_ready()?;
        let index = self.validate(operation)?;
        if self.slots[index].state == OperationState::Active {
            self.slots[index].state = OperationState::Cancelling {
                cancel_deadline_us: now_us
                    .saturating_add(u64::from(self.slots[index].cancel_grace_us)),
            };
            self.backend
                .cancel(operation)
                .map_err(VendorSessionError::Backend)?;
        }
        Ok(self.receipt(
            operation,
            index,
            VendorOperationOutcome::CancelRequested,
            now_us,
        ))
    }

    pub fn service(
        &mut self,
        operation: VendorOperationId,
        now_us: u64,
    ) -> Result<VendorOperationReceipt, VendorSessionError<B::Error>> {
        self.require_ready()?;
        let index = self.validate(operation)?;
        let state = self.slots[index].state;
        if state == OperationState::Active {
            match self
                .backend
                .poll(operation)
                .map_err(VendorSessionError::Backend)?
            {
                VendorPoll::Complete => {
                    let receipt =
                        self.receipt(operation, index, VendorOperationOutcome::Completed, now_us);
                    self.slots[index].state = OperationState::Vacant;
                    return Ok(receipt);
                }
                VendorPoll::Pending if now_us <= self.slots[index].deadline_us => {
                    return Ok(self.receipt(
                        operation,
                        index,
                        VendorOperationOutcome::Pending,
                        now_us,
                    ));
                }
                VendorPoll::Pending => return self.cancel(operation, now_us),
            }
        }

        let OperationState::Cancelling { cancel_deadline_us } = self.slots[index].state else {
            return Err(VendorSessionError::StaleOperation);
        };
        if self
            .backend
            .quiesced(operation)
            .map_err(VendorSessionError::Backend)?
        {
            let receipt = self.receipt(
                operation,
                index,
                VendorOperationOutcome::CancelledAndQuiesced,
                now_us,
            );
            self.slots[index].state = OperationState::Vacant;
            return Ok(receipt);
        }
        if now_us <= cancel_deadline_us {
            return Ok(self.receipt(
                operation,
                index,
                VendorOperationOutcome::CancelRequested,
                now_us,
            ));
        }

        let kind = self.slots[index].kind;
        // A reset may change hardware state even when its vendor API reports
        // failure. Burn the lifecycle and block every operation first.
        self.invalidate_for_reset()?;
        self.recover_backend(now_us)?;
        Ok(VendorOperationReceipt {
            operation,
            kind,
            outcome: VendorOperationOutcome::ResetAndRebound,
            observed_at_us: now_us,
            lifecycle_generation: self.lifecycle_generation,
            resources: self.resources,
        })
    }

    /// Retry service rebind after a reset succeeded but rebind (or its updated
    /// resource provenance) failed. Operations remain unavailable until this
    /// succeeds, and all pre-reset operation identities stay stale.
    pub fn rebind_backend(
        &mut self,
        now_us: u64,
    ) -> Result<VendorRebindReceipt, VendorSessionError<B::Error>> {
        if self.recovery_state == RecoveryState::Ready {
            return Ok(VendorRebindReceipt {
                lifecycle_generation: self.lifecycle_generation,
                rebound_at_us: now_us,
                resources: self.resources,
            });
        }
        if self.recovery_state == RecoveryState::ResetRequired {
            return Err(VendorSessionError::ResetRequired);
        }
        self.backend.rebind().map_err(VendorSessionError::Backend)?;
        let resources = self.backend.provenance();
        if !resources.valid() {
            return Err(VendorSessionError::InvalidProvenance);
        }
        self.resources = resources;
        self.recovery_state = RecoveryState::Ready;
        Ok(VendorRebindReceipt {
            lifecycle_generation: self.lifecycle_generation,
            rebound_at_us: now_us,
            resources,
        })
    }

    /// Complete a blocked recovery in order. A failed reset remains
    /// `ResetRequired`; a failed rebind remains `RebindRequired`. Neither state
    /// admits new work or accepts a pre-recovery completion.
    pub fn recover_backend(
        &mut self,
        now_us: u64,
    ) -> Result<VendorRebindReceipt, VendorSessionError<B::Error>> {
        if self.recovery_state == RecoveryState::ResetRequired {
            self.backend.reset().map_err(VendorSessionError::Backend)?;
            self.recovery_state = RecoveryState::RebindRequired;
        }
        self.rebind_backend(now_us)
    }

    fn require_ready(&self) -> Result<(), VendorSessionError<B::Error>> {
        match self.recovery_state {
            RecoveryState::Ready => Ok(()),
            RecoveryState::ResetRequired => Err(VendorSessionError::ResetRequired),
            RecoveryState::RebindRequired => Err(VendorSessionError::RebindRequired),
        }
    }

    fn invalidate_for_reset(&mut self) -> Result<(), VendorSessionError<B::Error>> {
        self.lifecycle_generation = self
            .lifecycle_generation
            .checked_add(1)
            .ok_or(VendorSessionError::LifecycleExhausted)?;
        for slot in &mut self.slots {
            slot.state = OperationState::Vacant;
        }
        self.recovery_state = RecoveryState::ResetRequired;
        Ok(())
    }

    fn validate(
        &self,
        operation: VendorOperationId,
    ) -> Result<usize, VendorSessionError<B::Error>> {
        let index = usize::from(operation.slot);
        let Some(slot) = self.slots.get(index) else {
            return Err(VendorSessionError::StaleOperation);
        };
        if operation.lifecycle_generation != self.lifecycle_generation
            || operation.generation != slot.generation
            || slot.state == OperationState::Vacant
        {
            return Err(VendorSessionError::StaleOperation);
        }
        Ok(index)
    }

    fn receipt(
        &self,
        operation: VendorOperationId,
        index: usize,
        outcome: VendorOperationOutcome,
        observed_at_us: u64,
    ) -> VendorOperationReceipt {
        VendorOperationReceipt {
            operation,
            kind: self.slots[index].kind,
            outcome,
            observed_at_us,
            lifecycle_generation: self.lifecycle_generation,
            resources: self.resources,
        }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn into_backend(self) -> B {
        self.backend
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Backend {
        complete: Option<VendorOperationId>,
        quiesced: bool,
        cancels: u8,
        resets: u8,
        rebinds: u8,
        fail_cancel: bool,
        fail_begin: bool,
        fail_reset: bool,
        fail_rebind: bool,
        valid_provenance: bool,
    }

    impl Backend {
        fn new() -> Self {
            Self {
                complete: None,
                quiesced: false,
                cancels: 0,
                resets: 0,
                rebinds: 0,
                fail_cancel: false,
                fail_begin: false,
                fail_reset: false,
                fail_rebind: false,
                valid_provenance: true,
            }
        }
    }

    impl VendorSessionBackend for Backend {
        type Error = u8;

        fn provenance(&self) -> VendorResourceProvenance {
            VendorResourceProvenance {
                backend_id: if self.valid_provenance {
                    "test-vendor"
                } else {
                    ""
                },
                worker_tasks: 2,
                static_ram_bytes: 4096,
                max_heap_bytes: Some(8192),
                scheduler_owned_by_vendor: true,
                heap_owned_by_vendor: true,
            }
        }

        fn begin(
            &mut self,
            _: VendorOperationId,
            _: VendorOperationKind,
            _: u64,
        ) -> Result<(), Self::Error> {
            if self.fail_begin {
                Err(6)
            } else {
                Ok(())
            }
        }

        fn poll(&mut self, operation: VendorOperationId) -> Result<VendorPoll, Self::Error> {
            Ok(if self.complete == Some(operation) {
                VendorPoll::Complete
            } else {
                VendorPoll::Pending
            })
        }

        fn cancel(&mut self, _: VendorOperationId) -> Result<(), Self::Error> {
            if self.fail_cancel {
                Err(7)
            } else {
                self.cancels += 1;
                Ok(())
            }
        }

        fn quiesced(&mut self, _: VendorOperationId) -> Result<bool, Self::Error> {
            Ok(self.quiesced)
        }

        fn reset(&mut self) -> Result<(), Self::Error> {
            self.resets += 1;
            if self.fail_reset {
                Err(9)
            } else {
                Ok(())
            }
        }

        fn rebind(&mut self) -> Result<(), Self::Error> {
            self.rebinds += 1;
            if self.fail_rebind {
                Err(8)
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn completion_and_timeout_cancel_are_bounded_and_attributed() {
        let mut sessions = VendorSessionController::<_, 2>::mount(Backend::new()).unwrap();
        let first = sessions
            .begin(VendorOperationKind::WifiJoin, 10, 100, 20)
            .unwrap();
        assert_eq!(
            sessions.service(first.operation, 50).unwrap().outcome,
            VendorOperationOutcome::Pending
        );
        sessions.backend_mut().complete = Some(first.operation);
        assert_eq!(
            sessions.service(first.operation, 60).unwrap().outcome,
            VendorOperationOutcome::Completed
        );
        assert_eq!(
            sessions.service(first.operation, 61),
            Err(VendorSessionError::StaleOperation)
        );

        let second = sessions
            .begin(VendorOperationKind::BleAdvertise, 100, 200, 20)
            .unwrap();
        assert_eq!(
            sessions.service(second.operation, 201).unwrap().outcome,
            VendorOperationOutcome::CancelRequested
        );
        assert_eq!(sessions.backend().cancels, 1);
        sessions.backend_mut().quiesced = true;
        assert_eq!(
            sessions.service(second.operation, 202).unwrap().outcome,
            VendorOperationOutcome::CancelledAndQuiesced
        );
    }

    #[test]
    fn failed_cancel_retains_identity_and_reset_rebind_rejects_stale_completions() {
        let mut sessions = VendorSessionController::<_, 2>::mount(Backend::new()).unwrap();
        let first = sessions
            .begin(VendorOperationKind::SocketConnect, 0, 10, 5)
            .unwrap();
        sessions.backend_mut().fail_cancel = true;
        assert_eq!(
            sessions.service(first.operation, 11),
            Err(VendorSessionError::Backend(7))
        );
        sessions.backend_mut().fail_cancel = false;
        sessions.service(first.operation, 12).unwrap();
        let reset = sessions.service(first.operation, 18).unwrap();
        assert_eq!(reset.outcome, VendorOperationOutcome::ResetAndRebound);
        assert_eq!(reset.lifecycle_generation, 2);
        assert_eq!(
            (sessions.backend().resets, sessions.backend().rebinds),
            (1, 1)
        );
        sessions.backend_mut().complete = Some(first.operation);
        assert_eq!(
            sessions.service(first.operation, 19),
            Err(VendorSessionError::StaleOperation)
        );
    }

    #[test]
    fn invalid_resource_provenance_and_identity_exhaustion_fail_closed() {
        let mut invalid = Backend::new();
        invalid.valid_provenance = false;
        let mount_error = match VendorSessionController::<_, 1>::mount(invalid) {
            Ok(_) => panic!("invalid provenance mounted"),
            Err(error) => error,
        };
        assert_eq!(mount_error.error(), &VendorSessionError::InvalidProvenance);
        assert!(!mount_error.into_backend().valid_provenance);

        let mut backend = Backend::new();
        backend.fail_cancel = false;
        let mut sessions = VendorSessionController::<_, 1>::mount(backend).unwrap();
        sessions.slots[0].generation = u32::MAX;
        assert_eq!(
            sessions.begin(VendorOperationKind::Receive, 0, 1, 1),
            Err(VendorSessionError::IdentityExhausted)
        );
    }

    #[test]
    fn irreversible_reset_invalidates_all_operations_until_rebind_succeeds() {
        let mut sessions = VendorSessionController::<_, 2>::mount(Backend::new()).unwrap();
        let expired = sessions
            .begin(VendorOperationKind::SocketConnect, 0, 10, 5)
            .unwrap();
        let peer = sessions
            .begin(VendorOperationKind::Receive, 0, 100, 5)
            .unwrap();
        sessions.service(expired.operation, 11).unwrap();
        sessions.backend_mut().fail_rebind = true;
        assert_eq!(
            sessions.service(expired.operation, 17),
            Err(VendorSessionError::Backend(8))
        );
        assert!(sessions.rebind_required());
        assert_eq!(sessions.lifecycle_generation(), 2);
        assert_eq!(
            sessions.service(peer.operation, 18),
            Err(VendorSessionError::RebindRequired)
        );
        assert_eq!(
            sessions.begin(VendorOperationKind::WifiScan, 18, 30, 5),
            Err(VendorSessionError::RebindRequired)
        );
        sessions.backend_mut().fail_rebind = false;
        let receipt = sessions.rebind_backend(19).unwrap();
        assert_eq!(receipt.lifecycle_generation, 2);
        assert_eq!(receipt.rebound_at_us, 19);
        assert!(!sessions.rebind_required());
        assert_eq!(
            sessions.service(peer.operation, 19),
            Err(VendorSessionError::StaleOperation)
        );
        sessions
            .begin(VendorOperationKind::WifiScan, 19, 30, 5)
            .unwrap();
    }

    #[test]
    fn fallible_begin_and_reset_block_all_work_until_ordered_recovery() {
        let mut backend = Backend::new();
        backend.fail_begin = true;
        let mut sessions = VendorSessionController::<_, 2>::mount(backend).unwrap();
        assert_eq!(
            sessions.begin(VendorOperationKind::WifiScan, 0, 10, 2),
            Err(VendorSessionError::Backend(6))
        );
        assert!(sessions.reset_required());
        assert!(sessions.recovery_required());
        assert_eq!(sessions.lifecycle_generation(), 2);
        assert_eq!(
            sessions.begin(VendorOperationKind::Receive, 1, 10, 2),
            Err(VendorSessionError::ResetRequired)
        );
        assert_eq!(
            sessions.rebind_backend(1),
            Err(VendorSessionError::ResetRequired)
        );

        sessions.backend_mut().fail_begin = false;
        sessions.backend_mut().fail_reset = true;
        assert_eq!(
            sessions.recover_backend(2),
            Err(VendorSessionError::Backend(9))
        );
        assert!(sessions.reset_required());
        assert_eq!(sessions.backend().resets, 1);

        sessions.backend_mut().fail_reset = false;
        let receipt = sessions.recover_backend(3).unwrap();
        assert_eq!(receipt.lifecycle_generation, 2);
        assert_eq!(receipt.rebound_at_us, 3);
        assert_eq!(
            (sessions.backend().resets, sessions.backend().rebinds),
            (2, 1)
        );
        assert!(!sessions.recovery_required());
        sessions
            .begin(VendorOperationKind::WifiScan, 3, 10, 2)
            .unwrap();
    }
}
