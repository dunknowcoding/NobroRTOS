//! Security + data-integrity primitives (M174/M176/M177/M179/M180/M185).
#![cfg_attr(not(test), no_std)]

use nobro_crypto::sha256::{hmac_sha256, sha256, Sha256};

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
        Self {
            ids: [0; N],
            keys: [[0; 32]; N],
            used: [false; N],
        }
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
        (0..N)
            .find(|&i| self.used[i] && self.ids[i] == id)
            .map(|i| &self.keys[i])
    }

    /// Remove and zeroize a provisioned RAM key.
    pub fn deprovision(&mut self, id: u32) -> bool {
        let Some(i) = (0..N).find(|&i| self.used[i] && self.ids[i] == id) else {
            return false;
        };
        self.keys[i].fill(0);
        self.ids[i] = 0;
        self.used[i] = false;
        true
    }
}

/// OTA rollback protection (M177): accept an image only if its version is strictly
/// greater than the highest ever installed (monotonic anti-rollback counter).
pub struct RollbackGuard {
    min_version: u32,
}

impl RollbackGuard {
    pub const fn new(current_version: u32) -> Self {
        Self {
            min_version: current_version,
        }
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
        Self {
            baseline: sha256(region),
        }
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
        Self {
            prev_tag: hmac_sha256(key, b"genesis"),
            count: 0,
        }
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
    fn authenticated_tag(key: &[u8; 32], version: u32, data: &[u8]) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(b"NobroRTOS ConfigStore v1");
        hash.update(&version.to_le_bytes());
        hash.update(&(data.len() as u64).to_le_bytes());
        hash.update(data);
        hmac_sha256(key, &hash.finalize())
    }

    pub const fn empty() -> Self {
        Self {
            version: 0,
            len: 0,
            bytes: [0; N],
            tag: [0; 32],
        }
    }
    pub fn store(&mut self, key: &[u8; 32], version: u32, data: &[u8]) -> bool {
        if data.len() > N {
            return false;
        }
        self.version = version;
        self.len = data.len();
        self.bytes.fill(0);
        self.bytes[..data.len()].copy_from_slice(data);
        self.tag = Self::authenticated_tag(key, version, data);
        true
    }
    /// Return the config bytes only if the integrity tag still verifies.
    pub fn load(&self, key: &[u8; 32]) -> Option<(u32, &[u8])> {
        if self.len > N {
            return None;
        }
        let expect = Self::authenticated_tag(key, self.version, &self.bytes[..self.len]);
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
        assert!(ks.deprovision(2));
        assert_eq!(ks.get(2), None);
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

        cfg.version ^= 1;
        assert!(cfg.load(&key).is_none());
        cfg.version ^= 1;
        cfg.len -= 1;
        assert!(cfg.load(&key).is_none());
    }
}

/// OTA A/B partition agent (M183): two firmware slots; installs new images into the
/// inactive slot, only boots a slot whose version passes anti-rollback, and can revert to
/// the last-known-good slot. Bounded state, no heap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    A,
    B,
}

#[derive(Clone, Copy, Debug)]
pub struct OtaAgent {
    active: Slot,
    version: [u32; 2], // installed version per slot (index 0=A,1=B)
    good: [bool; 2],   // slot confirmed-good (booted successfully)
    min_version: u32,  // anti-rollback floor
    staged: Option<Slot>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OtaError {
    NoStagedImage,
    InvalidStagedVersion,
}

impl OtaAgent {
    pub const fn new(active: Slot, active_version: u32) -> Self {
        let mut version = [0u32; 2];
        version[Self::idx(active)] = active_version;
        let mut good = [false; 2];
        good[Self::idx(active)] = true;
        Self {
            active,
            version,
            good,
            min_version: active_version,
            staged: None,
        }
    }

    const fn idx(s: Slot) -> usize {
        match s {
            Slot::A => 0,
            Slot::B => 1,
        }
    }

    pub fn active(&self) -> Slot {
        self.active
    }

    fn inactive(&self) -> Slot {
        match self.active {
            Slot::A => Slot::B,
            Slot::B => Slot::A,
        }
    }

    /// Stage an update into the INACTIVE slot; rejected if it does not beat the
    /// anti-rollback floor. Returns the slot it was written to on success.
    pub fn stage(&mut self, candidate_version: u32) -> Option<Slot> {
        if candidate_version <= self.min_version {
            return None;
        }
        let slot = self.inactive();
        self.version[Self::idx(slot)] = candidate_version;
        self.good[Self::idx(slot)] = false; // unproven until it boots + confirms
        self.staged = Some(slot);
        Some(slot)
    }

