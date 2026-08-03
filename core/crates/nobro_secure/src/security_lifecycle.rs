//! Bounded signing-key lifecycle, protected-root receipts, fleet-key ownership,
//! and exact update/AEAD scope.

use nobro_crypto::ccm::{self, CcmError};

use crate::{
    verify_signed_measurement_with, AsymmetricImageVerifier, BootVectorPolicy,
    Ed25519ImageVerifier, PersistentBootController, PersistentBootError, PinnedKeyPolicy,
    ProtectedRollbackBackend, SignedBootError, SignedImageManifest, Slot, VerifiedSignedImage,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecurityRootKind {
    SoftwareOnly,
    Otp,
    Efuse,
    TrustZone,
    SecureElement,
    TrustedCompanion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstantTimeScope {
    NotClaimed,
    SoftwareTagCompareOnly,
    HardwareBoundary,
    VendorDeclared,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SecurityRootProfile {
    pub backend_id: &'static str,
    pub generation: u32,
    pub root: SecurityRootKind,
    pub non_exportable_keys: bool,
    pub monotonic_counter: bool,
    pub constant_time_scope: ConstantTimeScope,
}

impl SecurityRootProfile {
    pub const fn valid(self) -> bool {
        if self.backend_id.is_empty() || self.generation == 0 {
            return false;
        }
        match self.root {
            SecurityRootKind::SoftwareOnly => {
                !self.non_exportable_keys
                    && !self.monotonic_counter
                    && matches!(
                        self.constant_time_scope,
                        ConstantTimeScope::NotClaimed | ConstantTimeScope::SoftwareTagCompareOnly
                    )
            }
            _ => true,
        }
    }

    pub const fn hardware_rooted(self) -> bool {
        !matches!(self.root, SecurityRootKind::SoftwareOnly)
            && self.non_exportable_keys
            && self.monotonic_counter
    }
}

/// Protected rollback connector with explicit deployment strength.
///
/// OTP, eFuse, TrustZone, secure-element, and trusted-companion adapters expose
/// their exact profile here. A software implementation remains valid but its
/// receipt is deliberately weaker.
pub trait AttestedRollbackBackend: ProtectedRollbackBackend {
    fn security_profile(&self) -> SecurityRootProfile;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProtectedVerificationReceipt {
    pub rollback_floor: u32,
    pub image_version: u32,
    pub root: SecurityRootProfile,
    pub hardware_rooted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttestedProtectedBootError<E> {
    InvalidProfile,
    Backend(E),
    Verification(SignedBootError),
    Rollback,
}

pub fn verify_signed_measurement_attested<const N: usize, B: AttestedRollbackBackend>(
    image_measurement: [u8; 32],
    manifest: &SignedImageManifest,
    keys: &PinnedKeyPolicy<N>,
    vectors: BootVectorPolicy,
    verifier: &impl AsymmetricImageVerifier,
    rollback: &B,
) -> Result<(VerifiedSignedImage, ProtectedVerificationReceipt), AttestedProtectedBootError<B::Error>>
{
    let root = rollback.security_profile();
    if !root.valid() {
        return Err(AttestedProtectedBootError::InvalidProfile);
    }
    let floor = rollback
        .load_floor()
        .map_err(AttestedProtectedBootError::Backend)?;
    if rollback.security_profile() != root {
        return Err(AttestedProtectedBootError::InvalidProfile);
    }
    let verified =
        verify_signed_measurement_with(image_measurement, manifest, keys, vectors, floor, verifier)
            .map_err(AttestedProtectedBootError::Verification)?;
    Ok((
        verified,
        ProtectedVerificationReceipt {
            rollback_floor: floor,
            image_version: verified.plan().version,
            root,
            hardware_rooted: root.hardware_rooted(),
        },
    ))
}

pub fn commit_attested_rollback_floor<B: AttestedRollbackBackend>(
    verified: &VerifiedSignedImage,
    rollback: &mut B,
) -> Result<ProtectedVerificationReceipt, AttestedProtectedBootError<B::Error>> {
    let root = rollback.security_profile();
    if !root.valid() {
        return Err(AttestedProtectedBootError::InvalidProfile);
    }
    let floor = rollback
        .load_floor()
        .map_err(AttestedProtectedBootError::Backend)?;
    let version = verified.plan().version;
    if version < floor {
        return Err(AttestedProtectedBootError::Rollback);
    }
    rollback
        .commit_floor_if_higher(version)
        .map_err(AttestedProtectedBootError::Backend)?;
    let committed = rollback
        .load_floor()
        .map_err(AttestedProtectedBootError::Backend)?;
    if committed < version || rollback.security_profile() != root {
        return Err(AttestedProtectedBootError::Rollback);
    }
    Ok(ProtectedVerificationReceipt {
        rollback_floor: committed,
        image_version: version,
        root,
        hardware_rooted: root.hardware_rooted(),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SigningKeyStatus {
    Pending,
    Active,
    Revoked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SigningKeyRecord {
    pub id: u32,
    pub public_key: [u8; 32],
    pub epoch: u32,
    pub valid_from_version: u32,
    pub revoked_from_version: Option<u32>,
    pub status: SigningKeyStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SigningKeyRing<const N: usize> {
    generation: u32,
    entries: [Option<SigningKeyRecord>; N],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SigningKeyResolutionError {
    UnknownKey,
    PendingKey,
    RevokedKey,
    VersionBeforeActivation,
}

impl<const N: usize> SigningKeyRing<N> {
    pub const fn empty() -> Self {
        Self {
            generation: 0,
            entries: [None; N],
        }
    }

    pub fn bootstrap(
        id: u32,
        public_key: [u8; 32],
        epoch: u32,
        valid_from_version: u32,
    ) -> Option<Self> {
        if N == 0 || id == 0 || epoch == 0 {
            return None;
        }
        let mut ring = Self::empty();
        ring.generation = 1;
        ring.entries[0] = Some(SigningKeyRecord {
            id,
            public_key,
            epoch,
            valid_from_version,
            revoked_from_version: None,
            status: SigningKeyStatus::Active,
        });
        Some(ring)
    }

    pub const fn generation(&self) -> u32 {
        self.generation
    }

    pub fn record(&self, id: u32) -> Option<SigningKeyRecord> {
        self.entries
            .iter()
            .flatten()
            .find(|key| key.id == id)
            .copied()
    }

    pub fn resolve(&self, id: u32, version: u32) -> Result<&[u8; 32], SigningKeyResolutionError> {
        let record = self
            .entries
            .iter()
            .flatten()
            .find(|record| record.id == id)
            .ok_or(SigningKeyResolutionError::UnknownKey)?;
        match record.status {
            SigningKeyStatus::Pending => Err(SigningKeyResolutionError::PendingKey),
            SigningKeyStatus::Revoked => Err(SigningKeyResolutionError::RevokedKey),
            SigningKeyStatus::Active if version < record.valid_from_version => {
                Err(SigningKeyResolutionError::VersionBeforeActivation)
            }
            SigningKeyStatus::Active => Ok(&record.public_key),
        }
    }

    fn active_index(&self, id: u32) -> Option<usize> {
        self.entries.iter().position(|entry| {
            matches!(entry, Some(record) if record.id == id && record.status == SigningKeyStatus::Active)
        })
    }

    fn pending_index(&self, id: u32) -> Option<usize> {
        self.entries.iter().position(|entry| {
            matches!(entry, Some(record) if record.id == id && record.status == SigningKeyStatus::Pending)
        })
    }
}

impl<const N: usize> Default for SigningKeyRing<N> {
    fn default() -> Self {
        Self::empty()
    }
}

pub trait SigningKeyMetadataStore<const N: usize> {
    type Error;
    fn load(&self) -> Result<SigningKeyRing<N>, Self::Error>;
    fn commit_if_newer(&mut self, ring: SigningKeyRing<N>) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SigningKeyLifecycleError<E> {
    Backend(E),
    InvalidState,
    UnknownKey,
    DuplicateKey,
    Full,
    EpochRollback,
    VersionRollback,
    GenerationExhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SigningKeyLifecycleAction {
    RotationStaged,
    RotationActivated,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SigningKeyLifecycleReceipt {
    pub generation: u32,
    pub old_key_id: u32,
    pub new_key_id: u32,
    pub activation_version: u32,
    pub action: SigningKeyLifecycleAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SigningKeyRevocationReceipt {
    pub generation: u32,
    pub key_id: u32,
    pub revoked_from_version: u32,
}

pub struct SigningKeyController<S, const N: usize> {
    store: S,
}

impl<S, const N: usize> SigningKeyController<S, N>
where
    S: SigningKeyMetadataStore<N>,
{
    pub const fn new(store: S) -> Self {
        Self { store }
    }

    pub fn stage_rotation(
        &mut self,
        old_key_id: u32,
        new_key_id: u32,
        new_public_key: [u8; 32],
        new_epoch: u32,
        activation_version: u32,
    ) -> Result<SigningKeyLifecycleReceipt, SigningKeyLifecycleError<S::Error>> {
        let mut ring = self
            .store
            .load()
            .map_err(SigningKeyLifecycleError::Backend)?;
        let old = ring
            .active_index(old_key_id)
            .and_then(|index| ring.entries[index])
            .ok_or(SigningKeyLifecycleError::UnknownKey)?;
        if ring.record(new_key_id).is_some() {
            return Err(SigningKeyLifecycleError::DuplicateKey);
        }
        if new_epoch <= old.epoch {
            return Err(SigningKeyLifecycleError::EpochRollback);
        }
        if activation_version <= old.valid_from_version {
            return Err(SigningKeyLifecycleError::VersionRollback);
        }
        let index = ring
            .entries
            .iter()
            .position(Option::is_none)
            .ok_or(SigningKeyLifecycleError::Full)?;
        ring.generation = ring
            .generation
            .checked_add(1)
            .ok_or(SigningKeyLifecycleError::GenerationExhausted)?;
        ring.entries[index] = Some(SigningKeyRecord {
            id: new_key_id,
            public_key: new_public_key,
            epoch: new_epoch,
            valid_from_version: activation_version,
            revoked_from_version: None,
            status: SigningKeyStatus::Pending,
        });
        self.store
            .commit_if_newer(ring)
            .map_err(SigningKeyLifecycleError::Backend)?;
        Ok(SigningKeyLifecycleReceipt {
            generation: ring.generation,
            old_key_id,
            new_key_id,
            activation_version,
            action: SigningKeyLifecycleAction::RotationStaged,
        })
    }

    pub fn activate_rotation(
        &mut self,
        old_key_id: u32,
        new_key_id: u32,
        activation_version: u32,
    ) -> Result<SigningKeyLifecycleReceipt, SigningKeyLifecycleError<S::Error>> {
        let mut ring = self
            .store
            .load()
            .map_err(SigningKeyLifecycleError::Backend)?;
        let old_index = ring
            .active_index(old_key_id)
            .ok_or(SigningKeyLifecycleError::UnknownKey)?;
        let new_index = ring
            .pending_index(new_key_id)
            .ok_or(SigningKeyLifecycleError::UnknownKey)?;
        let pending = ring.entries[new_index].ok_or(SigningKeyLifecycleError::InvalidState)?;
        if pending.valid_from_version != activation_version {
            return Err(SigningKeyLifecycleError::VersionRollback);
        }
        ring.generation = ring
            .generation
            .checked_add(1)
            .ok_or(SigningKeyLifecycleError::GenerationExhausted)?;
        if let Some(old) = ring.entries[old_index].as_mut() {
            old.status = SigningKeyStatus::Revoked;
            old.revoked_from_version = Some(activation_version);
        }
        if let Some(new) = ring.entries[new_index].as_mut() {
            new.status = SigningKeyStatus::Active;
        }
        self.store
            .commit_if_newer(ring)
            .map_err(SigningKeyLifecycleError::Backend)?;
        Ok(SigningKeyLifecycleReceipt {
            generation: ring.generation,
            old_key_id,
            new_key_id,
            activation_version,
            action: SigningKeyLifecycleAction::RotationActivated,
        })
    }

    pub fn revoke_key(
        &mut self,
        key_id: u32,
        revoked_from_version: u32,
    ) -> Result<SigningKeyRevocationReceipt, SigningKeyLifecycleError<S::Error>> {
        let mut ring = self
            .store
            .load()
            .map_err(SigningKeyLifecycleError::Backend)?;
        let index = ring
            .entries
            .iter()
            .position(|entry| matches!(entry, Some(record) if record.id == key_id))
            .ok_or(SigningKeyLifecycleError::UnknownKey)?;
        let record = ring.entries[index].ok_or(SigningKeyLifecycleError::InvalidState)?;
        if record.status == SigningKeyStatus::Revoked
            || revoked_from_version < record.valid_from_version
        {
            return Err(SigningKeyLifecycleError::VersionRollback);
        }
        if record.status == SigningKeyStatus::Active
            && ring
                .entries
                .iter()
                .flatten()
                .filter(|candidate| {
                    candidate.status == SigningKeyStatus::Active && candidate.id != key_id
                })
                .count()
                == 0
        {
            // Never turn a recoverable trust store into one with no active root.
            // Rotate first, then revoke the superseded key atomically.
            return Err(SigningKeyLifecycleError::InvalidState);
        }
        ring.generation = ring
            .generation
            .checked_add(1)
            .ok_or(SigningKeyLifecycleError::GenerationExhausted)?;
        if let Some(record) = ring.entries[index].as_mut() {
            record.status = SigningKeyStatus::Revoked;
            record.revoked_from_version = Some(revoked_from_version);
            record.public_key.fill(0);
        }
        self.store
            .commit_if_newer(ring)
            .map_err(SigningKeyLifecycleError::Backend)?;
        Ok(SigningKeyRevocationReceipt {
            generation: ring.generation,
            key_id,
            revoked_from_version,
        })
    }

    pub fn ring(&self) -> Result<SigningKeyRing<N>, SigningKeyLifecycleError<S::Error>> {
        self.store.load().map_err(SigningKeyLifecycleError::Backend)
    }

    pub fn into_store(self) -> S {
        self.store
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleSignedBootError {
    Key(SigningKeyResolutionError),
    Verification(SignedBootError),
}

pub fn verify_signed_measurement_lifecycle<const N: usize>(
    image_measurement: [u8; 32],
    manifest: &SignedImageManifest,
    ring: &SigningKeyRing<N>,
    vectors: BootVectorPolicy,
    rollback_floor: u32,
) -> Result<VerifiedSignedImage, LifecycleSignedBootError> {
    let key = *ring
        .resolve(manifest.key_id, manifest.version)
        .map_err(LifecycleSignedBootError::Key)?;
    let mut pinned = PinnedKeyPolicy::<1>::new();
    if !pinned.pin(manifest.key_id, key) {
        return Err(LifecycleSignedBootError::Key(
            SigningKeyResolutionError::UnknownKey,
        ));
    }
    verify_signed_measurement_with(
        image_measurement,
        manifest,
        &pinned,
        vectors,
        rollback_floor,
        &Ed25519ImageVerifier,
    )
    .map_err(LifecycleSignedBootError::Verification)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FleetKeyPurpose {
    FirmwareAead,
    TelemetryAead,
    Provisioning,
    Attestation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FleetKeyRequest {
    pub fleet_id: u32,
    pub device_id: [u8; 16],
    pub epoch: u32,
    pub purpose: FleetKeyPurpose,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FleetKeyOwnership {
    DeviceRootDerived,
    FleetAuthorityImported,
}

pub trait FleetKeyBackend {
    type Error;
    fn security_profile(&self) -> SecurityRootProfile;
    fn derive(
        &mut self,
        request: FleetKeyRequest,
        output: &mut [u8; 32],
    ) -> Result<(), Self::Error>;
    fn import_wrapped(&mut self, wrapped: &[u8], output: &mut [u8; 32]) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FleetKeyReceipt {
    pub ownership: FleetKeyOwnership,
    pub purpose: FleetKeyPurpose,
    pub epoch: u32,
    pub root: SecurityRootProfile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FleetKeyError<E> {
    InvalidProfile,
    InvalidRequest,
    Backend(E),
}

pub fn derive_fleet_key<B: FleetKeyBackend>(
    backend: &mut B,
    request: FleetKeyRequest,
    output: &mut [u8; 32],
) -> Result<FleetKeyReceipt, FleetKeyError<B::Error>> {
    if request.fleet_id == 0 || request.epoch == 0 {
        return Err(FleetKeyError::InvalidRequest);
    }
    let root = backend.security_profile();
    if !root.valid() {
        return Err(FleetKeyError::InvalidProfile);
    }
    output.fill(0);
    if let Err(error) = backend.derive(request, output) {
        output.fill(0);
        return Err(FleetKeyError::Backend(error));
    }
    if backend.security_profile() != root {
        output.fill(0);
        return Err(FleetKeyError::InvalidProfile);
    }
    Ok(FleetKeyReceipt {
        ownership: FleetKeyOwnership::DeviceRootDerived,
        purpose: request.purpose,
        epoch: request.epoch,
        root,
    })
}

pub fn import_fleet_key<B: FleetKeyBackend>(
    backend: &mut B,
    purpose: FleetKeyPurpose,
    epoch: u32,
    wrapped: &[u8],
    output: &mut [u8; 32],
) -> Result<FleetKeyReceipt, FleetKeyError<B::Error>> {
    if epoch == 0 || wrapped.is_empty() {
        return Err(FleetKeyError::InvalidRequest);
    }
    let root = backend.security_profile();
    if !root.valid() {
        return Err(FleetKeyError::InvalidProfile);
    }
    output.fill(0);
    if let Err(error) = backend.import_wrapped(wrapped, output) {
        output.fill(0);
        return Err(FleetKeyError::Backend(error));
    }
    if backend.security_profile() != root {
        output.fill(0);
        return Err(FleetKeyError::InvalidProfile);
    }
    Ok(FleetKeyReceipt {
        ownership: FleetKeyOwnership::FleetAuthorityImported,
        purpose,
        epoch,
        root,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AeadMemberOperation {
    Seal,
    Open,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AeadMemberReceipt {
    pub backend_id: &'static str,
    pub operation: AeadMemberOperation,
    pub ownership: FleetKeyOwnership,
    pub payload_bytes: u32,
    pub aad_bytes: u32,
    pub tag_bytes: u8,
    pub constant_time_scope: ConstantTimeScope,
}

pub fn seal_aead_member(
    key: &[u8; 16],
    nonce: &[u8; ccm::NONCE_LEN],
    aad: &[u8],
    payload: &[u8],
    output: &mut [u8],
    ownership: FleetKeyOwnership,
) -> Result<(usize, AeadMemberReceipt), CcmError> {
    let len = ccm::encrypt(key, nonce, aad, payload, output)?;
    Ok((
        len,
        AeadMemberReceipt {
            backend_id: "nobro-crypto/aes-ccm-128-8",
            operation: AeadMemberOperation::Seal,
            ownership,
            payload_bytes: payload.len() as u32,
            aad_bytes: aad.len() as u32,
            tag_bytes: ccm::TAG_LEN as u8,
            constant_time_scope: ConstantTimeScope::SoftwareTagCompareOnly,
        },
    ))
}

pub fn open_aead_member(
    key: &[u8; 16],
    nonce: &[u8; ccm::NONCE_LEN],
    aad: &[u8],
    input: &[u8],
    output: &mut [u8],
    ownership: FleetKeyOwnership,
) -> Result<(usize, AeadMemberReceipt), CcmError> {
    let len = ccm::decrypt(key, nonce, aad, input, output)?;
    Ok((
        len,
        AeadMemberReceipt {
            backend_id: "nobro-crypto/aes-ccm-128-8",
            operation: AeadMemberOperation::Open,
            ownership,
            payload_bytes: len as u32,
            aad_bytes: aad.len() as u32,
            tag_bytes: ccm::TAG_LEN as u8,
            constant_time_scope: ConstantTimeScope::SoftwareTagCompareOnly,
        },
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootSlotLayout {
    pub slot_count: u8,
    pub slot_bytes: u32,
    pub rollback_history_slots: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdatePayloadKind {
    FullImage,
    Delta { base_version: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopedBootStageReceipt {
    pub slot: Slot,
    pub version: u32,
    pub image_bytes: u32,
    pub payload: UpdatePayloadKind,
    pub rollback_history_slots: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopedBootStageError {
    UnsupportedLayout,
    ImageTooLarge,
    DeltaUnsupported,
    Boot(PersistentBootError),
}

pub fn stage_scoped_full_image<S: crate::MonotonicBootStore>(
    controller: &mut PersistentBootController<S>,
    slot: Slot,
    plan: crate::VerifiedBootPlan,
    layout: BootSlotLayout,
    payload: UpdatePayloadKind,
) -> Result<ScopedBootStageReceipt, ScopedBootStageError> {
    // PersistentBootState carries one active and one pending A/B slot, so it can
    // prove exactly one rollback history entry. Larger histories need a new
    // persisted schema rather than an optimistic counter.
    if layout.slot_count != 2 || layout.rollback_history_slots != 1 || layout.slot_bytes == 0 {
        return Err(ScopedBootStageError::UnsupportedLayout);
    }
    if plan.image_len > layout.slot_bytes {
        return Err(ScopedBootStageError::ImageTooLarge);
    }
    if !matches!(payload, UpdatePayloadKind::FullImage) {
        return Err(ScopedBootStageError::DeltaUnsupported);
    }
    controller
        .stage(slot, plan)
        .map_err(ScopedBootStageError::Boot)?;
    Ok(ScopedBootStageReceipt {
        slot,
        version: plan.version,
        image_bytes: plan.image_len,
        payload,
        rollback_history_slots: 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MonotonicBootStore, PersistentBootState, VerifiedBootPlan};
    use ed25519_dalek::{Signer, SigningKey};
    use nobro_crypto::sha256::{hmac_sha256, sha256};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum TestError {
        Offline,
        Corrupt,
    }

    #[derive(Clone, Copy)]
    struct KeyStore<const N: usize> {
        ring: SigningKeyRing<N>,
        fail_commit: bool,
    }

    impl<const N: usize> SigningKeyMetadataStore<N> for KeyStore<N> {
        type Error = TestError;

        fn load(&self) -> Result<SigningKeyRing<N>, Self::Error> {
            Ok(self.ring)
        }

        fn commit_if_newer(&mut self, ring: SigningKeyRing<N>) -> Result<(), Self::Error> {
            if self.fail_commit {
                return Err(TestError::Offline);
            }
            if ring.generation <= self.ring.generation {
                return Err(TestError::Corrupt);
            }
            self.ring = ring;
            Ok(())
        }
    }

    #[test]
    fn interrupted_rotation_keeps_old_active_and_activation_revokes_it_atomically() {
        let old = SigningKey::from_bytes(&[1; 32]);
        let new = SigningKey::from_bytes(&[2; 32]);
        let ring = SigningKeyRing::<3>::bootstrap(7, old.verifying_key().to_bytes(), 1, 1).unwrap();
        let mut controller = SigningKeyController::new(KeyStore {
            ring,
            fail_commit: false,
        });
        controller
            .stage_rotation(7, 8, new.verifying_key().to_bytes(), 2, 10)
            .unwrap();
        let staged = controller.ring().unwrap();
        assert!(staged.resolve(7, 10).is_ok());
        assert_eq!(
            staged.resolve(8, 10),
            Err(SigningKeyResolutionError::PendingKey)
        );

        controller.store.fail_commit = true;
        assert_eq!(
            controller.activate_rotation(7, 8, 10),
            Err(SigningKeyLifecycleError::Backend(TestError::Offline))
        );
        let interrupted = controller.ring().unwrap();
        assert!(interrupted.resolve(7, 10).is_ok());
        assert_eq!(
            interrupted.resolve(8, 10),
            Err(SigningKeyResolutionError::PendingKey)
        );

        controller.store.fail_commit = false;
        controller.activate_rotation(7, 8, 10).unwrap();
        let active = controller.ring().unwrap();
        assert_eq!(
            active.resolve(7, 10),
            Err(SigningKeyResolutionError::RevokedKey)
        );
        assert!(active.resolve(8, 10).is_ok());
        assert_eq!(
            active.resolve(99, 10),
            Err(SigningKeyResolutionError::UnknownKey)
        );

        assert_eq!(
            controller.revoke_key(8, 11),
            Err(SigningKeyLifecycleError::InvalidState),
            "the last active root cannot be revoked without a replacement"
        );

        controller
            .stage_rotation(8, 9, old.verifying_key().to_bytes(), 3, 20)
            .unwrap();
        let revoked = controller.revoke_key(9, 20).unwrap();
        assert_eq!(revoked.key_id, 9);
        assert_eq!(
            controller.ring().unwrap().resolve(9, 20),
            Err(SigningKeyResolutionError::RevokedKey)
        );
    }

    struct RollbackConnector {
        floor: u32,
        failure: Option<TestError>,
        profile: SecurityRootProfile,
    }

    impl ProtectedRollbackBackend for RollbackConnector {
        type Error = TestError;

        fn load_floor(&self) -> Result<u32, Self::Error> {
            self.failure.map_or(Ok(self.floor), Err)
        }

        fn commit_floor_if_higher(&mut self, version: u32) -> Result<(), Self::Error> {
            if let Some(error) = self.failure {
                return Err(error);
            }
            self.floor = self.floor.max(version);
            Ok(())
        }
    }

    impl AttestedRollbackBackend for RollbackConnector {
        fn security_profile(&self) -> SecurityRootProfile {
            self.profile
        }
    }

    fn root(kind: SecurityRootKind) -> SecurityRootProfile {
        SecurityRootProfile {
            backend_id: "test-root",
            generation: 1,
            root: kind,
            non_exportable_keys: !matches!(kind, SecurityRootKind::SoftwareOnly),
            monotonic_counter: !matches!(kind, SecurityRootKind::SoftwareOnly),
            constant_time_scope: if matches!(kind, SecurityRootKind::SoftwareOnly) {
                ConstantTimeScope::SoftwareTagCompareOnly
            } else {
                ConstantTimeScope::HardwareBoundary
            },
        }
    }

    fn manifest(image: &[u8], signing: &SigningKey) -> SignedImageManifest {
        let mut manifest = SignedImageManifest {
            key_id: 7,
            version: 2,
            image_len: image.len() as u32,
            load_addr: 0x1000,
            entry_addr: 0x1001,
            stack_top: 0x2000_1000,
            measurement: sha256(image),
            signature: [0; 64],
        };
        manifest.signature = signing.sign(&manifest.signing_digest()).to_bytes();
        manifest
    }

    #[test]
    fn protected_receipt_distinguishes_hardware_and_software_and_connector_loss() {
        let signing = SigningKey::from_bytes(&[3; 32]);
        let mut keys = PinnedKeyPolicy::<1>::new();
        keys.pin(7, signing.verifying_key().to_bytes());
        let image = b"rooted image";
        let manifest = manifest(image, &signing);
        let vectors = BootVectorPolicy::cortex_m(0x1000, 0x4000, 0x2000_0000, 0x2000_2000);

        let hardware = RollbackConnector {
            floor: 1,
            failure: None,
            profile: root(SecurityRootKind::Efuse),
        };
        let (_, receipt) = verify_signed_measurement_attested(
            sha256(image),
            &manifest,
            &keys,
            vectors,
            &Ed25519ImageVerifier,
            &hardware,
        )
        .unwrap();
        assert!(receipt.hardware_rooted);

        let software = RollbackConnector {
            floor: 1,
            failure: None,
            profile: root(SecurityRootKind::SoftwareOnly),
        };
        let (_, receipt) = verify_signed_measurement_attested(
            sha256(image),
            &manifest,
            &keys,
            vectors,
            &Ed25519ImageVerifier,
            &software,
        )
        .unwrap();
        assert!(!receipt.hardware_rooted);

        let offline = RollbackConnector {
            floor: 1,
            failure: Some(TestError::Corrupt),
            profile: root(SecurityRootKind::SecureElement),
        };
        assert_eq!(
            verify_signed_measurement_attested(
                sha256(image),
                &manifest,
                &keys,
                vectors,
                &Ed25519ImageVerifier,
                &offline,
            ),
            Err(AttestedProtectedBootError::Backend(TestError::Corrupt))
        );
    }

    struct FleetBackend {
        fail: bool,
        profile: SecurityRootProfile,
    }

    impl FleetKeyBackend for FleetBackend {
        type Error = TestError;

        fn security_profile(&self) -> SecurityRootProfile {
            self.profile
        }

        fn derive(
            &mut self,
            request: FleetKeyRequest,
            output: &mut [u8; 32],
        ) -> Result<(), Self::Error> {
            if self.fail {
                return Err(TestError::Offline);
            }
            let mut context = [0u8; 28];
            context[..4].copy_from_slice(&request.fleet_id.to_le_bytes());
            context[4..20].copy_from_slice(&request.device_id);
            context[20..24].copy_from_slice(&request.epoch.to_le_bytes());
            context[24..28].copy_from_slice(&(request.purpose as u32).to_le_bytes());
            *output = hmac_sha256(&[0xA5; 32], &context);
            Ok(())
        }

        fn import_wrapped(
            &mut self,
            wrapped: &[u8],
            output: &mut [u8; 32],
        ) -> Result<(), Self::Error> {
            if self.fail {
                return Err(TestError::Offline);
            }
            *output = hmac_sha256(&[0x5A; 32], wrapped);
            Ok(())
        }
    }

    #[test]
    fn fleet_ownership_and_aead_member_scope_are_explicit_and_bad_tags_zero_output() {
        let mut backend = FleetBackend {
            fail: false,
            profile: root(SecurityRootKind::TrustZone),
        };
        let request = FleetKeyRequest {
            fleet_id: 4,
            device_id: [7; 16],
            epoch: 2,
            purpose: FleetKeyPurpose::TelemetryAead,
        };
        let mut derived = [0; 32];
        let receipt = derive_fleet_key(&mut backend, request, &mut derived).unwrap();
        assert_eq!(receipt.ownership, FleetKeyOwnership::DeviceRootDerived);
        let mut imported = [0; 32];
        let receipt = import_fleet_key(
            &mut backend,
            FleetKeyPurpose::FirmwareAead,
            3,
            b"wrapped",
            &mut imported,
        )
        .unwrap();
        assert_eq!(receipt.ownership, FleetKeyOwnership::FleetAuthorityImported);
        assert_ne!(derived, imported);

        let key: [u8; 16] = derived[..16].try_into().unwrap();
        let nonce = [9; ccm::NONCE_LEN];
        let mut sealed = [0u8; 64];
        let (sealed_len, seal) = seal_aead_member(
            &key,
            &nonce,
            b"route",
            b"payload",
            &mut sealed,
            FleetKeyOwnership::DeviceRootDerived,
        )
        .unwrap();
        assert_eq!(seal.backend_id, "nobro-crypto/aes-ccm-128-8");
        let mut opened = [0xAA; 32];
        let (opened_len, _) = open_aead_member(
            &key,
            &nonce,
            b"route",
            &sealed[..sealed_len],
            &mut opened,
            FleetKeyOwnership::DeviceRootDerived,
        )
        .unwrap();
        assert_eq!(&opened[..opened_len], b"payload");
        sealed[0] ^= 1;
        opened.fill(0xAA);
        assert_eq!(
            open_aead_member(
                &key,
                &nonce,
                b"route",
                &sealed[..sealed_len],
                &mut opened,
                FleetKeyOwnership::DeviceRootDerived,
            ),
            Err(CcmError::BadTag)
        );
        assert_eq!(&opened[..7], &[0; 7]);

        backend.fail = true;
        derived.fill(0xAA);
        assert_eq!(
            derive_fleet_key(&mut backend, request, &mut derived),
            Err(FleetKeyError::Backend(TestError::Offline))
        );
        assert_eq!(derived, [0; 32]);
    }

    #[derive(Clone, Copy)]
    struct BootStore {
        state: PersistentBootState,
    }

    impl MonotonicBootStore for BootStore {
        fn load(&self) -> Result<PersistentBootState, PersistentBootError> {
            Ok(self.state)
        }

        fn commit_if_newer(
            &mut self,
            state: PersistentBootState,
        ) -> Result<(), PersistentBootError> {
            if state.generation <= self.state.generation {
                return Err(PersistentBootError::Rollback);
            }
            self.state = state;
            Ok(())
        }
    }

    #[test]
    fn scoped_boot_admits_one_full_image_history_and_rejects_delta_or_oversize() {
        let initial = PersistentBootState {
            generation: 1,
            active_slot: Slot::A,
            confirmed_version: 1,
            pending: None,
            trial_attempted: false,
        };
        let plan = VerifiedBootPlan {
            version: 2,
            image_len: 1024,
            load_addr: 0x1000,
            entry_addr: 0x1001,
            stack_top: 0x2000_1000,
        };
        let layout = BootSlotLayout {
            slot_count: 2,
            slot_bytes: 2048,
            rollback_history_slots: 1,
        };
        let mut controller = PersistentBootController::new(BootStore { state: initial });
        assert_eq!(
            stage_scoped_full_image(
                &mut controller,
                Slot::B,
                plan,
                layout,
                UpdatePayloadKind::Delta { base_version: 1 },
            ),
            Err(ScopedBootStageError::DeltaUnsupported)
        );
        let receipt = stage_scoped_full_image(
            &mut controller,
            Slot::B,
            plan,
            layout,
            UpdatePayloadKind::FullImage,
        )
        .unwrap();
        assert_eq!(receipt.rollback_history_slots, 1);

        let mut other = PersistentBootController::new(BootStore { state: initial });
        assert_eq!(
            stage_scoped_full_image(
                &mut other,
                Slot::B,
                VerifiedBootPlan {
                    image_len: 4096,
                    ..plan
                },
                layout,
                UpdatePayloadKind::FullImage,
            ),
            Err(ScopedBootStageError::ImageTooLarge)
        );
    }
}
