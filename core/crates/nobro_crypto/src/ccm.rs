//! AES-CCM authenticated encryption: CTR encryption + CBC-MAC over the in-tree
//! AES-128, with a 13-byte nonce, 2-byte length field (L=2) and an 8-byte tag (M=8) -
//! the RFC 3610 parameterization used by 802.15.4/Zigbee-class mesh security. Verified
//! against the RFC 3610 Packet Vector #1.

use crate::Aes128;

pub const NONCE_LEN: usize = 13;
pub const TAG_LEN: usize = 8;
const L: usize = 2; // length-field bytes; nonce is 15 - L = 13

/// Errors from CCM operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CcmError {
    /// Output buffer too small, or payload longer than the length field can carry.
    BadLength,
    /// Tag verification failed on decrypt (tampered or wrong key/nonce).
    BadTag,
}

fn xor_into(dst: &mut [u8; 16], src: &[u8]) {
    for (d, s) in dst.iter_mut().zip(src) {
        *d ^= s;
    }
}

/// CBC-MAC over B0 | AAD blocks | payload blocks, per RFC 3610.
fn cbc_mac(aes: &Aes128, nonce: &[u8; NONCE_LEN], aad: &[u8], payload: &[u8]) -> [u8; 16] {
    // B0: flags | nonce | l(m). flags = 64*Adata + 8*M' + L' (M'=(M-2)/2, L'=L-1).
    let adata = u8::from(!aad.is_empty());
    let flags = 64 * adata + 8 * (((TAG_LEN - 2) / 2) as u8) + (L - 1) as u8;
    let mut b0 = [0u8; 16];
    b0[0] = flags;
    b0[1..1 + NONCE_LEN].copy_from_slice(nonce);
    b0[14] = (payload.len() >> 8) as u8;
    b0[15] = payload.len() as u8;
    let mut x = aes.encrypt_block(&b0);

    // AAD: 2-byte length prefix, then the data, zero-padded to block boundaries.
    if !aad.is_empty() {
        let mut block = [0u8; 16];
        block[0] = (aad.len() >> 8) as u8;
        block[1] = aad.len() as u8;
        let take = aad.len().min(14);
        block[2..2 + take].copy_from_slice(&aad[..take]);
        xor_into(&mut block, &x);
        x = aes.encrypt_block(&block);
        let mut off = take;
        while off < aad.len() {
            let mut block = [0u8; 16];
            let take = (aad.len() - off).min(16);
            block[..take].copy_from_slice(&aad[off..off + take]);
            xor_into(&mut block, &x);
            x = aes.encrypt_block(&block);
            off += take;
        }
    }

    // Payload blocks, zero-padded.
    let mut off = 0;
    while off < payload.len() {
        let mut block = [0u8; 16];
        let take = (payload.len() - off).min(16);
        block[..take].copy_from_slice(&payload[off..off + take]);
        xor_into(&mut block, &x);
        x = aes.encrypt_block(&block);
        off += take;
    }
    x
}

/// Keystream block S_i: AES(flags | nonce | counter i).
fn s_block(aes: &Aes128, nonce: &[u8; NONCE_LEN], i: u16) -> [u8; 16] {
    let mut a = [0u8; 16];
    a[0] = (L - 1) as u8;
    a[1..1 + NONCE_LEN].copy_from_slice(nonce);
    a[14] = (i >> 8) as u8;
    a[15] = i as u8;
    aes.encrypt_block(&a)
}

/// Encrypt `payload` in place semantics: writes ciphertext into `out[..payload.len()]`
/// and the 8-byte tag into `out[payload.len()..payload.len()+8]`. Returns total length.
pub fn encrypt(
    key: &[u8; 16],
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    payload: &[u8],
    out: &mut [u8],
) -> Result<usize, CcmError> {
    if out.len() < payload.len() + TAG_LEN || payload.len() > u16::MAX as usize {
        return Err(CcmError::BadLength);
    }
    let aes = Aes128::new(key);
    let mac = cbc_mac(&aes, nonce, aad, payload);

    // CTR-encrypt payload with S_1..; mask the MAC with S_0 for the tag.
    for (i, chunk) in payload.chunks(16).enumerate() {
        let s = s_block(&aes, nonce, (i + 1) as u16);
        for (j, &p) in chunk.iter().enumerate() {
            out[i * 16 + j] = p ^ s[j];
        }
    }
    let s0 = s_block(&aes, nonce, 0);
    for j in 0..TAG_LEN {
        out[payload.len() + j] = mac[j] ^ s0[j];
    }
    Ok(payload.len() + TAG_LEN)
}

