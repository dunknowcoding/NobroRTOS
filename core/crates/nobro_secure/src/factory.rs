//! Authenticated, replay-safe factory provisioning.

use nobro_crypto::sha256::{hmac_sha256, Sha256};

use crate::{provision_protected_key, ProtectedKeyBackend, ProvisionPolicy};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FactoryProvisionRequest {
    pub sequence: u32,
    pub authority_key_id: u32,
    pub new_key_id: u32,
    pub new_key: [u8; 32],
    pub tag: [u8; 32],
}

impl FactoryProvisionRequest {
    pub fn authenticated(
        authority_key: &[u8; 32],
        sequence: u32,
        authority_key_id: u32,
        new_key_id: u32,
        new_key: [u8; 32],
    ) -> Self {
        let digest = request_digest(sequence, authority_key_id, new_key_id, &new_key);
        Self {
            sequence,
            authority_key_id,
            new_key_id,
            new_key,
            tag: hmac_sha256(authority_key, &digest),
        }
    }

    fn digest(&self) -> [u8; 32] {
        request_digest(
            self.sequence,
            self.authority_key_id,
            self.new_key_id,
            &self.new_key,
        )
    }
}

pub trait FactorySequenceStore {
    type Error;

    fn load(&self) -> Result<u32, Self::Error>;
    fn commit_if_newer(&mut self, sequence: u32) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FactoryProvisionError<BackendError, StoreError> {
    Backend(BackendError),
    Store(StoreError),
    Replay,
    Unauthorized,
    Policy,
    ExistingKeyConflict,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FactoryProvisionReceipt {
    pub sequence: u32,
    pub key_id: u32,
    /// True when a reset occurred after key installation but before sequence
    /// commit and the identical authenticated request completed the transaction.
    pub recovered: bool,
}

pub struct AuthenticatedFactoryFlow<B, S> {
    backend: B,
    sequences: S,
}

impl<B, S> AuthenticatedFactoryFlow<B, S>
where
    B: ProtectedKeyBackend,
    S: FactorySequenceStore,
{
    pub const fn new(backend: B, sequences: S) -> Self {
        Self { backend, sequences }
    }

    pub fn into_parts(self) -> (B, S) {
        (self.backend, self.sequences)
    }

    pub fn apply(
        &mut self,
        policy: ProvisionPolicy,
        request: &FactoryProvisionRequest,
    ) -> Result<FactoryProvisionReceipt, FactoryProvisionError<B::Error, S::Error>> {
        // Replacement makes reset recovery ambiguous: after a cut, the flow
        // cannot distinguish the old protected key from a partially replaced one.
        if policy.allow_replace {
            return Err(FactoryProvisionError::Policy);
        }
        let committed = self
            .sequences
            .load()
            .map_err(FactoryProvisionError::Store)?;
        if request.sequence <= committed {
            return Err(FactoryProvisionError::Replay);
        }
        let authenticated = self
            .backend
            .authenticate(request.authority_key_id, &request.digest(), &request.tag)
            .map_err(FactoryProvisionError::Backend)?;
        if !authenticated {
            return Err(FactoryProvisionError::Unauthorized);
        }

        let exists = self
            .backend
            .contains(request.new_key_id)
            .map_err(FactoryProvisionError::Backend)?;
        let recovered = if exists {
            let challenge = recovery_challenge(request);
            let proof = hmac_sha256(&request.new_key, &challenge);
            let same_key = self
                .backend
                .authenticate(request.new_key_id, &challenge, &proof)
                .map_err(FactoryProvisionError::Backend)?;
            if !same_key {
                return Err(FactoryProvisionError::ExistingKeyConflict);
            }
            true
        } else {
            let accepted = provision_protected_key(
                &mut self.backend,
                policy,
                request.new_key_id,
                &request.new_key,
            )
            .map_err(FactoryProvisionError::Backend)?;
            if !accepted {
                return Err(FactoryProvisionError::Policy);
            }
            false
        };

        self.sequences
            .commit_if_newer(request.sequence)
            .map_err(FactoryProvisionError::Store)?;
        Ok(FactoryProvisionReceipt {
            sequence: request.sequence,
            key_id: request.new_key_id,
            recovered,
        })
    }
}

fn request_digest(
    sequence: u32,
    authority_key_id: u32,
    new_key_id: u32,
    new_key: &[u8; 32],
) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"NobroRTOS authenticated factory provision v1");
    hash.update(&sequence.to_le_bytes());
    hash.update(&authority_key_id.to_le_bytes());
    hash.update(&new_key_id.to_le_bytes());
    hash.update(new_key);
    hash.finalize()
}