    /// Boot into the staged slot (call after a reset into it). Does not yet confirm-good.
    pub fn boot_staged(&mut self) -> Result<Slot, OtaError> {
        let slot = self.staged.ok_or(OtaError::NoStagedImage)?;
        let version = self.version[Self::idx(slot)];
        if version <= self.min_version {
            return Err(OtaError::InvalidStagedVersion);
        }
        self.active = slot;
        self.min_version = version;
        self.staged = None;
        Ok(self.active)
    }

    /// The freshly-booted slot confirmed healthy (watchdog fed, self-test passed).
    pub fn confirm(&mut self) {
        self.good[Self::idx(self.active)] = true;
    }

    /// Boot failed to confirm: revert to the other slot if it is known-good.
    pub fn revert(&mut self) -> Slot {
        if self.good[Self::idx(self.active)] {
            return self.active;
        }
        let other = self.inactive();
        if self.good[Self::idx(other)] {
            self.active = other;
        }
        self.active
    }
}

// ---------------------------------------------------------------- secure boot (M173)

/// The verdict from checking a firmware image before it is allowed to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootVerdict {
    /// Measurement, signature, and version all check out - safe to boot.
    Accept,
    /// The image bytes do not match the signed measurement (tampered).
    RejectTampered,
    /// The signature does not verify under the boot key (forged / wrong key).
    RejectSignature,
    /// The image version is below the anti-rollback floor.
    RejectRollback,
}

/// Static image metadata a bootloader validates before jumping to an app image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootImageManifest {
    pub version: u32,
    pub image_len: u32,
    pub load_addr: u32,
    pub entry_addr: u32,
    pub stack_top: u32,
    pub measurement: [u8; 32],
    pub signature: [u8; 32],
}

/// Address policy for a board bootloader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootVectorPolicy {
    pub min_load_addr: u32,
    pub max_end_addr: u32,
    pub min_stack_addr: u32,
    pub max_stack_addr: u32,
    pub stack_alignment: u32,
    pub require_thumb_entry: bool,
}

impl BootVectorPolicy {
    pub const fn cortex_m(
        min_load_addr: u32,
        max_end_addr: u32,
        min_stack_addr: u32,
        max_stack_addr: u32,
    ) -> Self {
        Self {
            min_load_addr,
            max_end_addr,
            min_stack_addr,
            max_stack_addr,
            stack_alignment: 8,
            require_thumb_entry: true,
        }
    }
}

/// A verified target that a board-specific bootloader may jump to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerifiedBootPlan {
    pub version: u32,
    pub image_len: u32,
    pub load_addr: u32,
    pub entry_addr: u32,
    pub stack_top: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootPlanError {
    Verification(BootVerdict),
    SizeMismatch,
    AddressRange,
    InvalidEntry,
    InvalidStack,
}

/// Secure-boot verification (M173): gate a firmware image on a signature over its
/// SHA-256 measurement plus a monotonic version, using our own HMAC-SHA256 - no vendor
/// secure-boot infra. The signing authority holds the boot key and emits
/// `sig = HMAC(boot_key, sha256(image) || version_le)`; the device (sharing the key via
/// [`KeyStore`]) recomputes and refuses to run an image that is tampered, forged, or
/// rolled back. This is the verification core; the jump-to-image step is a bootloader's
/// job (out of scope for a probe-less bench, but the security decision lives here).
pub struct SecureBoot {
    min_version: u32,
}

impl SecureBoot {
    pub const fn new(min_version: u32) -> Self {
        SecureBoot { min_version }
    }

    /// The measurement (SHA-256) an authority signs and a device recomputes.
    pub fn measure(image: &[u8]) -> [u8; 32] {
        sha256(image)
    }

    /// Authority-side: sign `measurement || version` with the boot key.
    pub fn sign(boot_key: &[u8; 32], measurement: &[u8; 32], version: u32) -> [u8; 32] {
        let mut msg = [0u8; 36];
        msg[..32].copy_from_slice(measurement);
        msg[32..36].copy_from_slice(&version.to_le_bytes());
        hmac_sha256(boot_key, &msg)
    }