/// Decrypt-and-verify: `input` = ciphertext | 8-byte tag. Writes the plaintext into
/// `out` and returns its length; `BadTag` if authentication fails (out is zeroed then).
pub fn decrypt(
    key: &[u8; 16],
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    input: &[u8],
    out: &mut [u8],
) -> Result<usize, CcmError> {
    if input.len() < TAG_LEN || out.len() < input.len() - TAG_LEN {
        return Err(CcmError::BadLength);
    }
    let n = input.len() - TAG_LEN;
    let aes = Aes128::new(key);

    for (i, chunk) in input[..n].chunks(16).enumerate() {
        let s = s_block(&aes, nonce, (i + 1) as u16);
        for (j, &c) in chunk.iter().enumerate() {
            out[i * 16 + j] = c ^ s[j];
        }
    }
    let mac = cbc_mac(&aes, nonce, aad, &out[..n]);
    let s0 = s_block(&aes, nonce, 0);
    let mut diff = 0u8;
    for j in 0..TAG_LEN {
        diff |= input[n + j] ^ (mac[j] ^ s0[j]); // constant-time compare
    }
    if diff != 0 {
        out[..n].fill(0);
        return Err(CcmError::BadTag);
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 3610 Packet Vector #1: AES key, 13-byte nonce, 8-byte AAD, 23-byte payload.
    const KEY: [u8; 16] = [
        0xC0, 0xC1, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xCB, 0xCC, 0xCD, 0xCE,
        0xCF,
    ];
    const NONCE: [u8; 13] = [
        0x00, 0x00, 0x00, 0x03, 0x02, 0x01, 0x00, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5,
    ];
    const AAD: [u8; 8] = [0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
    const PAYLOAD: [u8; 23] = [
        0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16,
        0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E,
    ];
    const EXPECT: [u8; 31] = [
        0x58, 0x8C, 0x97, 0x9A, 0x61, 0xC6, 0x63, 0xD2, 0xF0, 0x66, 0xD0, 0xC2, 0xC0, 0xF9, 0x89,
        0x80, 0x6D, 0x5F, 0x6B, 0x61, 0xDA, 0xC3, 0x84, 0x17, 0xE8, 0xD1, 0x2C, 0xFD, 0xF9, 0x26,
        0xE0,
    ];

    #[test]
    fn matches_rfc3610_packet_vector_1() {
        let mut out = [0u8; 31];
        let n = encrypt(&KEY, &NONCE, &AAD, &PAYLOAD, &mut out).unwrap();
        assert_eq!(n, 31);
        assert_eq!(out, EXPECT);
    }

    #[test]
    fn decrypt_roundtrip_and_tamper_detection() {
        let mut ct = [0u8; 31];
        encrypt(&KEY, &NONCE, &AAD, &PAYLOAD, &mut ct).unwrap();
        let mut pt = [0u8; 23];
        assert_eq!(decrypt(&KEY, &NONCE, &AAD, &ct, &mut pt), Ok(23));
        assert_eq!(pt, PAYLOAD);
        // flip one ciphertext bit -> BadTag and zeroed output
        let mut bad = ct;
        bad[5] ^= 0x01;
        assert_eq!(
            decrypt(&KEY, &NONCE, &AAD, &bad, &mut pt),
            Err(CcmError::BadTag)
        );
        assert!(pt.iter().all(|&b| b == 0));
        // wrong AAD -> BadTag
        assert_eq!(
            decrypt(&KEY, &NONCE, &[0xFF; 8], &ct, &mut pt),
            Err(CcmError::BadTag)
        );
    }
}
