//! Cryptographic utilities for near-forms WASI module
//!
//! Implements private key derivation and form submission decryption.
//!
//! Uses pure Rust crypto libraries for WASI compatibility:
//! - libsecp256k1 (not secp256k1 which has C bindings)
//! - ECDH + ChaCha20-Poly1305 for encryption (EC01 format)

use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Nonce,
};
use hkdf::Hkdf;
use libsecp256k1::{PublicKey, SecretKey};
use sha2::{Digest, Sha256};

/// Magic bytes for ECDH + ChaCha20 format (current)
const ECDH_MAGIC: &[u8; 4] = b"EC01";

/// Domain separation prefix for key derivation
const DERIVATION_PREFIX: &[u8] = b"near-forms:v1:";

/// Parse a hex-encoded private key
pub fn parse_private_key(hex_str: &str) -> Result<SecretKey, Box<dyn std::error::Error>> {
    let bytes = hex::decode(hex_str)?;
    let privkey = SecretKey::parse_slice(&bytes)
        .map_err(|e| format!("Invalid private key: {:?}", e))?;
    Ok(privkey)
}

/// Derive a form-specific private key from master private key
///
/// Uses additive key derivation:
///   form_privkey = master_privkey + SHA256(prefix + form_id)
///
/// This must match the public key derivation used by WASI module's encrypt_for_form.
pub fn derive_form_privkey(
    master_privkey: &SecretKey,
    form_id: &str,
) -> Result<SecretKey, Box<dyn std::error::Error>> {
    // Create deterministic tweak from form_id
    let mut hasher = Sha256::new();
    hasher.update(DERIVATION_PREFIX);
    hasher.update(form_id.as_bytes());
    let tweak_bytes: [u8; 32] = hasher.finalize().into();

    // Convert tweak to SecretKey (which is a scalar)
    let tweak = SecretKey::parse_slice(&tweak_bytes)
        .map_err(|e| format!("Failed to create tweak: {:?}", e))?;

    // Add tweak to private key (scalar addition)
    let mut user_privkey = master_privkey.clone();
    user_privkey.tweak_add_assign(&tweak)
        .map_err(|e| format!("Failed to derive private key: {:?}", e))?;

    Ok(user_privkey)
}

/// Decrypt form submission data using EC01 format
///
/// Format:
/// - Magic: "EC01" (4 bytes)
/// - Ephemeral public key: 33 bytes (compressed)
/// - Nonce: 12 bytes
/// - ChaCha20-Poly1305 ciphertext + tag: remaining bytes
pub fn decrypt_blob(
    form_privkey: &SecretKey,
    encrypted: &[u8],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if encrypted.len() <= 4 || &encrypted[0..4] != ECDH_MAGIC {
        return Err("Invalid encryption format: expected EC01 magic bytes".into());
    }
    decrypt_ecdh(form_privkey, encrypted)
}

/// Decrypt data using ECDH + ChaCha20-Poly1305 (EC01 format)
///
/// Format: EC01 (4) || ephemeral_pubkey (33) || nonce (12) || ciphertext+tag
fn decrypt_ecdh(
    user_privkey: &SecretKey,
    encrypted: &[u8],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    const HEADER_SIZE: usize = 4;       // EC01
    const PUBKEY_SIZE: usize = 33;      // compressed pubkey
    const NONCE_SIZE: usize = 12;
    const TAG_SIZE: usize = 16;         // Poly1305 tag
    const MIN_SIZE: usize = HEADER_SIZE + PUBKEY_SIZE + NONCE_SIZE + TAG_SIZE;

    if encrypted.len() < MIN_SIZE {
        return Err(format!(
            "EC01 data too short: {} bytes, need at least {}",
            encrypted.len(), MIN_SIZE
        ).into());
    }

    // Parse ephemeral public key
    let ephemeral_pubkey_bytes = &encrypted[HEADER_SIZE..HEADER_SIZE + PUBKEY_SIZE];
    let mut shared_point = PublicKey::parse_slice(ephemeral_pubkey_bytes, None)
        .map_err(|e| format!("Invalid ephemeral pubkey: {:?}", e))?;

    // ECDH: shared_point = ephemeral_pubkey * user_privkey
    shared_point.tweak_mul_assign(user_privkey)
        .map_err(|e| format!("ECDH failed: {:?}", e))?;

    // Extract x-coordinate (skip prefix byte from compressed pubkey)
    let shared_compressed = shared_point.serialize_compressed();
    let shared_x = &shared_compressed[1..];

    // Derive key: HKDF-SHA256 with domain separation
    let hk = Hkdf::<Sha256>::new(None, shared_x);
    let mut key = [0u8; 32];
    hk.expand(b"near-forms:v1:ecdh", &mut key)
        .map_err(|_| "HKDF expand failed")?;

    // Extract nonce and ciphertext
    let nonce_start = HEADER_SIZE + PUBKEY_SIZE;
    let nonce_bytes = &encrypted[nonce_start..nonce_start + NONCE_SIZE];
    let ciphertext = &encrypted[nonce_start + NONCE_SIZE..];

    // Decrypt with ChaCha20-Poly1305
    let cipher = ChaCha20Poly1305::new_from_slice(&key)
        .map_err(|e| format!("Failed to create cipher: {:?}", e))?;
    let nonce = Nonce::from_slice(nonce_bytes);

    let decrypted = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| format!("ChaCha20-Poly1305 decryption failed: {:?}", e))?;

    Ok(decrypted)
}

