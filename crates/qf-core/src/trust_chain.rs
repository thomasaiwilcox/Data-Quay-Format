//! Quay Format (QF) v1.0 — Trust chain (Spec §63).
//!
//! Each QF-O record can carry a `trust_hash` that chains to its predecessor
//! by hashing canonical *logical* values (Spec §17), not FileCodes. This
//! lets a re-encoded file with reassigned FileCodes preserve the chain
//! intact. Redactions deliberately break the chain (Spec §63.5).

use crate::QfError;

#[cfg(feature = "digest-sha2")]
use sha2::Digest as _;

/// Compute a 32-byte trust hash that chains `prev_hash` with `payload`. The
/// payload MUST be the canonical bytes of the record's logical values.
///
/// `payload` typically concatenates canonically-encoded scalars in column
/// order (Spec §17, §63.3).
#[cfg(feature = "digest-sha2")]
pub fn chain(prev_hash: &[u8; 32], payload: &[u8]) -> [u8; 32] {
    let mut h = sha2::Sha256::new();
    h.update(prev_hash);
    h.update(payload);
    h.finalize().into()
}

/// Stub used when the SHA-256 backend is not compiled in.
#[cfg(not(feature = "digest-sha2"))]
pub fn chain(_prev_hash: &[u8; 32], _payload: &[u8]) -> [u8; 32] {
    [0u8; 32]
}

/// Verify a trust chain over a sequence of (payload, expected_hash) pairs.
/// The first payload chains from `genesis_hash` (typically all zeros).
pub fn verify_chain(genesis_hash: [u8; 32], records: &[(&[u8], [u8; 32])]) -> Result<(), QfError> {
    let mut prev = genesis_hash;
    for (payload, expected) in records {
        let computed = chain(&prev, payload);
        if &computed != expected {
            return Err(QfError::DigestMismatch);
        }
        prev = computed;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "digest-sha2")]
    #[test]
    fn chain_round_trip() {
        let genesis = [0u8; 32];
        let h1 = chain(&genesis, b"first");
        let h2 = chain(&h1, b"second");
        assert!(verify_chain(genesis, &[(b"first", h1), (b"second", h2)]).is_ok());
    }

    #[cfg(feature = "digest-sha2")]
    #[test]
    fn chain_break_rejected() {
        let genesis = [0u8; 32];
        let h1 = chain(&genesis, b"first");
        // wrong second hash
        assert_eq!(
            verify_chain(genesis, &[(b"first", h1), (b"second", [0u8; 32])]),
            Err(QfError::DigestMismatch)
        );
    }
}
