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
use getrandom::getrandom;
use hkdf::Hkdf;
use libsecp256k1::{PublicKey, SecretKey};
use sha2::{Digest, Sha256};

/// Magic bytes for ECDH + ChaCha20 format (current)
const ECDH_MAGIC: &[u8; 4] = b"EC01";

/// Domain separation prefix for key derivation
const DERIVATION_PREFIX: &[u8] = b"near-forms:v1:";

/// secp256k1 curve order (big-endian)
const CURVE_ORDER: [u8; 32] = [
    0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFE,
    0xBA, 0xAE, 0xDC, 0xE6, 0xAF, 0x48, 0xA0, 0x3B,
    0xBF, 0xD2, 0x5E, 0x8C, 0xD0, 0x36, 0x41, 0x41,
];

/// Reduce a 32-byte big-endian value modulo the secp256k1 curve order.
/// Since 2^256 < 2n, at most one subtraction is needed.
/// This matches TypeScript's `bytesToBigInt(hash) % CURVE_ORDER` behavior.
fn reduce_mod_order(bytes: &[u8; 32]) -> [u8; 32] {
    // Compare bytes >= CURVE_ORDER (big-endian)
    let mut gte = true;
    for i in 0..32 {
        if bytes[i] < CURVE_ORDER[i] {
            gte = false;
            break;
        }
        if bytes[i] > CURVE_ORDER[i] {
            break;
        }
    }

    if !gte {
        return *bytes;
    }

    // Subtract CURVE_ORDER (single subtraction suffices since 2^256 < 2n)
    let mut result = [0u8; 32];
    let mut borrow: i16 = 0;
    for i in (0..32).rev() {
        let diff = (bytes[i] as i16) - (CURVE_ORDER[i] as i16) - borrow;
        if diff < 0 {
            result[i] = (diff + 256) as u8;
            borrow = 1;
        } else {
            result[i] = diff as u8;
            borrow = 0;
        }
    }

    result
}

/// Parse a hex-encoded private key.
/// Trims whitespace to handle trailing newlines from Docker secrets, k8s ConfigMaps, or copy-paste.
pub fn parse_private_key(hex_str: &str) -> Result<SecretKey, Box<dyn std::error::Error>> {
    let trimmed = hex_str.trim();
    if trimmed.len() != 64 {
        eprintln!(
            "PROTECTED_MASTER_KEY: expected 64 hex chars, got {} (raw input was {} chars)",
            trimmed.len(),
            hex_str.len()
        );
        return Err(format!(
            "Private key must be exactly 64 hex characters (32 bytes), got {}",
            trimmed.len()
        ).into());
    }
    let bytes = hex::decode(trimmed)?;
    let privkey = SecretKey::parse_slice(&bytes)
        .map_err(|e| format!("Invalid private key: {}", e))?;
    Ok(privkey)
}

/// Derive a form-specific private key from master private key
///
/// Uses additive key derivation:
///   form_privkey = master_privkey + SHA256(prefix + form_id)
///
/// This must match the public key derivation in web-ui's `deriveFormPublicKey()`.
pub fn derive_form_privkey(
    master_privkey: &SecretKey,
    form_id: &str,
) -> Result<SecretKey, Box<dyn std::error::Error>> {
    // Create deterministic tweak from form_id
    let mut hasher = Sha256::new();
    hasher.update(DERIVATION_PREFIX);
    hasher.update(form_id.as_bytes());
    let tweak_bytes: [u8; 32] = hasher.finalize().into();

    // Reduce mod curve order to match TypeScript's `bytesToBigInt(hash) % CURVE_ORDER`.
    // This differs from near-email which passes raw SHA256 output to SecretKey::parse_slice
    // (which rejects values >= curve order). Our approach ensures Rust and TypeScript derive
    // identical keys for all inputs, not just the ~(1 - 2^-128) that happen to be in range.
    //
    // Note: if the reduced tweak is zero (~2^-256 probability), both Rust
    // (SecretKey::parse_slice rejects zero) and TypeScript (explicit zero check)
    // would return an error. The probability is astronomically small.
    let reduced = reduce_mod_order(&tweak_bytes);

    // Convert tweak to SecretKey (which is a scalar)
    let tweak = SecretKey::parse_slice(&reduced)
        .map_err(|e| format!("Failed to create tweak: {}", e))?;

    // Add tweak to private key (scalar addition)
    let mut user_privkey = *master_privkey;
    user_privkey.tweak_add_assign(&tweak)
        .map_err(|e| format!("Failed to derive private key: {}", e))?;

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
        .map_err(|e| format!("Invalid ephemeral pubkey: {}", e))?;

    // ECDH: shared_point = ephemeral_pubkey * user_privkey
    shared_point.tweak_mul_assign(user_privkey)
        .map_err(|e| format!("ECDH failed: {}", e))?;

    // serialize_compressed() returns [02/03 parity prefix][32-byte x-coordinate].
    // We use only the x-coordinate as ECDH shared secret per standard ECDH convention.
    // Both Rust (serialize_compressed) and TypeScript (getSharedSecret(..., true)) use this format,
    // so skipping byte 0 consistently gives the 32-byte x-coordinate for HKDF input.
    let shared_compressed = shared_point.serialize_compressed();
    let shared_x = &shared_compressed[1..];

    // Derive key: HKDF-SHA256 with domain separation
    // None salt = zero-length (matches TypeScript implementation)
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
        .map_err(|e| format!("Failed to create cipher: {}", e))?;
    let nonce = Nonce::from_slice(nonce_bytes);

    let decrypted = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| format!("ChaCha20-Poly1305 decryption failed: {}", e))?;

    Ok(decrypted)
}

