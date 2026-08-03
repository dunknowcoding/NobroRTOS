//! Asymmetric image policy, persistent boot state, protected keys, and report envelopes.

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use nobro_crypto::sha256::{hmac_sha256, sha256, Sha256};

use crate::{verify_tag, BootPlanError, BootVectorPolicy, Slot, VerifiedBootPlan};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignatureBackendError {
    InvalidPublicKey,
    InvalidSignature,
}

/// Fixed-footprint asymmetric verifier selected by the board boot policy.
///
/// Manifests keep bounded 32-byte public keys and 64-byte signatures. A target
/// may use the built-in Ed25519 backend or provide an accelerator/secure-element
/// Ed25519 implementation without changing slot, rollback, or vector
/// validation. Other algorithms require a new manifest schema and signing
/// domain rather than substituting a verifier behind this contract.
pub trait AsymmetricImageVerifier {
    fn verify(
        &self,
        public_key: &[u8; 32],
        digest: &[u8; 32],
        signature: &[u8; 64],
    ) -> Result<(), SignatureBackendError>;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ed25519ImageVerifier;

impl AsymmetricImageVerifier for Ed25519ImageVerifier {
    fn verify(
        &self,
        public_key: &[u8; 32],
        digest: &[u8; 32],
        signature: &[u8; 64],
    ) -> Result<(), SignatureBackendError> {
        let verifying = VerifyingKey::from_bytes(public_key)
            .map_err(|_| SignatureBackendError::InvalidPublicKey)?;
        let signature = Signature::from_bytes(signature);
        verifying
            .verify(digest, &signature)
            .map_err(|_| SignatureBackendError::InvalidSignature)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignedImageManifest {
    pub key_id: u32,
    pub version: u32,
    pub image_len: u32,
    pub load_addr: u32,
    pub entry_addr: u32,
    pub stack_top: u32,
    pub measurement: [u8; 32],
    pub signature: [u8; 64],
}

impl SignedImageManifest {
    pub fn signing_digest(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(b"NobroRTOS Ed25519 image manifest v1");
        hash.update(&self.key_id.to_le_bytes());
        hash.update(&self.version.to_le_bytes());
        hash.update(&self.image_len.to_le_bytes());
        hash.update(&self.load_addr.to_le_bytes());
        hash.update(&self.entry_addr.to_le_bytes());
        hash.update(&self.stack_top.to_le_bytes());
        hash.update(&self.measurement);
        hash.finalize()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignedBootError {
    UnknownKey,
    InvalidPublicKey,
    InvalidSignature,
    Tampered,
    Rollback,
    Plan(BootPlanError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerifiedSignedImage {
    plan: VerifiedBootPlan,
    manifest_digest: [u8; 32],
    image_measurement: [u8; 32],
    manifest: SignedImageManifest,
}

impl VerifiedSignedImage {
    pub const fn plan(&self) -> VerifiedBootPlan {
        self.plan
    }

    pub const fn manifest_digest(&self) -> [u8; 32] {
        self.manifest_digest
    }

    pub const fn image_measurement(&self) -> [u8; 32] {
        self.image_measurement
    }

    pub const fn manifest(&self) -> SignedImageManifest {
        self.manifest
    }
}

pub struct PinnedKeyPolicy<const N: usize> {
    entries: [Option<(u32, [u8; 32])>; N],
}

impl<const N: usize> PinnedKeyPolicy<N> {
    pub const fn new() -> Self {
        Self { entries: [None; N] }
    }

    pub fn pin(&mut self, id: u32, key: [u8; 32]) -> bool {
        if self.entries.iter().flatten().any(|entry| entry.0 == id) {
            return false;
        }
        let Some(slot) = self.entries.iter_mut().find(|entry| entry.is_none()) else {
            return false;
        };
        *slot = Some((id, key));
        true
    }

    pub fn key(&self, id: u32) -> Option<&[u8; 32]> {
        self.entries
            .iter()
            .flatten()
            .find(|entry| entry.0 == id)
            .map(|entry| &entry.1)
    }
}

impl<const N: usize> Default for PinnedKeyPolicy<N> {
    fn default() -> Self {
        Self::new()
    }
}

pub fn verify_signed_boot<const N: usize>(
    image: &[u8],
    manifest: &SignedImageManifest,
    keys: &PinnedKeyPolicy<N>,
    vectors: BootVectorPolicy,
    rollback_floor: u32,
) -> Result<VerifiedSignedImage, SignedBootError> {
    verify_signed_boot_with(
        image,
        manifest,
        keys,
        vectors,
        rollback_floor,
        &Ed25519ImageVerifier,
    )
}

pub fn verify_signed_boot_with<const N: usize>(
    image: &[u8],
    manifest: &SignedImageManifest,
    keys: &PinnedKeyPolicy<N>,
    vectors: BootVectorPolicy,
    rollback_floor: u32,
    verifier: &impl AsymmetricImageVerifier,
) -> Result<VerifiedSignedImage, SignedBootError> {
    if image.len() != manifest.image_len as usize {
        return Err(SignedBootError::Plan(BootPlanError::SizeMismatch));
    }
    verify_signed_measurement_with(
        sha256(image),
        manifest,
        keys,
        vectors,
        rollback_floor,
        verifier,
    )
}

/// Verify a signed manifest against a measurement produced by a board's
/// streaming flash reader.
///
/// This is the reset-time counterpart to [`verify_signed_boot`]: a boot adapter
/// need not copy a complete memory-mapped slot into RAM to re-authenticate it.
pub fn verify_signed_measurement<const N: usize>(
    image_measurement: [u8; 32],
    manifest: &SignedImageManifest,
    keys: &PinnedKeyPolicy<N>,
    vectors: BootVectorPolicy,
    rollback_floor: u32,
) -> Result<VerifiedSignedImage, SignedBootError> {
    verify_signed_measurement_with(
        image_measurement,
        manifest,
        keys,
        vectors,
        rollback_floor,
        &Ed25519ImageVerifier,
    )
}

pub fn verify_signed_measurement_with<const N: usize>(
    image_measurement: [u8; 32],
    manifest: &SignedImageManifest,
    keys: &PinnedKeyPolicy<N>,
    vectors: BootVectorPolicy,
    rollback_floor: u32,
    verifier: &impl AsymmetricImageVerifier,
) -> Result<VerifiedSignedImage, SignedBootError> {
    if image_measurement != manifest.measurement {
        return Err(SignedBootError::Tampered);
    }
    if manifest.version < rollback_floor {
        return Err(SignedBootError::Rollback);
    }
    let key = keys
        .key(manifest.key_id)
        .ok_or(SignedBootError::UnknownKey)?;
    verifier
        .verify(key, &manifest.signing_digest(), &manifest.signature)
        .map_err(|error| match error {
            SignatureBackendError::InvalidPublicKey => SignedBootError::InvalidPublicKey,
            SignatureBackendError::InvalidSignature => SignedBootError::InvalidSignature,
        })?;

    let end = manifest
        .load_addr
        .checked_add(manifest.image_len)
        .ok_or(SignedBootError::Plan(BootPlanError::AddressRange))?;
    if manifest.load_addr < vectors.min_load_addr || end > vectors.max_end_addr {
        return Err(SignedBootError::Plan(BootPlanError::AddressRange));
    }
    let entry = if vectors.require_thumb_entry {
        if manifest.entry_addr & 1 == 0 {
            return Err(SignedBootError::Plan(BootPlanError::InvalidEntry));
        }
        manifest.entry_addr & !1
    } else {
        manifest.entry_addr
    };
    if entry < manifest.load_addr || entry >= end {
        return Err(SignedBootError::Plan(BootPlanError::InvalidEntry));
    }
    if vectors.stack_alignment == 0
        || manifest.stack_top <= vectors.min_stack_addr
        || manifest.stack_top > vectors.max_stack_addr
        || manifest.stack_top % vectors.stack_alignment != 0
    {
        return Err(SignedBootError::Plan(BootPlanError::InvalidStack));
    }
    Ok(VerifiedSignedImage {
        plan: VerifiedBootPlan {
            version: manifest.version,
            image_len: manifest.image_len,
            load_addr: manifest.load_addr,
            entry_addr: manifest.entry_addr,
            stack_top: manifest.stack_top,
        },
        manifest_digest: manifest.signing_digest(),
        image_measurement: manifest.measurement,
        manifest: *manifest,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PersistentBootState {
    pub generation: u32,
    pub active_slot: Slot,
    pub confirmed_version: u32,
    pub pending: Option<(Slot, u32)>,
    pub trial_attempted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersistentBootError {
    Storage,
    Rollback,
    NoPendingTrial,
    VersionMismatch,
    GenerationExhausted,
}

pub trait MonotonicBootStore {
    fn load(&self) -> Result<PersistentBootState, PersistentBootError>;
    fn commit_if_newer(&mut self, state: PersistentBootState) -> Result<(), PersistentBootError>;
}

/// Board-owned protected monotonic floor.
///
/// Implementations may use write-protected flash, OTP/eFuse, a secure element,
/// or a trusted companion. Loading and advancing the floor are separate from
/// image verification so a trial image cannot consume the rollback counter
/// before it is confirmed.
pub trait ProtectedRollbackBackend {
    type Error;

    fn load_floor(&self) -> Result<u32, Self::Error>;
    fn commit_floor_if_higher(&mut self, version: u32) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtectedBootError<E> {
    Backend(E),
    Verification(SignedBootError),
    Rollback,
}

pub fn verify_signed_measurement_protected<const N: usize, B: ProtectedRollbackBackend>(
    image_measurement: [u8; 32],
    manifest: &SignedImageManifest,
    keys: &PinnedKeyPolicy<N>,
    vectors: BootVectorPolicy,
    verifier: &impl AsymmetricImageVerifier,
    rollback: &B,
) -> Result<VerifiedSignedImage, ProtectedBootError<B::Error>> {
    let floor = rollback.load_floor().map_err(ProtectedBootError::Backend)?;
    verify_signed_measurement_with(image_measurement, manifest, keys, vectors, floor, verifier)
        .map_err(ProtectedBootError::Verification)
}

/// Advance the protected floor only after the trial image is confirmed.
pub fn commit_verified_rollback_floor<B: ProtectedRollbackBackend>(
    verified: &VerifiedSignedImage,
    rollback: &mut B,
) -> Result<(), ProtectedBootError<B::Error>> {
    let floor = rollback.load_floor().map_err(ProtectedBootError::Backend)?;
    let version = verified.plan().version;
    if version < floor {
        return Err(ProtectedBootError::Rollback);
    }
    rollback
        .commit_floor_if_higher(version)
        .map_err(ProtectedBootError::Backend)
}

pub struct PersistentBootController<S> {
    store: S,
}

impl<S: MonotonicBootStore> PersistentBootController<S> {
    pub const fn new(store: S) -> Self {
        Self { store }
    }

    pub fn stage(&mut self, slot: Slot, plan: VerifiedBootPlan) -> Result<(), PersistentBootError> {
        let mut state = self.store.load()?;
        if plan.version <= state.confirmed_version || slot == state.active_slot {
            return Err(PersistentBootError::Rollback);
        }
        state.generation = next_persistent_generation(state.generation)?;
        state.pending = Some((slot, plan.version));
        state.trial_attempted = false;
        self.store.commit_if_newer(state)
    }

    pub fn stage_signed(
        &mut self,
        slot: Slot,
        verified: &VerifiedSignedImage,
    ) -> Result<(), PersistentBootError> {
        self.stage(slot, verified.plan())
    }

    pub fn select_boot(&mut self) -> Result<(Slot, u32), PersistentBootError> {
        let mut state = self.store.load()?;
        match state.pending {
            Some(candidate) if !state.trial_attempted => {
                state.generation = next_persistent_generation(state.generation)?;
                state.trial_attempted = true;
                self.store.commit_if_newer(state)?;
                Ok(candidate)
            }
            Some(_) => {
                state.generation = next_persistent_generation(state.generation)?;
                state.pending = None;
                state.trial_attempted = false;
                self.store.commit_if_newer(state)?;
                Ok((state.active_slot, state.confirmed_version))
            }
            None => Ok((state.active_slot, state.confirmed_version)),
        }
    }

    pub fn confirm(&mut self, version: u32) -> Result<(), PersistentBootError> {
        let mut state = self.store.load()?;
        let Some((slot, pending_version)) = state.pending else {
            return Err(PersistentBootError::NoPendingTrial);
        };
        if !state.trial_attempted || version != pending_version {
            return Err(PersistentBootError::VersionMismatch);
        }
        state.generation = next_persistent_generation(state.generation)?;
        state.active_slot = slot;
        state.confirmed_version = version;
        state.pending = None;
        state.trial_attempted = false;
        self.store.commit_if_newer(state)
    }

    pub fn into_store(self) -> S {
        self.store
    }
}

fn next_persistent_generation(generation: u32) -> Result<u32, PersistentBootError> {
    generation
        .checked_add(1)
        .ok_or(PersistentBootError::GenerationExhausted)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProvisionPolicy {
    pub min_key_id: u32,
    pub max_key_id: u32,
    pub allow_replace: bool,
}

pub trait ProtectedKeyBackend {
    type Error;
    fn contains(&self, id: u32) -> Result<bool, Self::Error>;
    fn provision(&mut self, id: u32, key: &[u8; 32]) -> Result<(), Self::Error>;
    fn revoke(&mut self, id: u32) -> Result<(), Self::Error>;
    fn authenticate(&self, id: u32, message: &[u8], tag: &[u8; 32]) -> Result<bool, Self::Error>;
}

pub fn provision_protected_key<B: ProtectedKeyBackend>(
    backend: &mut B,
    policy: ProvisionPolicy,
    id: u32,
    key: &[u8; 32],
) -> Result<bool, B::Error> {
    if id < policy.min_key_id || id > policy.max_key_id {
        return Ok(false);
    }
    if backend.contains(id)? && !policy.allow_replace {
        return Ok(false);
    }
    backend.provision(id, key)?;
    Ok(true)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthenticatedReportEnvelope {
    pub sequence: u32,
    pub payload_len: u32,
    pub payload_digest: [u8; 32],
    pub tag: [u8; 32],
}

/// A report payload admitted through the keyed authentication boundary.
///
/// Its fields are private so code making a trust decision cannot construct this
/// token from an unkeyed diagnostic checksum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerifiedReportPayload<'a> {
    sequence: u32,
    payload: &'a [u8],
}

impl<'a> VerifiedReportPayload<'a> {
    pub const fn sequence(&self) -> u32 {
        self.sequence
    }

    pub const fn payload(&self) -> &'a [u8] {
        self.payload
    }
}

impl AuthenticatedReportEnvelope {
    pub fn seal(key: &[u8; 32], sequence: u32, payload: &[u8]) -> Self {
        let payload_digest = sha256(payload);
        let tag = report_tag(key, sequence, payload.len() as u32, &payload_digest);
        Self {
            sequence,
            payload_len: payload.len() as u32,
            payload_digest,
            tag,
        }
    }

    pub fn verify(&self, key: &[u8; 32], payload: &[u8]) -> bool {
        self.open(key, payload).is_some()
    }

    /// Authenticate an untrusted report payload before it enters a trust
    /// decision. Unkeyed report diagnostics are deliberately not accepted here.
    pub fn open<'a>(&self, key: &[u8; 32], payload: &'a [u8]) -> Option<VerifiedReportPayload<'a>> {
        if payload.len() != self.payload_len as usize || sha256(payload) != self.payload_digest {
            return None;
        }
        if !verify_tag(
            &self.tag,
            &report_tag(key, self.sequence, self.payload_len, &self.payload_digest),
        ) {
            return None;
        }
        Some(VerifiedReportPayload {
            sequence: self.sequence,
            payload,
        })
    }
}

fn report_tag(key: &[u8; 32], sequence: u32, len: u32, digest: &[u8; 32]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"NobroRTOS report envelope v1");
    hash.update(&sequence.to_le_bytes());
    hash.update(&len.to_le_bytes());
    hash.update(digest);
    hmac_sha256(key, &hash.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    struct MemoryKeys {
        id: Option<u32>,
        key: [u8; 32],
    }

    impl ProtectedKeyBackend for MemoryKeys {
        type Error = ();
        fn contains(&self, id: u32) -> Result<bool, Self::Error> {
            Ok(self.id == Some(id))
        }
        fn provision(&mut self, id: u32, key: &[u8; 32]) -> Result<(), Self::Error> {
            self.id = Some(id);
            self.key.copy_from_slice(key);
            Ok(())
        }
        fn revoke(&mut self, id: u32) -> Result<(), Self::Error> {
            if self.id == Some(id) {
                self.key.fill(0);
                self.id = None;
            }
            Ok(())
        }
        fn authenticate(
            &self,
            id: u32,
            message: &[u8],
            tag: &[u8; 32],
        ) -> Result<bool, Self::Error> {
            Ok(self.id == Some(id) && verify_tag(&hmac_sha256(&self.key, message), tag))
        }
    }

    fn signed_manifest(image: &[u8], signing: &SigningKey) -> SignedImageManifest {
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
    fn pinned_ed25519_manifest_covers_vectors_measurement_and_version() {
        let signing = SigningKey::from_bytes(&[3; 32]);
        let mut keys = PinnedKeyPolicy::<1>::new();
        assert!(keys.pin(7, signing.verifying_key().to_bytes()));
        let image = b"signed image";
        let manifest = signed_manifest(image, &signing);
        let vectors = BootVectorPolicy::cortex_m(0x1000, 0x4000, 0x2000_0000, 0x2000_2000);
        assert!(verify_signed_boot(image, &manifest, &keys, vectors, 1).is_ok());

        let mut forged = manifest;
        forged.entry_addr = 0x1101;
        assert_eq!(
            verify_signed_boot(image, &forged, &keys, vectors, 1),
            Err(SignedBootError::InvalidSignature)
        );
        assert_eq!(
            verify_signed_boot(image, &manifest, &keys, vectors, 3),
            Err(SignedBootError::Rollback)
        );
    }

    struct MemoryRollback {
        floor: u32,
    }

    impl ProtectedRollbackBackend for MemoryRollback {
        type Error = ();

        fn load_floor(&self) -> Result<u32, Self::Error> {
            Ok(self.floor)
        }

        fn commit_floor_if_higher(&mut self, version: u32) -> Result<(), Self::Error> {
            if version > self.floor {
                self.floor = version;
            }
            Ok(())
        }
    }

    #[test]
    fn asymmetric_backend_and_protected_floor_are_separate_confirm_steps() {
        let signing = SigningKey::from_bytes(&[4; 32]);
        let mut keys = PinnedKeyPolicy::<1>::new();
        assert!(keys.pin(7, signing.verifying_key().to_bytes()));
        let image = b"protected signed image";
        let manifest = signed_manifest(image, &signing);
        let vectors = BootVectorPolicy::cortex_m(0x1000, 0x4000, 0x2000_0000, 0x2000_2000);
        let mut rollback = MemoryRollback { floor: 1 };

        let verified = verify_signed_measurement_protected(
            sha256(image),
            &manifest,
            &keys,
            vectors,
            &Ed25519ImageVerifier,
            &rollback,
        )
        .unwrap();
        assert_eq!(rollback.floor, 1);
        commit_verified_rollback_floor(&verified, &mut rollback).unwrap();
        assert_eq!(rollback.floor, manifest.version);

        rollback.floor = manifest.version + 1;
        assert_eq!(
            verify_signed_measurement_protected(
                sha256(image),
                &manifest,
                &keys,
                vectors,
                &Ed25519ImageVerifier,
                &rollback,
            ),
            Err(ProtectedBootError::Verification(SignedBootError::Rollback))
        );
    }

    #[derive(Clone, Copy)]
    struct MemoryBootStore {
        state: PersistentBootState,
        fail: bool,
    }

    impl MonotonicBootStore for MemoryBootStore {
        fn load(&self) -> Result<PersistentBootState, PersistentBootError> {
            if self.fail {
                Err(PersistentBootError::Storage)
            } else {
                Ok(self.state)
            }
        }

        fn commit_if_newer(
            &mut self,
            state: PersistentBootState,
        ) -> Result<(), PersistentBootError> {
            if self.fail {
                return Err(PersistentBootError::Storage);
            }
            if state.generation <= self.state.generation
                || state.confirmed_version < self.state.confirmed_version
            {
                return Err(PersistentBootError::Rollback);
            }
            self.state = state;
            Ok(())
        }
    }

    fn plan(version: u32) -> VerifiedBootPlan {
        VerifiedBootPlan {
            version,
            image_len: 16,
            load_addr: 0x1000,
            entry_addr: 0x1001,
            stack_top: 0x2000_1000,
        }
    }

    #[test]
    fn persistent_trial_confirms_or_reverts_and_storage_failure_is_closed() {
        let initial = PersistentBootState {
            generation: 1,
            active_slot: Slot::A,
            confirmed_version: 1,
            pending: None,
            trial_attempted: false,
        };
        let mut controller = PersistentBootController::new(MemoryBootStore {
            state: initial,
            fail: false,
        });
        controller.stage(Slot::B, plan(2)).unwrap();
        assert_eq!(controller.select_boot(), Ok((Slot::B, 2)));
        assert_eq!(controller.select_boot(), Ok((Slot::A, 1)));
        assert_eq!(
            controller.confirm(2),
            Err(PersistentBootError::NoPendingTrial)
        );

        controller.stage(Slot::B, plan(2)).unwrap();
        assert_eq!(controller.select_boot(), Ok((Slot::B, 2)));
        controller.confirm(2).unwrap();
        assert_eq!(controller.select_boot(), Ok((Slot::B, 2)));

        let mut failed = PersistentBootController::new(MemoryBootStore {
            state: initial,
            fail: true,
        });
        assert_eq!(
            failed.stage(Slot::B, plan(2)),
            Err(PersistentBootError::Storage)
        );
        assert_eq!(failed.select_boot(), Err(PersistentBootError::Storage));

        let mut exhausted = PersistentBootController::new(MemoryBootStore {
            state: PersistentBootState {
                generation: u32::MAX,
                ..initial
            },
            fail: false,
        });
        assert_eq!(
            exhausted.stage(Slot::B, plan(2)),
            Err(PersistentBootError::GenerationExhausted)
        );
        assert_eq!(
            exhausted.into_store().state.generation,
            u32::MAX,
            "exhaustion must not wrap or commit"
        );
    }

    #[test]
    fn authenticated_report_rejects_payload_metadata_and_tag_changes() {
        let key = [9; 32];
        let payload = b"health report";
        let envelope = AuthenticatedReportEnvelope::seal(&key, 4, payload);
        let verified = envelope.open(&key, payload).unwrap();
        assert_eq!(verified.sequence(), 4);
        assert_eq!(verified.payload(), payload);
        assert!(envelope.verify(&key, payload));
        assert!(envelope.open(&[8; 32], payload).is_none());
        assert!(!envelope.verify(&key, b"health repors"));
        let mut changed = envelope;
        changed.sequence += 1;
        assert!(!changed.verify(&key, payload));
        changed = envelope;
        changed.tag[0] ^= 1;
        assert!(!changed.verify(&key, payload));
    }

    #[test]
    fn protected_backend_policy_rejects_range_and_replacement() {
        let mut backend = MemoryKeys {
            id: None,
            key: [0; 32],
        };
        let policy = ProvisionPolicy {
            min_key_id: 10,
            max_key_id: 20,
            allow_replace: false,
        };
        assert_eq!(
            provision_protected_key(&mut backend, policy, 9, &[1; 32]),
            Ok(false)
        );
        assert_eq!(
            provision_protected_key(&mut backend, policy, 10, &[2; 32]),
            Ok(true)
        );
        assert_eq!(
            provision_protected_key(&mut backend, policy, 10, &[3; 32]),
            Ok(false)
        );
        let tag = hmac_sha256(&[2; 32], b"challenge");
        assert_eq!(backend.authenticate(10, b"challenge", &tag), Ok(true));
        backend.revoke(10).unwrap();
        assert_eq!(backend.authenticate(10, b"challenge", &tag), Ok(false));
        assert_eq!(backend.key, [0; 32]);
    }
}
