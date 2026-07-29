//! Portable adapter conformance application.
//!
//! Every promoted board can instantiate [`exercise`] with its selected native,
//! `embedded-hal`, Arduino, or C backend. The same source checks identity,
//! deadlines, partial progress, cancellation, cleanup, fresh-generation recovery,
//! and diagnostics without requiring a heap.

#![cfg_attr(not(test), no_std)]

use nobro_device::{
    MountedAdapter, PortableAdapterBackend, PortableAdapterDiagnostics, PortableAdapterError,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConformanceReceipt {
    pub stable_backend_id: u32,
    pub initial_generation: u32,
    pub recovered_generation: u32,
    pub cancelled_units: u32,
    pub diagnostics: PortableAdapterDiagnostics,
}

pub fn exercise<B: PortableAdapterBackend>(
    backend: B,
    now_us: u64,
    deadline_us: u64,
) -> Result<ConformanceReceipt, PortableAdapterError<B::Error>> {
    let mut adapter = MountedAdapter::mount(backend)?;
    let contract = adapter.contract();
    let initial_generation = adapter.session().generation();
    let operation = adapter.begin_operation(now_us, deadline_us, 4)?;
    adapter.advance_operation(operation, 2, now_us.saturating_add(1))?;
    let cancelled_units = adapter.cancel_operation(operation)?.completed_units;
    let recovery = adapter.recover()?;
    let diagnostics = adapter.diagnostics();
    Ok(ConformanceReceipt {
        stable_backend_id: contract.stable_id,
        initial_generation,
        recovered_generation: recovery.session.generation(),
        cancelled_units,
        diagnostics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nobro_device::{
        AdapterBackendKind, AdapterCapabilitySet, PortableAdapterContract, ProviderOwnedResources,
    };

    const OWNED: ProviderOwnedResources = ProviderOwnedResources {
        leases: 1,
        callbacks: 1,
        application_static_objects: 1,
    };

    struct Backend<const KIND: u8>;

    impl<const KIND: u8> PortableAdapterBackend for Backend<KIND> {
        type Error = ();

        const CONTRACT: PortableAdapterContract = PortableAdapterContract::new(
            0xC000 + KIND as u32,
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
            Ok(OWNED)
        }
        fn diagnostic_word(&self) -> u32 {
            KIND as u32
        }
    }

    fn check<const KIND: u8>() {
        let receipt = exercise(Backend::<KIND>, 10, 20).unwrap();
        assert_eq!(receipt.cancelled_units, 2);
        assert!(receipt.recovered_generation > receipt.initial_generation);
        assert_eq!(receipt.diagnostics.backend_word, KIND as u32);
    }

    #[test]
    fn native_embedded_hal_arduino_and_c_use_identical_conformance() {
        check::<1>();
        check::<2>();
        check::<3>();
        check::<4>();
    }
}
