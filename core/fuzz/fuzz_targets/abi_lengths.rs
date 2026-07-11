#![no_main]
//! Cross-language ABI length fuzzing (Wave 43): hostile u32 lengths and
//! payloads through the boundaries where bytes cross between C modules,
//! devices, and the host — foreign-host quota/trace charging, authenticated
//! report envelopes, and signed-image manifests whose declared geometry
//! disagrees with reality. Invariants: no panic, no overflow, fail-closed
//! verdicts on every mismatch.

use libfuzzer_sys::fuzz_target;
use nobro_kernel::{
    Capability, CapabilitySet, CapabilityTraceOp, ForeignHostCall, ForeignHostContext,
    ForeignHostQuota, ModuleId, ModuleLaunchGate,
};
use nobro_secure::{AuthenticatedReportEnvelope, PinnedKeyPolicy, SignedImageManifest};

fn word(bytes: &[u8], index: usize) -> u32 {
    let mut out = [0u8; 4];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = bytes.get(index * 4 + i).copied().unwrap_or(0);
    }
    u32::from_le_bytes(out)
}

fuzz_target!(|data: &[u8]| {
    // 1. Foreign-host call charging with arbitrary lengths: saturating math,
    //    quota verdicts, and trace recording must hold for any u32.
    let gate = ModuleLaunchGate::new();
    let grants = CapabilitySet::empty().with(Capability::Bus0);
    gate.install(grants);
    let host = ForeignHostContext::<8>::new(&gate, ForeignHostQuota::new(3, 4096));
    for i in 0..4 {
        let len = word(data, i);
        let _ = host.invoke(
            ForeignHostCall::new(Capability::Bus0, CapabilityTraceOp::Write, u64::from(len))
                .args(len, len ^ 0xFFFF_FFFF)
                .bytes(len),
            || 0,
        );
    }
    let _ = host.usage();
    let _ = ModuleId::Sensor;

    // 2. Authenticated report envelope: arbitrary payload bytes and a sequence
    //    derived from the input must verify only for the exact sealed payload.
    let key = [0x42u8; 32];
    let sequence = word(data, 4);
    let envelope = AuthenticatedReportEnvelope::seal(&key, sequence, data);
    assert!(envelope.verify(&key, data));
    if !data.is_empty() {
        let mut mutated = data.to_vec();
        mutated[0] ^= 0x01;
        assert!(!envelope.verify(&key, &mutated));
    }

    // 3. Signed-image manifest whose lengths/geometry come from the fuzzer:
    //    verification must reject (never panic) on every mismatch.
    let mut measurement = [0u8; 32];
    let mut signature = [0u8; 64];
    for (i, slot) in measurement.iter_mut().enumerate() {
        *slot = data.get(i).copied().unwrap_or(0);
    }
    for (i, slot) in signature.iter_mut().enumerate() {
        *slot = data.get(32 + i).copied().unwrap_or(0);
    }
    let manifest = SignedImageManifest {
        key_id: word(data, 5),
        version: word(data, 6),
        image_len: word(data, 7),
        load_addr: word(data, 8),
        entry_addr: word(data, 9),
        stack_top: word(data, 10),
        measurement,
        signature,
    };
    let _ = manifest.signing_digest();
    let mut keys = PinnedKeyPolicy::<2>::new();
    let _ = keys.pin(manifest.key_id, [0x24; 32]);
    let vectors = nobro_secure::BootVectorPolicy {
        min_load_addr: 0x1000,
        max_end_addr: 0x0010_0000,
        require_thumb_entry: true,
        min_stack_addr: 0x2000_0000,
        max_stack_addr: 0x2004_0000,
        stack_alignment: 8,
    };
    // A fuzzer-constructed manifest must never verify (the signature is noise)
    // and must never panic while being rejected.
    assert!(nobro_secure::verify_signed_boot(data, &manifest, &keys, vectors, 0).is_err());
});
