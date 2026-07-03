//! Security + data-integrity primitives (M174/M176/M177/M179/M180/M185).
#![cfg_attr(not(test), no_std)]

use nobro_crypto::sha256::{hmac_sha256, sha256};

/// Device attestation (M174): prove firmware identity by HMAC over a nonce + the
/// firmware measurement, keyed by a per-device secret. A verifier that shares the key
/// and knows the expected measurement recomputes and compares.
pub fn attest(device_key: &[u8; 32], firmware_measurement: &[u8; 32], nonce: &[u8]) -> [u8; 32] {
    let mut msg = [0u8; 64 + 32];
    msg[..32].copy_from_slice(firmware_measurement);
    let n = nonce.len().min(64);
    msg[32..32 + n].copy_from_slice(&nonce[..n]);
    hmac_sha256(device_key, &msg[..32 + n])
}

/// Constant-time-ish 32-byte compare (no early return on mismatch).
pub fn verify_tag(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for i in 0..32 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// Bounded key store (M176): fixed slots of (key_id -> 32-byte key), no heap. A slot can
/// be provisioned once and looked up; re-provisioning a used id is rejected.
pub struct KeyStore<const N: usize> {
    ids: [u32; N],
    keys: [[u8; 32]; N],
    used: [bool; N],
}

impl<const N: usize> Default for KeyStore<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> KeyStore<N> {
    pub const fn new() -> Self {
        Self { ids: [0; N], keys: [[0; 32]; N], used: [false; N] }
    }
    pub fn provision(&mut self, id: u32, key: [u8; 32]) -> bool {
        if self.get(id).is_some() {
            return false;
        }
        if let Some(i) = self.used.iter().position(|&u| !u) {
            self.ids[i] = id;
            self.keys[i] = key;
            self.used[i] = true;
            true
        } else {
            false
        }
    }
    pub fn get(&self, id: u32) -> Option<&[u8; 32]> {
        (0..N).find(|&i| self.used[i] && self.ids[i] == id).map(|i| &self.keys[i])
    }
}

/// OTA rollback protection (M177): accept an image only if its version is strictly
/// greater than the highest ever installed (monotonic anti-rollback counter).
pub struct RollbackGuard {
    min_version: u32,
}

impl RollbackGuard {
    pub const fn new(current_version: u32) -> Self {
        Self { min_version: current_version }
    }
    pub fn accept(&mut self, candidate_version: u32) -> bool {
        if candidate_version > self.min_version {
            self.min_version = candidate_version;
            true
        } else {
            false
        }
    }
    pub fn min_version(&self) -> u32 {
        self.min_version
    }
}

/// Tamper detection (M179): a baseline measurement (hash of a critical region) captured
/// at provisioning; `check` recomputes and flags any drift.
pub struct TamperSeal {
    baseline: [u8; 32],
}

impl TamperSeal {
    pub fn seal(region: &[u8]) -> Self {
        Self { baseline: sha256(region) }
    }
    pub fn intact(&self, region: &[u8]) -> bool {
        verify_tag(&self.baseline, &sha256(region))
    }
}

/// Hash-chained signed audit log (M180): each entry commits to the previous entry's tag,
/// so any deletion or reordering breaks the chain. Entries are HMAC'd with a log key.
pub struct AuditLog {
    prev_tag: [u8; 32],
    count: u32,
}

impl AuditLog {
    /// Genesis tag = HMAC(key, "genesis").
    pub fn new(key: &[u8; 32]) -> Self {
        Self { prev_tag: hmac_sha256(key, b"genesis"), count: 0 }
    }
    /// Append `event`; returns the new chain tag. tag = HMAC(key, prev_tag || seq || event).
    pub fn append(&mut self, key: &[u8; 32], event: &[u8]) -> [u8; 32] {
        let mut buf = [0u8; 32 + 4 + 96];
        buf[..32].copy_from_slice(&self.prev_tag);
        buf[32..36].copy_from_slice(&self.count.to_be_bytes());
        let n = event.len().min(96);
        buf[36..36 + n].copy_from_slice(&event[..n]);
        let tag = hmac_sha256(key, &buf[..36 + n]);
        self.prev_tag = tag;
        self.count += 1;
        tag
    }
    pub fn head(&self) -> [u8; 32] {
        self.prev_tag
    }
    pub fn count(&self) -> u32 {
        self.count
    }
}

/// Versioned config store with an integrity tag (M185): store a small config blob with a
/// version; `load` verifies the stored tag before returning the bytes.
pub struct ConfigStore<const N: usize> {
    version: u32,
    len: usize,
    bytes: [u8; N],
    tag: [u8; 32],
}

impl<const N: usize> ConfigStore<N> {
    pub const fn empty() -> Self {
        Self { version: 0, len: 0, bytes: [0; N], tag: [0; 32] }
    }
    pub fn store(&mut self, key: &[u8; 32], version: u32, data: &[u8]) -> bool {
        if data.len() > N {
            return false;
        }
        self.version = version;
        self.len = data.len();
        self.bytes[..data.len()].copy_from_slice(data);
        self.tag = hmac_sha256(key, &self.bytes[..self.len]);
        true
    }
    /// Return the config bytes only if the integrity tag still verifies.
    pub fn load(&self, key: &[u8; 32]) -> Option<(u32, &[u8])> {
        let expect = hmac_sha256(key, &self.bytes[..self.len]);
        if verify_tag(&expect, &self.tag) {
            Some((self.version, &self.bytes[..self.len]))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attestation_accepts_genuine_rejects_forged() {
        let key = [9u8; 32];
        let fw = sha256(b"firmware v1 image");
        let nonce = [1, 2, 3, 4, 5, 6, 7, 8];
        let tag = attest(&key, &fw, &nonce);
        // verifier recomputes with the same inputs
        assert!(verify_tag(&tag, &attest(&key, &fw, &nonce)));
        // wrong firmware measurement -> different tag
        let fw2 = sha256(b"firmware v2 image");
        assert!(!verify_tag(&tag, &attest(&key, &fw2, &nonce)));
    }

    #[test]
    fn key_store_provisions_once() {
        let mut ks = KeyStore::<2>::new();
        assert!(ks.provision(1, [1; 32]));
        assert!(ks.provision(2, [2; 32]));
        assert!(!ks.provision(1, [9; 32])); // dup id rejected
        assert!(!ks.provision(3, [3; 32])); // full
        assert_eq!(ks.get(2), Some(&[2u8; 32]));
        assert_eq!(ks.get(9), None);
    }

    #[test]
    fn rollback_guard_is_monotonic() {
        let mut g = RollbackGuard::new(5);
        assert!(!g.accept(5)); // not strictly greater
        assert!(!g.accept(3));
        assert!(g.accept(6));
        assert!(!g.accept(6));
        assert_eq!(g.min_version(), 6);
    }

    #[test]
    fn tamper_seal_detects_drift() {
        let region = [0xAAu8; 128];
        let seal = TamperSeal::seal(&region);
        assert!(seal.intact(&region));
        let mut tampered = region;
        tampered[64] ^= 1;
        assert!(!seal.intact(&tampered));
    }

    #[test]
    fn audit_log_chain_is_tamper_evident() {
        let key = [7u8; 32];
        let mut log = AuditLog::new(&key);
        let t1 = log.append(&key, b"boot");
        let t2 = log.append(&key, b"login user=admin");
        assert_ne!(t1, t2);
        assert_eq!(log.count(), 2);
        // an independent verifier replays the same events and must reach the same head
        let mut verify = AuditLog::new(&key);
        verify.append(&key, b"boot");
        verify.append(&key, b"login user=admin");
        assert_eq!(verify.head(), log.head());
        // dropping the first event yields a different head (deletion detected)
        let mut dropped = AuditLog::new(&key);
        dropped.append(&key, b"login user=admin");
        assert_ne!(dropped.head(), log.head());
    }

    #[test]
    fn config_store_verifies_integrity() {
        let key = [3u8; 32];
        let mut cfg = ConfigStore::<32>::empty();
        assert!(cfg.store(&key, 2, b"rate=100;mode=turbo"));
        let (v, data) = cfg.load(&key).unwrap();
        assert_eq!(v, 2);
        assert_eq!(data, b"rate=100;mode=turbo");
        // wrong key -> integrity fails
        assert!(cfg.load(&[0u8; 32]).is_none());
    }
}
