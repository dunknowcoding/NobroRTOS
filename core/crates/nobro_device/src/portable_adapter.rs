//! One bounded lifecycle wrapper for native, `embedded-hal`, Arduino, and C backends.
//!
//! Backend-specific code owns hardware initialization and cleanup. This module owns
//! the common generation, deadline, cancellation, recovery, capability, and
//! diagnostic rules so each binding does not invent a subtly different lifecycle.

use crate::{
    ProviderCancellationReceipt, ProviderLifecycle, ProviderLifecycleError, ProviderLifecycleState,
    ProviderOperation, ProviderOwnedResources, ProviderProgress, ProviderRecoveryReceipt,
    ProviderReleaseReceipt, ProviderSession,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AdapterBackendKind {
    Native = 1,
    EmbeddedHal = 2,
    Arduino = 3,
    C = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AdapterCapability {
    Mount = 0,
    Quiesce = 1,
    Recover = 2,
    Diagnostics = 3,
    Deadline = 4,
    PartialProgress = 5,
    Cancellation = 6,
}

impl AdapterCapability {
    const fn bit(self) -> u16 {
        1_u16 << self as u8
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AdapterCapabilitySet(u16);

impl AdapterCapabilitySet {
    pub const EMPTY: Self = Self(0);
    pub const LIFECYCLE: Self = Self::EMPTY
        .with(AdapterCapability::Mount)
        .with(AdapterCapability::Quiesce)
        .with(AdapterCapability::Recover)
        .with(AdapterCapability::Diagnostics);
    pub const BOUNDED_OPERATIONS: Self = Self::LIFECYCLE
        .with(AdapterCapability::Deadline)
        .with(AdapterCapability::PartialProgress)
        .with(AdapterCapability::Cancellation);

    pub const fn with(self, capability: AdapterCapability) -> Self {
        Self(self.0 | capability.bit())
    }

    pub const fn contains(self, capability: AdapterCapability) -> bool {
        self.0 & capability.bit() != 0
    }

    pub const fn contains_all(self, required: Self) -> bool {
        self.0 & required.0 == required.0
    }

    pub const fn bits(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortableAdapterContract {
    /// Stable catalog identity. Zero is reserved for an invalid/unregistered backend.
    pub stable_id: u32,
    pub kind: AdapterBackendKind,
    pub capabilities: AdapterCapabilitySet,
}

impl PortableAdapterContract {
    pub const fn new(
        stable_id: u32,
        kind: AdapterBackendKind,
        capabilities: AdapterCapabilitySet,
    ) -> Self {
        Self {
            stable_id,
            kind,
            capabilities,
        }
    }

    pub const fn valid(self) -> bool {
        self.stable_id != 0
            && self
                .capabilities
                .contains_all(AdapterCapabilitySet::LIFECYCLE)
    }
}

/// Backend hooks. Exactly one value implementing this trait is owned by each
/// [`MountedAdapter`] instance.
pub trait PortableAdapterBackend {
    type Error;

    const CONTRACT: PortableAdapterContract;

    fn mount(&mut self) -> Result<ProviderOwnedResources, Self::Error>;
    fn quiesce(&mut self) -> Result<(), Self::Error>;
    /// Disable callbacks/DMA, revoke leases, and report exactly what was cleaned.
    fn release(&mut self) -> Result<ProviderOwnedResources, Self::Error>;
    /// Reinitialize the same logical backend and report its newly owned resources.
    fn recover(&mut self) -> Result<ProviderOwnedResources, Self::Error>;
    /// Fixed-width backend-specific diagnostic word.
    fn diagnostic_word(&self) -> u32;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PortableAdapterDiagnostics {
    pub mounts: u32,
    pub quiesces: u32,
    pub recoveries: u32,
    pub backend_failures: u32,
    pub rejected_capabilities: u32,
    pub backend_word: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PortableAdapterError<E> {
    InvalidContract,
    Unavailable(AdapterCapability),
    Backend(E),
    Lifecycle(ProviderLifecycleError),
}

impl<E> From<ProviderLifecycleError> for PortableAdapterError<E> {
    fn from(error: ProviderLifecycleError) -> Self {
        Self::Lifecycle(error)
    }
}

pub struct MountedAdapter<B: PortableAdapterBackend> {
    backend: B,
    lifecycle: ProviderLifecycle,
    session: ProviderSession,
    pending_release: Option<ProviderReleaseReceipt>,
    diagnostics: PortableAdapterDiagnostics,
}

impl<B: PortableAdapterBackend> MountedAdapter<B> {
    pub fn mount(mut backend: B) -> Result<Self, PortableAdapterError<B::Error>> {
        if !B::CONTRACT.valid() {
            return Err(PortableAdapterError::InvalidContract);
        }
        let owned = backend.mount().map_err(PortableAdapterError::Backend)?;
        let mut lifecycle = ProviderLifecycle::new();
        let session = lifecycle.mount(owned)?;
        let backend_word = backend.diagnostic_word();
        Ok(Self {
            backend,
            lifecycle,
            session,
            pending_release: None,
            diagnostics: PortableAdapterDiagnostics {
                mounts: 1,
                backend_word,
                ..PortableAdapterDiagnostics::default()
            },
        })
    }

    pub const fn contract(&self) -> PortableAdapterContract {
        B::CONTRACT
    }

    pub const fn session(&self) -> ProviderSession {
        self.session
    }

    pub const fn state(&self) -> ProviderLifecycleState {
        self.lifecycle.state()
    }

    pub fn require(
        &mut self,
        capability: AdapterCapability,
    ) -> Result<(), PortableAdapterError<B::Error>> {
        if B::CONTRACT.capabilities.contains(capability) {
            Ok(())
        } else {
            self.diagnostics.rejected_capabilities =
                self.diagnostics.rejected_capabilities.saturating_add(1);
            Err(PortableAdapterError::Unavailable(capability))
        }
    }

    pub fn begin_operation(
        &mut self,
        now_us: u64,
        deadline_us: u64,
        total_units: u32,
    ) -> Result<ProviderOperation, PortableAdapterError<B::Error>> {
        self.require(AdapterCapability::Deadline)?;
        self.require(AdapterCapability::PartialProgress)?;
        self.lifecycle
            .begin_operation(self.session, now_us, deadline_us, total_units)
            .map_err(Into::into)
    }

    pub fn advance_operation(
        &mut self,
        operation: ProviderOperation,
        completed_units: u32,
        now_us: u64,
    ) -> Result<ProviderProgress, PortableAdapterError<B::Error>> {
        self.require(AdapterCapability::PartialProgress)?;
        self.lifecycle
            .advance_operation(self.session, operation, completed_units, now_us)
            .map_err(Into::into)
    }

    pub fn finish_operation(
        &mut self,
        operation: ProviderOperation,
        now_us: u64,
    ) -> Result<ProviderProgress, PortableAdapterError<B::Error>> {
        self.require(AdapterCapability::PartialProgress)?;
        self.lifecycle
            .finish_operation(self.session, operation, now_us)
            .map_err(Into::into)
    }

    pub fn cancel_operation(
        &mut self,
        operation: ProviderOperation,
    ) -> Result<ProviderCancellationReceipt, PortableAdapterError<B::Error>> {
        self.require(AdapterCapability::Cancellation)?;
        self.lifecycle
            .cancel_operation(self.session, operation)
            .map_err(Into::into)
    }

    pub fn quiesce(&mut self) -> Result<(), PortableAdapterError<B::Error>> {
        self.require(AdapterCapability::Quiesce)?;
        self.lifecycle.quiesce(self.session)?;
        if let Err(error) = self.backend.quiesce() {
            let _ = self.lifecycle.resume(self.session);
            self.diagnostics.backend_failures = self.diagnostics.backend_failures.saturating_add(1);
            return Err(PortableAdapterError::Backend(error));
        }
        self.diagnostics.quiesces = self.diagnostics.quiesces.saturating_add(1);
        Ok(())
    }

    /// Quiesce, prove cleanup, and remount the same logical backend with a fresh
    /// generation. Old sessions therefore fail closed after recovery.
    pub fn recover(&mut self) -> Result<ProviderRecoveryReceipt, PortableAdapterError<B::Error>> {
        self.require(AdapterCapability::Recover)?;
        if self.lifecycle.state() == ProviderLifecycleState::Ready {
            self.quiesce()?;
        }
        let release = if self.lifecycle.state() == ProviderLifecycleState::Quiesced {
            let cleaned = self.backend.release().map_err(|error| {
                self.diagnostics.backend_failures =
                    self.diagnostics.backend_failures.saturating_add(1);
                PortableAdapterError::Backend(error)
            })?;
            let receipt = self.lifecycle.release(self.session, cleaned)?;
            self.pending_release = Some(receipt);
            receipt
        } else {
            self.pending_release.ok_or(PortableAdapterError::Lifecycle(
                ProviderLifecycleError::NotMounted,
            ))?
        };
        let remounted = self.backend.recover().map_err(|error| {
            self.diagnostics.backend_failures = self.diagnostics.backend_failures.saturating_add(1);
            PortableAdapterError::Backend(error)
        })?;
        let session = self.lifecycle.mount(remounted)?;
        self.session = session;
        self.pending_release = None;
        self.diagnostics.mounts = self.diagnostics.mounts.saturating_add(1);
        self.diagnostics.recoveries = self.diagnostics.recoveries.saturating_add(1);
        self.diagnostics.backend_word = self.backend.diagnostic_word();
        Ok(ProviderRecoveryReceipt { release, session })
    }

    pub fn diagnostics(&mut self) -> PortableAdapterDiagnostics {
        self.diagnostics.backend_word = self.backend.diagnostic_word();
        self.diagnostics
    }

    pub const fn backend(&self) -> &B {
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

    const OWNED: ProviderOwnedResources = ProviderOwnedResources {
        leases: 1,
        callbacks: 1,
        application_static_objects: 1,
    };

    struct Fake<const KIND: u8> {
        word: u32,
    }

    impl<const KIND: u8> PortableAdapterBackend for Fake<KIND> {
        type Error = ();

        const CONTRACT: PortableAdapterContract = PortableAdapterContract::new(
            0xA000 + KIND as u32,
            match KIND {
                1 => AdapterBackendKind::Native,
                2 => AdapterBackendKind::EmbeddedHal,
                3 => AdapterBackendKind::Arduino,
                _ => AdapterBackendKind::C,
            },
            AdapterCapabilitySet::BOUNDED_OPERATIONS,
        );

        fn mount(&mut self) -> Result<ProviderOwnedResources, Self::Error> {
            Ok(OWNED)
        }
        fn quiesce(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
        fn release(&mut self) -> Result<ProviderOwnedResources, Self::Error> {
            Ok(OWNED)
        }
        fn recover(&mut self) -> Result<ProviderOwnedResources, Self::Error> {
            self.word = self.word.saturating_add(1);
            Ok(OWNED)
        }
        fn diagnostic_word(&self) -> u32 {
            self.word
        }
    }

    fn conformance<const KIND: u8>() {
        let mut adapter = MountedAdapter::mount(Fake::<KIND> { word: 7 }).unwrap();
        let stale = adapter.session();
        let operation = adapter.begin_operation(10, 20, 4).unwrap();
        assert_eq!(
            adapter
                .advance_operation(operation, 2, 15)
                .unwrap()
                .completed_units,
            2
        );
        assert_eq!(
            adapter.cancel_operation(operation).unwrap().completed_units,
            2
        );
        let recovered = adapter.recover().unwrap();
        assert_ne!(stale.generation(), recovered.session.generation());
        assert_eq!(adapter.diagnostics().backend_word, 8);
    }

    #[test]
    fn every_backend_class_obeys_the_same_non_vacuous_contract() {
        conformance::<1>();
        conformance::<2>();
        conformance::<3>();
        conformance::<4>();
    }

    #[test]
    fn unavailable_capability_is_rejected_before_backend_io() {
        struct Limited;
        impl PortableAdapterBackend for Limited {
            type Error = ();
            const CONTRACT: PortableAdapterContract = PortableAdapterContract::new(
                0xB001,
                AdapterBackendKind::Native,
                AdapterCapabilitySet::LIFECYCLE,
            );
            fn mount(&mut self) -> Result<ProviderOwnedResources, Self::Error> {
                Ok(OWNED)
            }
            fn quiesce(&mut self) -> Result<(), Self::Error> {
                Ok(())
            }
            fn release(&mut self) -> Result<ProviderOwnedResources, Self::Error> {
                Ok(OWNED)
            }
            fn recover(&mut self) -> Result<ProviderOwnedResources, Self::Error> {
                Ok(OWNED)
            }
            fn diagnostic_word(&self) -> u32 {
                0
            }
        }

        let mut adapter = MountedAdapter::mount(Limited).unwrap();
        assert_eq!(
            adapter.begin_operation(1, 2, 1),
            Err(PortableAdapterError::Unavailable(
                AdapterCapability::Deadline
            ))
        );
        assert_eq!(adapter.diagnostics().rejected_capabilities, 1);
    }

    #[test]
    fn transient_backend_remount_failure_can_be_retried_after_release() {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        enum Error {
            Transient,
        }

        struct RetryBackend {
            fail_once: bool,
        }

        impl PortableAdapterBackend for RetryBackend {
            type Error = Error;
            const CONTRACT: PortableAdapterContract = PortableAdapterContract::new(
                0xB002,
                AdapterBackendKind::EmbeddedHal,
                AdapterCapabilitySet::BOUNDED_OPERATIONS,
            );

            fn mount(&mut self) -> Result<ProviderOwnedResources, Self::Error> {
                Ok(OWNED)
            }
            fn quiesce(&mut self) -> Result<(), Self::Error> {
                Ok(())
            }
            fn release(&mut self) -> Result<ProviderOwnedResources, Self::Error> {
                Ok(OWNED)
            }
            fn recover(&mut self) -> Result<ProviderOwnedResources, Self::Error> {
                if core::mem::take(&mut self.fail_once) {
                    Err(Error::Transient)
                } else {
                    Ok(OWNED)
                }
            }
            fn diagnostic_word(&self) -> u32 {
                0
            }
        }

        let mut adapter = MountedAdapter::mount(RetryBackend { fail_once: true }).unwrap();
        let stale = adapter.session();
        assert_eq!(
            adapter.recover(),
            Err(PortableAdapterError::Backend(Error::Transient))
        );
        assert_eq!(adapter.state(), ProviderLifecycleState::Down);
        let receipt = adapter.recover().unwrap();
        assert_ne!(receipt.session.generation(), stale.generation());
        assert_eq!(adapter.state(), ProviderLifecycleState::Ready);
        assert_eq!(adapter.diagnostics().backend_failures, 1);
    }
}