    /// Device-side: fully verify an image before boot. Checks the measurement matches
    /// (implicit in re-signing over the recomputed hash), the signature, and rollback.
    pub fn verify(
        &self,
        boot_key: &[u8; 32],
        image: &[u8],
        version: u32,
        signed_measurement: &[u8; 32],
        sig: &[u8; 32],
    ) -> BootVerdict {
        // 1. the image must hash to the measurement the signature covers
        let actual = Self::measure(image);
        if !verify_tag(&actual, signed_measurement) {
            return BootVerdict::RejectTampered;
        }
        // 2. the signature must verify under the boot key
        let expect = Self::sign(boot_key, signed_measurement, version);
        if !verify_tag(&expect, sig) {
            return BootVerdict::RejectSignature;
        }
        // 3. anti-rollback: version must be at or above the floor
        if version < self.min_version {
            return BootVerdict::RejectRollback;
        }
        BootVerdict::Accept
    }

    /// Advance the rollback floor after a verified image is committed as the new active
    /// firmware (so an older signed image can no longer be booted).
    pub fn commit(&mut self, version: u32) {
        if version > self.min_version {
            self.min_version = version;
        }
    }

    pub fn min_version(&self) -> u32 {
        self.min_version
    }

    /// Verify a signed image manifest and return the exact target a bootloader may run.
    ///
    /// This keeps policy, anti-rollback, measurement, and vector sanity checks in one
    /// testable contract. The final CPU-specific jump remains outside this safe API.
    pub fn boot_plan(
        &self,
        boot_key: &[u8; 32],
        image: &[u8],
        manifest: &BootImageManifest,
        policy: BootVectorPolicy,
    ) -> Result<VerifiedBootPlan, BootPlanError> {
        if image.len() != manifest.image_len as usize {
            return Err(BootPlanError::SizeMismatch);
        }
        let end = manifest
            .load_addr
            .checked_add(manifest.image_len)
            .ok_or(BootPlanError::AddressRange)?;
        if manifest.load_addr < policy.min_load_addr || end > policy.max_end_addr {
            return Err(BootPlanError::AddressRange);
        }
        let entry_code_addr = if policy.require_thumb_entry {
            if manifest.entry_addr & 1 == 0 {
                return Err(BootPlanError::InvalidEntry);
            }
            manifest.entry_addr & !1
        } else {
            manifest.entry_addr
        };
        if entry_code_addr < manifest.load_addr || entry_code_addr >= end {
            return Err(BootPlanError::InvalidEntry);
        }
        if manifest.stack_top == 0
            || policy.stack_alignment == 0
            || !manifest.stack_top.is_multiple_of(policy.stack_alignment)
            || manifest.stack_top <= policy.min_stack_addr
            || manifest.stack_top > policy.max_stack_addr
        {
            return Err(BootPlanError::InvalidStack);
        }
        let verdict = self.verify(
            boot_key,
            image,
            manifest.version,
            &manifest.measurement,
            &manifest.signature,
        );
        if verdict != BootVerdict::Accept {
            return Err(BootPlanError::Verification(verdict));
        }
        Ok(VerifiedBootPlan {
            version: manifest.version,
            image_len: manifest.image_len,
            load_addr: manifest.load_addr,
            entry_addr: manifest.entry_addr,
            stack_top: manifest.stack_top,
        })
    }
}

#[cfg(test)]
mod secure_boot_tests {
    use super::*;

    const BOOT_KEY: [u8; 32] = [0x5A; 32];
    // pinned + mirrored in tools/sign_firmware.py so host and device signers agree
    const PINNED_SIG4: [u8; 4] = [0xBB, 0x49, 0x2F, 0x39];

    #[test]
    fn accepts_a_correctly_signed_image() {
        let image = b"NOBRO firmware v2 payload bytes....";
        let m = SecureBoot::measure(image);
        let sig = SecureBoot::sign(&BOOT_KEY, &m, 2);
        let sb = SecureBoot::new(1);
        assert_eq!(
            sb.verify(&BOOT_KEY, image, 2, &m, &sig),
            BootVerdict::Accept
        );
    }

    #[test]
    fn rejects_a_tampered_image() {
        let image = b"NOBRO firmware v2 payload bytes....";
        let m = SecureBoot::measure(image);
        let sig = SecureBoot::sign(&BOOT_KEY, &m, 2);
        let sb = SecureBoot::new(1);
        let tampered = b"NOBRO firmware v2 payload byteXX...."; // one byte changed
        assert_eq!(
            sb.verify(&BOOT_KEY, tampered, 2, &m, &sig),
            BootVerdict::RejectTampered
        );
    }

