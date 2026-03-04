//! Input validation for the near-forms WASI module.
//!
//! Pure functions extracted from `main.rs` for testability:
//! - [`is_implicit_account`] — detects 64-char hex NEAR implicit accounts
//! - [`sanitize_error`] — strips internal details from on-chain error messages
//! - [`validate_ec01_hex`] — validates hex-encoded EC01 ciphertext format

/// Maximum binary size for encrypted submissions (200 KB).
const MAX_BLOB_SIZE: usize = 200 * 1024;

/// Maximum hex-encoded length (hex doubles the byte count).
const MAX_HEX_LEN: usize = MAX_BLOB_SIZE * 2;

/// Minimum EC01 ciphertext size: magic(4) + compressed pubkey(33) + nonce(12) + Poly1305 tag(16).
const MIN_EC01_SIZE: usize = 4 + 33 + 12 + 16;

/// Check if a NEAR account ID is an implicit account (64-char lowercase hex = ed25519 pubkey).
///
/// Implicit accounts can be created without on-chain registration and could be used
/// for spoofing, so they are rejected for form submissions and response reads.
///
/// # Examples
///
/// ```ignore
/// assert!(is_implicit_account("a]".repeat(32).as_str())); // 64 hex chars
/// assert!(!is_implicit_account("alice.testnet"));
/// ```
pub fn is_implicit_account(account_id: &str) -> bool {
    account_id.len() == 64 && account_id.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

/// Known safe prefixes from this module's own error messages.
const PASSTHROUGH_PREFIXES: &[&str] = &[
    "Authentication required",
    "Not authorized",
    "Implicit accounts",
    "Invalid input JSON",
    "encrypted_answers too short",
    "encrypted_answers too large",
    "encrypted_answers hex too long",
    "encrypted_answers must start with EC01 magic bytes",
    "Invalid response_pubkey",
    "Private key must be exactly",
];

/// Known safe substrings (exact phrases from db-api responses).
/// COUPLING: These strings must match db-api error messages in:
///   - db-api/src/lib.rs create_submission() → "already submitted this form" (unique violation)
///   - db-api/src/lib.rs get_form() / create_submission() → "Form not found" (404 / FK violation)
const PASSTHROUGH_CONTAINS: &[&str] = &[
    "already submitted this form",
    "Form not found",
];

/// Sanitize internal error messages for on-chain responses.
///
/// Keeps user-actionable messages (auth, format, size) but strips crypto
/// implementation details. Uses prefix-based matching on this module's own error
/// strings to avoid accidentally passing through dependency errors.
///
/// # Examples
///
/// ```ignore
/// assert_eq!(sanitize_error("Not authorized to read responses"), "Not authorized to read responses");
/// assert_eq!(sanitize_error("secp256k1 internal panic"), "Request failed. Please try again or contact the form administrator.");
/// ```
pub fn sanitize_error(err: &str) -> String {
    for prefix in PASSTHROUGH_PREFIXES {
        if err.starts_with(prefix) {
            return err.to_string();
        }
    }
    for substr in PASSTHROUGH_CONTAINS {
        if err.contains(substr) {
            return err.to_string();
        }
    }

    // Everything else (crypto internals, database errors) — return generic message
    "Request failed. Please try again or contact the form administrator.".to_string()
}

/// Validate a hex-encoded EC01 ciphertext blob and return the decoded bytes.
///
/// Checks in order:
/// 1. Hex length does not exceed [`MAX_HEX_LEN`] (400 KB)
/// 2. Valid hex decoding
/// 3. Minimum binary size (magic + pubkey + nonce + tag = 65 bytes)
/// 4. EC01 magic bytes (`b"EC01"`)
/// 5. Ephemeral public key is a valid compressed secp256k1 point
/// 6. Binary size does not exceed [`MAX_BLOB_SIZE`] (200 KB)
///
/// Returns the decoded ciphertext bytes on success.
pub fn validate_ec01_hex(hex_str: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if hex_str.len() > MAX_HEX_LEN {
        return Err(format!(
            "encrypted_answers hex too long: {} chars (max: {})",
            hex_str.len(), MAX_HEX_LEN
        ).into());
    }

    let encrypted_bytes = hex::decode(hex_str)
        .map_err(|e| format!("Invalid hex in encrypted_answers: {}", e))?;

    if encrypted_bytes.len() < MIN_EC01_SIZE {
        return Err(format!(
            "encrypted_answers too short: {} bytes, need at least {}",
            encrypted_bytes.len(), MIN_EC01_SIZE
        ).into());
    }

    if &encrypted_bytes[0..4] != b"EC01" {
        return Err("encrypted_answers must start with EC01 magic bytes".into());
    }

    // Verify the ephemeral public key is a valid compressed secp256k1 point
    let ephemeral_pubkey_bytes = &encrypted_bytes[4..37];
    libsecp256k1::PublicKey::parse_slice(ephemeral_pubkey_bytes, None)
        .map_err(|e| format!("Invalid ephemeral public key in EC01 blob: {:?}", e))?;

    if encrypted_bytes.len() > MAX_BLOB_SIZE {
        return Err(format!(
            "encrypted_answers too large: {} bytes (max: {} bytes)",
            encrypted_bytes.len(), MAX_BLOB_SIZE
        ).into());
    }

    Ok(encrypted_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== is_implicit_account ====================

    #[test]
    fn implicit_valid_64_hex() {
        let hex64 = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        assert!(is_implicit_account(hex64));
    }

    #[test]
    fn implicit_all_digits() {
        let digits = "0".repeat(64);
        assert!(is_implicit_account(&digits));
    }

    #[test]
    fn implicit_too_short() {
        let hex63 = "a".repeat(63);
        assert!(!is_implicit_account(&hex63));
    }

    #[test]
    fn implicit_too_long() {
        let hex65 = "a".repeat(65);
        assert!(!is_implicit_account(&hex65));
    }

    #[test]
    fn implicit_uppercase_rejected() {
        // 64 chars but uppercase hex — not implicit (lowercase only)
        let upper = "A".repeat(64);
        assert!(!is_implicit_account(&upper));
    }

    #[test]
    fn implicit_named_account() {
        assert!(!is_implicit_account("alice.testnet"));
    }

    #[test]
    fn implicit_empty_string() {
        assert!(!is_implicit_account(""));
    }

    #[test]
    fn implicit_mixed_case() {
        // 64 chars with mixed case — not implicit
        let mixed = "aA".repeat(32);
        assert_eq!(mixed.len(), 64);
        assert!(!is_implicit_account(&mixed));
    }

    // ==================== sanitize_error ====================

    #[test]
    fn sanitize_auth_required() {
        let msg = "Authentication required - signer_account_id not available";
        assert_eq!(sanitize_error(msg), msg);
    }

    #[test]
    fn sanitize_not_authorized() {
        let msg = "Not authorized to read responses";
        assert_eq!(sanitize_error(msg), msg);
    }

    #[test]
    fn sanitize_implicit_accounts() {
        let msg = "Implicit accounts (64-char hex) are not allowed.";
        assert_eq!(sanitize_error(msg), msg);
    }

    #[test]
    fn sanitize_invalid_json() {
        let msg = "Invalid input JSON: expected value at line 1 column 1";
        assert_eq!(sanitize_error(msg), msg);
    }

    #[test]
    fn sanitize_too_short() {
        let msg = "encrypted_answers too short: 10 bytes, need at least 65";
        assert_eq!(sanitize_error(msg), msg);
    }

    #[test]
    fn sanitize_too_large() {
        let msg = "encrypted_answers too large: 999999 bytes (max: 204800 bytes)";
        assert_eq!(sanitize_error(msg), msg);
    }

    #[test]
    fn sanitize_db_api_already_submitted() {
        let msg = "db-api error: You have already submitted this form. Each account can only submit once.";
        assert_eq!(sanitize_error(msg), msg);
    }

    #[test]
    fn sanitize_db_api_form_not_found() {
        let msg = "db-api returned 404: Form not found";
        assert_eq!(sanitize_error(msg), msg);
    }

    #[test]
    fn sanitize_hex_too_long() {
        let msg = "encrypted_answers hex too long: 500000 chars (max: 409600)";
        assert_eq!(sanitize_error(msg), msg);
    }

    #[test]
    fn sanitize_crypto_error_stripped() {
        let msg = "secp256k1 scalar multiplication failed: InvalidSecretKey";
        let sanitized = sanitize_error(msg);
        assert_eq!(sanitized, "Request failed. Please try again or contact the form administrator.");
    }

    #[test]
    fn sanitize_empty_string() {
        let sanitized = sanitize_error("");
        assert_eq!(sanitized, "Request failed. Please try again or contact the form administrator.");
    }

    // ==================== validate_ec01_hex ====================

    /// Build a minimal valid EC01 hex blob using a real secp256k1 public key.
    fn make_valid_ec01_hex(payload_len: usize) -> String {
        // Generate a real compressed public key from a known secret key
        let secret = libsecp256k1::SecretKey::parse_slice(
            &[1u8; 32]
        ).unwrap();
        let pubkey = libsecp256k1::PublicKey::from_secret_key(&secret);
        let compressed = pubkey.serialize_compressed(); // 33 bytes

        let mut blob = Vec::new();
        blob.extend_from_slice(b"EC01");            // 4 bytes magic
        blob.extend_from_slice(&compressed);         // 33 bytes pubkey
        blob.extend_from_slice(&[0u8; 12]);          // 12 bytes nonce
        blob.extend_from_slice(&[0u8; 16]);          // 16 bytes tag
        blob.extend_from_slice(&vec![0u8; payload_len]); // payload
        hex::encode(blob)
    }

    #[test]
    fn ec01_valid_minimal() {
        let hex_blob = make_valid_ec01_hex(0);
        let result = validate_ec01_hex(&hex_blob);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), MIN_EC01_SIZE);
    }

    #[test]
    fn ec01_valid_with_payload() {
        let hex_blob = make_valid_ec01_hex(100);
        let result = validate_ec01_hex(&hex_blob);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), MIN_EC01_SIZE + 100);
    }

    #[test]
    fn ec01_hex_too_long() {
        // MAX_HEX_LEN + 2 chars (1 extra byte)
        let hex_blob = "a".repeat(MAX_HEX_LEN + 2);
        let err = validate_ec01_hex(&hex_blob).unwrap_err();
        assert!(err.to_string().contains("too long"));
    }

    #[test]
    fn ec01_invalid_hex_chars() {
        let err = validate_ec01_hex("ZZZZ").unwrap_err();
        assert!(err.to_string().contains("Invalid hex"));
    }

    #[test]
    fn ec01_too_short() {
        // Valid hex but too few bytes once decoded
        let short = hex::encode(b"EC01");  // only 4 bytes
        let err = validate_ec01_hex(&short).unwrap_err();
        assert!(err.to_string().contains("too short"));
    }

    #[test]
    fn ec01_wrong_magic() {
        // Right length but wrong magic bytes
        let mut bytes = vec![0u8; MIN_EC01_SIZE];
        bytes[0..4].copy_from_slice(b"XXXX");
        let err = validate_ec01_hex(&hex::encode(&bytes)).unwrap_err();
        assert!(err.to_string().contains("EC01 magic bytes"));
    }

    #[test]
    fn ec01_invalid_pubkey() {
        // EC01 magic + 33 zero bytes (not a valid secp256k1 point) + nonce + tag
        let mut bytes = vec![0u8; MIN_EC01_SIZE];
        bytes[0..4].copy_from_slice(b"EC01");
        // bytes[4..37] are all zeros — invalid compressed point
        let err = validate_ec01_hex(&hex::encode(&bytes)).unwrap_err();
        assert!(err.to_string().contains("Invalid ephemeral public key"));
    }

    #[test]
    fn ec01_oversized_binary() {
        // Since MAX_HEX_LEN = MAX_BLOB_SIZE * 2, any blob exceeding MAX_BLOB_SIZE
        // also exceeds MAX_HEX_LEN and hits the hex-length check first.
        // The binary size check is defense-in-depth for independent constant changes.
        let excess = MAX_BLOB_SIZE - MIN_EC01_SIZE + 1;
        let hex_blob = make_valid_ec01_hex(excess);
        let err = validate_ec01_hex(&hex_blob).unwrap_err();
        assert!(err.to_string().contains("too long"));
    }
}
