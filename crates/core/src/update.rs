//! Signed updates: what the release tool signs and the app verifies.
//!
//! A release is `RAMOptimizer.exe` plus `RAMOptimizer.exe.sig`. The signature
//! is made with a private key that never leaves the developer's machine and
//! covers both the version and the SHA-256 of the executable, so neither a
//! tampered file nor an old build relabeled as new will be accepted.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

/// The exact bytes that get signed for a release.
pub fn signed_message(version: &str, exe: &[u8]) -> Vec<u8> {
    format!("ram-optimizer {version}\n{}", to_hex(&Sha256::digest(exe))).into_bytes()
}

/// Signs a release; returns the signature as hex (the `.sig` file's content).
pub fn sign(secret_hex: &str, version: &str, exe: &[u8]) -> Result<String, String> {
    let key = SigningKey::from_bytes(&key_bytes(secret_hex)?);
    Ok(to_hex(&key.sign(&signed_message(version, exe)).to_bytes()))
}

/// The public key (hex) belonging to a private key.
pub fn public_key(secret_hex: &str) -> Result<String, String> {
    let key = SigningKey::from_bytes(&key_bytes(secret_hex)?);
    Ok(to_hex(key.verifying_key().as_bytes()))
}

/// Checks a downloaded release against the public key built into the app.
pub fn verify(public_hex: &str, version: &str, exe: &[u8], signature_hex: &str) -> Result<(), String> {
    let key = VerifyingKey::from_bytes(&key_bytes(public_hex)?).map_err(|_| "Bad update key")?;
    let signature: [u8; 64] = from_hex(signature_hex.trim())?
        .try_into()
        .map_err(|_| "Bad signature length")?;
    key.verify_strict(&signed_message(version, exe), &Signature::from_bytes(&signature))
        .map_err(|_| "Update signature doesn't match".to_string())
}

/// Whether `candidate` (like "0.3.0") is a newer version than `current`.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let mut parts = text.trim().trim_start_matches('v').split('.').map(|p| p.parse::<u32>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

fn key_bytes(hex: &str) -> Result<[u8; 32], String> {
    from_hex(hex.trim())?.try_into().map_err(|_| "Keys are 32 bytes".to_string())
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn from_hex(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) {
        return Err("Odd-length hex".into());
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|_| "Invalid hex".to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";

    #[test]
    fn signed_release_verifies() {
        let public = public_key(SECRET).unwrap();
        let signature = sign(SECRET, "0.2.0", b"exe bytes").unwrap();
        assert_eq!(verify(&public, "0.2.0", b"exe bytes", &signature), Ok(()));
    }

    #[test]
    fn rejects_tampering() {
        let public = public_key(SECRET).unwrap();
        let signature = sign(SECRET, "0.2.0", b"exe bytes").unwrap();
        assert!(verify(&public, "0.2.0", b"evil bytes", &signature).is_err());
        // An old build relabeled as a newer version.
        assert!(verify(&public, "0.3.0", b"exe bytes", &signature).is_err());
        assert!(verify(&public, "0.2.0", b"exe bytes", "00").is_err());
    }

    #[test]
    fn compares_versions() {
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }
}