    #[test]
    fn rejects_a_forged_signature_or_wrong_key() {
        let image = b"payload";
        let m = SecureBoot::measure(image);
        let sig = SecureBoot::sign(&BOOT_KEY, &m, 3);
        let sb = SecureBoot::new(1);
        let attacker_key = [0x11u8; 32];
        assert_eq!(
            sb.verify(&attacker_key, image, 3, &m, &sig),
            BootVerdict::RejectSignature
        );
        let mut bad_sig = sig;
        bad_sig[0] ^= 1;
        assert_eq!(
            sb.verify(&BOOT_KEY, image, 3, &m, &bad_sig),
            BootVerdict::RejectSignature
        );
    }

    #[test]
    fn enforces_anti_rollback() {
        let image = b"old firmware";
        let m = SecureBoot::measure(image);
        let sig = SecureBoot::sign(&BOOT_KEY, &m, 2); // validly signed, but old
        let mut sb = SecureBoot::new(1);
        sb.commit(5); // we are now running v5
        assert_eq!(sb.min_version(), 5);
        assert_eq!(
            sb.verify(&BOOT_KEY, image, 2, &m, &sig),
            BootVerdict::RejectRollback
        );
    }

    #[test]
    fn sign_matches_a_pinned_vector_for_host_parity() {
        // The host signer (tools/sign_firmware.py) pins the same vector so the two sides
        // agree byte-for-byte; a divergence breaks a build, not a deployment.
        let m = SecureBoot::measure(b"nobro");
        let sig = SecureBoot::sign(&[0x5A; 32], &m, 1);
        assert_eq!(&sig[..4], &PINNED_SIG4);
    }

    #[test]
    fn verified_manifest_produces_a_boot_plan() {
        let image = b"NOBRO signed app image";
        let measurement = SecureBoot::measure(image);
        let signature = SecureBoot::sign(&BOOT_KEY, &measurement, 8);
        let manifest = BootImageManifest {
            version: 8,
            image_len: image.len() as u32,
            load_addr: 0x1000,
            entry_addr: 0x1001,
            stack_top: 0x2001_0000,
            measurement,
            signature,
        };
        let plan = SecureBoot::new(7)
            .boot_plan(
                &BOOT_KEY,
                image,
                &manifest,
                BootVectorPolicy::cortex_m(0x1000, 0x80000, 0x2000_0000, 0x2004_0000),
            )
            .unwrap();
        assert_eq!(plan.entry_addr, 0x1001);
        assert_eq!(plan.version, 8);
    }

    #[test]
    fn boot_plan_rejects_bad_size_vector_or_signature() {
        let image = b"NOBRO signed app image";
        let measurement = SecureBoot::measure(image);
        let signature = SecureBoot::sign(&BOOT_KEY, &measurement, 8);
        let mut manifest = BootImageManifest {
            version: 8,
            image_len: image.len() as u32,
            load_addr: 0x1000,
            entry_addr: 0x1001,
            stack_top: 0x2001_0000,
            measurement,
            signature,
        };
        let sb = SecureBoot::new(7);
        let policy = BootVectorPolicy::cortex_m(0x1000, 0x80000, 0x2000_0000, 0x2004_0000);

        manifest.image_len += 1;
        assert_eq!(
            sb.boot_plan(&BOOT_KEY, image, &manifest, policy),
            Err(BootPlanError::SizeMismatch)
        );
        manifest.image_len = image.len() as u32;
        manifest.entry_addr = 0x1000;
        assert_eq!(
            sb.boot_plan(&BOOT_KEY, image, &manifest, policy),
            Err(BootPlanError::InvalidEntry)
        );
        manifest.entry_addr = 0x1001;
        manifest.signature[0] ^= 1;
        assert_eq!(
            sb.boot_plan(&BOOT_KEY, image, &manifest, policy),
            Err(BootPlanError::Verification(BootVerdict::RejectSignature))
        );
    }
}

#[cfg(test)]
mod ota_tests {
    use super::*;

    #[test]
    fn ota_ab_update_confirm_and_revert() {
        // boot A@5
        let mut ota = OtaAgent::new(Slot::A, 5);
        // stage v6 into B, boot it
        assert_eq!(ota.stage(6), Some(Slot::B));
        assert_eq!(ota.stage(4), None); // rollback rejected
        assert_eq!(ota.boot_staged(), Ok(Slot::B));
        // if B never confirms, revert to the still-good A
        assert_eq!(ota.revert(), Slot::A);
        // now do a good update: stage v7 into B, boot + confirm
        assert_eq!(ota.stage(7), Some(Slot::B));
        assert_eq!(ota.boot_staged(), Ok(Slot::B));
        ota.confirm();
        // a later revert stays on B (A is older but still good) - active unchanged when
        // current slot is confirmed
        assert_eq!(ota.active(), Slot::B);
    }
}