fn recovery_challenge(request: &FactoryProvisionRequest) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"NobroRTOS factory provision recovery v1");
    hash.update(&request.sequence.to_le_bytes());
    hash.update(&request.new_key_id.to_le_bytes());
    hash.update(&request.tag);
    hash.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verify_tag;
    use std::{cell::Cell, rc::Rc};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Fault {
        Cut,
        Full,
    }

    #[derive(Clone)]
    struct CutControl {
        mutations: Rc<Cell<usize>>,
        cut_after: Rc<Cell<Option<usize>>>,
    }

    impl CutControl {
        fn new() -> Self {
            Self {
                mutations: Rc::new(Cell::new(0)),
                cut_after: Rc::new(Cell::new(None)),
            }
        }

        fn mutate(&self) -> Result<(), Fault> {
            let count = self.mutations.get();
            if self.cut_after.get() == Some(count) {
                return Err(Fault::Cut);
            }
            self.mutations.set(count + 1);
            Ok(())
        }
    }

    struct Keys {
        ids: [Option<u32>; 4],
        keys: [[u8; 32]; 4],
        cut: CutControl,
    }

    impl Keys {
        fn with_authority(cut: CutControl, id: u32, key: [u8; 32]) -> Self {
            Self {
                ids: [Some(id), None, None, None],
                keys: [key, [0; 32], [0; 32], [0; 32]],
                cut,
            }
        }
    }

    impl ProtectedKeyBackend for Keys {
        type Error = Fault;

        fn contains(&self, id: u32) -> Result<bool, Self::Error> {
            Ok(self.ids.contains(&Some(id)))
        }

        fn provision(&mut self, id: u32, key: &[u8; 32]) -> Result<(), Self::Error> {
            self.cut.mutate()?;
            let index = self
                .ids
                .iter()
                .position(Option::is_none)
                .ok_or(Fault::Full)?;
            self.ids[index] = Some(id);
            self.keys[index] = *key;
            Ok(())
        }

        fn revoke(&mut self, id: u32) -> Result<(), Self::Error> {
            if let Some(index) = self.ids.iter().position(|entry| *entry == Some(id)) {
                self.cut.mutate()?;
                self.keys[index].fill(0);
                self.ids[index] = None;
            }
            Ok(())
        }

        fn authenticate(
            &self,
            id: u32,
            message: &[u8],
            tag: &[u8; 32],
        ) -> Result<bool, Self::Error> {
            let Some(index) = self.ids.iter().position(|entry| *entry == Some(id)) else {
                return Ok(false);
            };
            Ok(verify_tag(&hmac_sha256(&self.keys[index], message), tag))
        }
    }

    struct Sequence {
        value: u32,
        cut: CutControl,
    }

    impl FactorySequenceStore for Sequence {
        type Error = Fault;

        fn load(&self) -> Result<u32, Self::Error> {
            Ok(self.value)
        }

        fn commit_if_newer(&mut self, sequence: u32) -> Result<(), Self::Error> {
            if sequence <= self.value {
                return Err(Fault::Cut);
            }
            self.cut.mutate()?;
            self.value = sequence;
            Ok(())
        }
    }

    fn policy() -> ProvisionPolicy {
        ProvisionPolicy {
            min_key_id: 10,
            max_key_id: 20,
            allow_replace: false,
        }
    }

    #[test]
    fn authenticates_provisions_and_rejects_replay_or_tampering() {
        let authority = [0xA5; 32];
        let cut = CutControl::new();
        let keys = Keys::with_authority(cut.clone(), 1, authority);
        let sequence = Sequence {
            value: 4,
            cut: cut.clone(),
        };
        let mut flow = AuthenticatedFactoryFlow::new(keys, sequence);
        let request = FactoryProvisionRequest::authenticated(&authority, 5, 1, 10, [0x3C; 32]);
        let receipt = flow.apply(policy(), &request).unwrap();
        assert_eq!(receipt.key_id, 10);
        assert!(!receipt.recovered);
        assert_eq!(
            flow.apply(policy(), &request),
            Err(FactoryProvisionError::Replay)
        );

        let mut forged = FactoryProvisionRequest::authenticated(&authority, 6, 1, 11, [0x4D; 32]);
        forged.new_key[0] ^= 1;
        assert_eq!(
            flow.apply(policy(), &forged),
            Err(FactoryProvisionError::Unauthorized)
        );
    }

    #[test]
    fn every_factory_cut_recovers_idempotently_without_key_export() {
        let authority = [0xA5; 32];
        let request = FactoryProvisionRequest::authenticated(&authority, 1, 1, 10, [0x3C; 32]);

        for cut_at in 0..2 {
            let cut = CutControl::new();
            cut.cut_after.set(Some(cut_at));
            let keys = Keys::with_authority(cut.clone(), 1, authority);
            let sequence = Sequence {
                value: 0,
                cut: cut.clone(),
            };
            let mut flow = AuthenticatedFactoryFlow::new(keys, sequence);
            assert!(flow.apply(policy(), &request).is_err());
            let (keys, sequence) = flow.into_parts();
            cut.cut_after.set(None);
            let mut resumed = AuthenticatedFactoryFlow::new(keys, sequence);
            let receipt = resumed.apply(policy(), &request).unwrap();
            assert_eq!(receipt.recovered, cut_at == 1);
            let (keys, sequence) = resumed.into_parts();
            assert_eq!(sequence.value, 1);
            assert!(keys.contains(10).unwrap());
        }
    }

    #[test]
    fn existing_different_key_and_replacement_policy_fail_closed() {
        let authority = [0xA5; 32];
        let cut = CutControl::new();
        let mut keys = Keys::with_authority(cut.clone(), 1, authority);
        keys.provision(10, &[0x11; 32]).unwrap();
        let sequence = Sequence {
            value: 0,
            cut: cut.clone(),
        };
        let mut flow = AuthenticatedFactoryFlow::new(keys, sequence);
        let request = FactoryProvisionRequest::authenticated(&authority, 1, 1, 10, [0x22; 32]);
        assert_eq!(
            flow.apply(policy(), &request),
            Err(FactoryProvisionError::ExistingKeyConflict)
        );
        let mut replace = policy();
        replace.allow_replace = true;
        assert_eq!(
            flow.apply(replace, &request),
            Err(FactoryProvisionError::Policy)
        );
    }
}
