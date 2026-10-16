//! Stable cryptographic digest utilities using SHA-256.
//!
//! Provides deterministic hex-encoded SHA-256 digests across process boundaries,
//! replacing unstable non-cryptographic hashers (e.g. `DefaultHasher`).

use sha2::{Digest as Sha2Digest, Sha256};

/// Computes a lowercase 64-character hex-encoded SHA-256 digest of `bytes`.
pub fn sha256_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let result = hasher.finalize();
    to_hex(&result)
}

/// Helper to format raw bytes as lowercase hex.
pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_known_sha256_test_vectors() {
        // Standard NIST / RFC 6234 test vectors
        assert_eq!(
            sha256_digest(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_digest(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_digest(b"hello world"),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_sha256_length_and_case() {
        let digest = sha256_digest(b"relay integrity check");
        assert_eq!(digest.len(), 64);
        assert!(
            digest
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn test_deterministic_stability() {
        let payload = b"canonical-test-data-for-relay";
        let d1 = sha256_digest(payload);
        let d2 = sha256_digest(payload);
        assert_eq!(d1, d2);
    }
}