/// Parse a hex-encoded compressed secp256k1 public key (66 hex chars = 33 bytes).
/// Trims whitespace for robustness.
pub fn parse_public_key(hex_str: &str) -> Result<PublicKey, Box<dyn std::error::Error>> {
    let trimmed = hex_str.trim();
    if trimmed.len() != 66 {
        return Err(format!(
            "Expected 66-char compressed public key hex, got {} chars",
            trimmed.len()
        )
        .into());
    }
    let bytes = hex::decode(trimmed)?;
    let pubkey = PublicKey::parse_compressed(&bytes.try_into().map_err(|_| "unexpected length")?)
        .map_err(|e| format!("Invalid public key: {}", e))?;
    Ok(pubkey)
}

/// Encrypt plaintext to a target public key using EC01 format (ECDH + ChaCha20-Poly1305)
///
/// Mirror of decrypt_ecdh: generates ephemeral keypair, performs ECDH, derives key via HKDF,
/// encrypts with ChaCha20-Poly1305.
///
/// Format: EC01 (4) || ephemeral_pubkey (33) || nonce (12) || ciphertext+tag
pub fn encrypt_blob(
    target_pubkey: &PublicKey,
    plaintext: &[u8],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    const MAX_PLAINTEXT: usize = 8 * 1024 * 1024; // 8 MB
    if plaintext.len() > MAX_PLAINTEXT {
        return Err(format!(
            "plaintext too large: {} bytes (max: {} bytes)",
            plaintext.len(), MAX_PLAINTEXT
        ).into());
    }

    // 1. Generate ephemeral keypair
    let mut ephemeral_secret_bytes = [0u8; 32];
    getrandom(&mut ephemeral_secret_bytes)
        .map_err(|e| format!("Failed to generate random bytes: {}", e))?;
    let ephemeral_privkey = SecretKey::parse_slice(&ephemeral_secret_bytes)
        .map_err(|e| format!("Failed to create ephemeral key: {}", e))?;
    let ephemeral_pubkey = PublicKey::from_secret_key(&ephemeral_privkey);

    // 2. ECDH: shared_point = target_pubkey * ephemeral_privkey
    let mut shared_point = *target_pubkey;
    shared_point.tweak_mul_assign(&ephemeral_privkey)
        .map_err(|e| format!("ECDH failed: {}", e))?;

    // Extract x-coordinate (skip prefix byte from compressed pubkey)
    let shared_compressed = shared_point.serialize_compressed();
    let shared_x = &shared_compressed[1..];

    // 3. Derive key: HKDF-SHA256 with domain separation
    let hk = Hkdf::<Sha256>::new(None, shared_x);
    let mut key = [0u8; 32];
    hk.expand(b"near-forms:v1:ecdh", &mut key)
        .map_err(|_| "HKDF expand failed")?;

    // 4. Generate random nonce
    let mut nonce_bytes = [0u8; 12];
    getrandom(&mut nonce_bytes)
        .map_err(|e| format!("Failed to generate nonce: {}", e))?;

    // 5. ChaCha20-Poly1305 encrypt
    let cipher = ChaCha20Poly1305::new_from_slice(&key)
        .map_err(|e| format!("Failed to create cipher: {}", e))?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| format!("ChaCha20-Poly1305 encryption failed: {}", e))?;

    // 6. Assemble: EC01 || ephemeral_pubkey || nonce || ciphertext+tag
    let ephemeral_pub_compressed = ephemeral_pubkey.serialize_compressed();
    let mut output = Vec::with_capacity(4 + 33 + 12 + ciphertext.len());
    output.extend_from_slice(ECDH_MAGIC);
    output.extend_from_slice(&ephemeral_pub_compressed);
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Known test master private key (32 bytes hex)
    const TEST_MASTER_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000001";
    const TEST_FORM_ID: &str = "daf14a0c-20f7-4199-a07b-c6456d53ef2d";

    #[test]
    fn encrypt_decrypt_round_trip() {
        let privkey = parse_private_key(TEST_MASTER_KEY).unwrap();
        let pubkey = PublicKey::from_secret_key(&privkey);
        let plaintext = b"hello near-forms";

        let encrypted = encrypt_blob(&pubkey, plaintext).unwrap();

        // Verify EC01 format
        assert_eq!(&encrypted[0..4], b"EC01");
        assert!(encrypted.len() > 4 + 33 + 12 + 16); // header + pubkey + nonce + tag + data

        let decrypted = decrypt_blob(&privkey, &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn encrypt_decrypt_empty_plaintext() {
        let privkey = parse_private_key(TEST_MASTER_KEY).unwrap();
        let pubkey = PublicKey::from_secret_key(&privkey);

        let encrypted = encrypt_blob(&pubkey, b"").unwrap();
        let decrypted = decrypt_blob(&privkey, &encrypted).unwrap();
        assert!(decrypted.is_empty());
    }

    #[test]
    fn encrypt_decrypt_large_payload() {
        let privkey = parse_private_key(TEST_MASTER_KEY).unwrap();
        let pubkey = PublicKey::from_secret_key(&privkey);
        let plaintext = vec![0xABu8; 100_000];

        let encrypted = encrypt_blob(&pubkey, &plaintext).unwrap();
        let decrypted = decrypt_blob(&privkey, &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn encrypt_decrypt_json_answers() {
        let privkey = parse_private_key(TEST_MASTER_KEY).unwrap();
        let pubkey = PublicKey::from_secret_key(&privkey);

        let answers = serde_json::json!({
            "q1": "Option A",
            "q2": ["Choice 1", "Choice 2"],
            "q3": "Some open text response"
        });
        let plaintext = serde_json::to_vec(&answers).unwrap();

        let encrypted = encrypt_blob(&pubkey, &plaintext).unwrap();
        let decrypted = decrypt_blob(&privkey, &encrypted).unwrap();

        let parsed: serde_json::Value = serde_json::from_slice(&decrypted).unwrap();
        assert_eq!(parsed, answers);
    }

    #[test]
    fn derive_form_privkey_deterministic() {
        let master_key = parse_private_key(TEST_MASTER_KEY).unwrap();

        let derived1 = derive_form_privkey(&master_key, TEST_FORM_ID).unwrap();
        let derived2 = derive_form_privkey(&master_key, TEST_FORM_ID).unwrap();

        assert_eq!(derived1.serialize(), derived2.serialize());
    }

    #[test]
    fn derive_form_privkey_different_forms_different_keys() {
        let master_key = parse_private_key(TEST_MASTER_KEY).unwrap();

        let key1 = derive_form_privkey(&master_key, "form-a").unwrap();
        let key2 = derive_form_privkey(&master_key, "form-b").unwrap();

        assert_ne!(key1.serialize(), key2.serialize());
    }

    #[test]
    fn derive_then_encrypt_decrypt() {
        // Simulates the full flow: derive form key, encrypt to form pubkey, decrypt with form privkey
        let master_key = parse_private_key(TEST_MASTER_KEY).unwrap();
        let form_privkey = derive_form_privkey(&master_key, TEST_FORM_ID).unwrap();
        let form_pubkey = PublicKey::from_secret_key(&form_privkey);

        let plaintext = b"{\"q1\": \"answer\"}";
        let encrypted = encrypt_blob(&form_pubkey, plaintext).unwrap();
        let decrypted = decrypt_blob(&form_privkey, &encrypted).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn wrong_key_fails_to_decrypt() {
        let privkey1 = parse_private_key(TEST_MASTER_KEY).unwrap();
        let pubkey1 = PublicKey::from_secret_key(&privkey1);
        let privkey2 = parse_private_key(
            "0000000000000000000000000000000000000000000000000000000000000002",
        ).unwrap();

        let encrypted = encrypt_blob(&pubkey1, b"secret").unwrap();
        let result = decrypt_blob(&privkey2, &encrypted);
        assert!(result.is_err());
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let privkey = parse_private_key(TEST_MASTER_KEY).unwrap();
        let pubkey = PublicKey::from_secret_key(&privkey);

        let mut encrypted = encrypt_blob(&pubkey, b"secret data").unwrap();
        // Flip a byte in the ciphertext area (after header + pubkey + nonce)
        let flip_idx = 4 + 33 + 12 + 1;
        encrypted[flip_idx] ^= 0xFF;

        let result = decrypt_blob(&privkey, &encrypted);
        assert!(result.is_err());
    }

    #[test]
    fn invalid_magic_rejected() {
        let privkey = parse_private_key(TEST_MASTER_KEY).unwrap();
        let bad_data = b"XX01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";
        let result = decrypt_blob(&privkey, bad_data);
        assert!(result.is_err());
    }

    #[test]
    fn too_short_blob_rejected() {
        let privkey = parse_private_key(TEST_MASTER_KEY).unwrap();
        let result = decrypt_blob(&privkey, b"EC01short");
        assert!(result.is_err());
    }

    #[test]
    fn reduce_mod_order_below_order_unchanged() {
        // A value well below curve order should be returned unchanged
        let mut bytes = [0u8; 32];
        bytes[31] = 42;
        assert_eq!(reduce_mod_order(&bytes), bytes);
    }

    #[test]
    fn reduce_mod_order_at_order_reduced() {
        // Exactly the curve order should reduce to 0
        let result = reduce_mod_order(&CURVE_ORDER);
        assert_eq!(result, [0u8; 32]);
    }

    #[test]
    fn reduce_mod_order_above_order() {
        // curve_order + 1 should reduce to 1
        let mut above = CURVE_ORDER;
        // Add 1 to the last byte (with carry if needed)
        let mut carry = 1u16;
        for i in (0..32).rev() {
            let sum = above[i] as u16 + carry;
            above[i] = sum as u8;
            carry = sum >> 8;
            if carry == 0 {
                break;
            }
        }

        let result = reduce_mod_order(&above);
        let mut expected = [0u8; 32];
        expected[31] = 1;
        assert_eq!(result, expected);
    }

    /// Integration test: simulates the complete submit→store→read→decrypt flow.
    ///
    /// 1. Client derives form public key from master public key (TypeScript side)
    /// 2. Client encrypts answers with form public key (SubmitForm)
    /// 3. WASI decrypts with derived form private key
    /// 4. WASI re-encrypts decrypted payload to caller's ephemeral session key (ReadResponses)
    /// 5. Client decrypts response with session private key
    #[test]
    fn full_submit_read_decrypt_flow() {
        // Setup: master key and form key derivation
        let master_key = parse_private_key(TEST_MASTER_KEY).unwrap();
        let form_privkey = derive_form_privkey(&master_key, TEST_FORM_ID).unwrap();
        let form_pubkey = PublicKey::from_secret_key(&form_privkey);

        // Step 1-2: Client encrypts form answers with form public key
        let answers = serde_json::json!({
            "q1": "Option A",
            "q2": ["Choice 1", "Choice 2"],
            "q3": "Open text response"
        });
        let plaintext = serde_json::to_vec(&answers).unwrap();
        let encrypted_submission = encrypt_blob(&form_pubkey, &plaintext).unwrap();

        // Verify EC01 format
        assert_eq!(&encrypted_submission[0..4], b"EC01");

        // Step 3: WASI module decrypts with form private key (SubmitForm validation)
        let decrypted = decrypt_blob(&form_privkey, &encrypted_submission).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&decrypted).unwrap();
        assert_eq!(parsed, answers);

        // Step 4: WASI module builds response payload and re-encrypts to caller's session key
        let mut session_key_bytes = [0u8; 32];
        getrandom(&mut session_key_bytes).unwrap();
        let session_privkey = SecretKey::parse_slice(&session_key_bytes).unwrap();
        let session_pubkey = PublicKey::from_secret_key(&session_privkey);

        let response_payload = serde_json::json!({
            "responses": [{
                "submitter_id": "alice.testnet",
                "answers": parsed,
                "submitted_at": "2026-03-03T00:00:00Z"
            }],
            "total_count": 1,
            "has_more": false,
            "skipped_count": 0
        });
        let response_bytes = serde_json::to_vec(&response_payload).unwrap();
        let encrypted_response = encrypt_blob(&session_pubkey, &response_bytes).unwrap();

        // Step 5: Client decrypts response with session private key
        let decrypted_response = decrypt_blob(&session_privkey, &encrypted_response).unwrap();
        let parsed_response: serde_json::Value = serde_json::from_slice(&decrypted_response).unwrap();

        // Verify full round-trip integrity
        assert_eq!(parsed_response["responses"][0]["submitter_id"], "alice.testnet");
        assert_eq!(parsed_response["responses"][0]["answers"], answers);
        assert_eq!(parsed_response["total_count"], 1);
        assert_eq!(parsed_response["has_more"], false);
    }

    #[test]
    fn parse_private_key_trims_whitespace() {
        // Trailing newline (common with Docker secrets / copy-paste)
        let with_newline = format!("{}\n", TEST_MASTER_KEY);
        assert!(parse_private_key(&with_newline).is_ok());

        // Leading/trailing spaces
        let with_spaces = format!("  {}  ", TEST_MASTER_KEY);
        assert!(parse_private_key(&with_spaces).is_ok());

        // Wrong length after trim should fail
        assert!(parse_private_key("deadbeef").is_err());
    }

    #[test]
    fn encrypt_blob_rejects_over_8mb() {
        let privkey = parse_private_key(TEST_MASTER_KEY).unwrap();
        let pubkey = PublicKey::from_secret_key(&privkey);
        let oversized = vec![0u8; 8 * 1024 * 1024 + 1];

        let result = encrypt_blob(&pubkey, &oversized);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("too large"));
    }

    #[test]
    fn decrypt_rejects_invalid_ephemeral_pubkey() {
        // Build a blob with valid EC01 header but invalid pubkey bytes (all 0xFF)
        let privkey = parse_private_key(TEST_MASTER_KEY).unwrap();
        let mut bad_blob = Vec::new();
        bad_blob.extend_from_slice(b"EC01");
        bad_blob.extend_from_slice(&[0xFF; 33]); // invalid compressed pubkey
        bad_blob.extend_from_slice(&[0u8; 12]);  // nonce
        bad_blob.extend_from_slice(&[0u8; 17]);  // min ciphertext + tag

        let result = decrypt_blob(&privkey, &bad_blob);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("ephemeral pubkey"));
    }

    /// Golden test vector: master key 0x01 with form_id "daf14a0c-..."
    /// This derives a deterministic form private key that TypeScript must match.
    #[test]
    fn golden_derive_form_pubkey() {
        let master_key = parse_private_key(TEST_MASTER_KEY).unwrap();
        let master_pubkey = PublicKey::from_secret_key(&master_key);

        let form_privkey = derive_form_privkey(&master_key, TEST_FORM_ID).unwrap();
        let form_pubkey = PublicKey::from_secret_key(&form_privkey);

        let master_pubkey_hex = hex::encode(master_pubkey.serialize_compressed());
        let form_pubkey_hex = hex::encode(form_pubkey.serialize_compressed());

        // These values must match TypeScript's deriveFormPublicKey output
        assert_eq!(
            master_pubkey_hex,
            "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
        );
        // Pin the derived form pubkey — if this changes, TypeScript tests will also break
        assert_eq!(
            form_pubkey_hex,
            "02257731f1d53b68b0c8e8602250746131b1b037556343b4f666c9ac753e5cc4ea"
        );
    }
}

