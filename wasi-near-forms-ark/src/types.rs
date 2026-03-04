//! Type definitions for near-forms WASI module

use serde::{Deserialize, Serialize};

/// Maximum HTTP response body size (10 MB). Shared by db.rs and http_chunked.rs.
pub const MAX_RESPONSE_SIZE: usize = 10 * 1024 * 1024;

/// WASI module input - determines which action to perform
#[derive(Debug, Deserialize)]
#[serde(tag = "action")]
pub enum Input {
    /// ReadResponses: Fetch and decrypt all submissions (creator only, via transaction)
    ReadResponses(ReadResponsesInput),
    /// SubmitForm: Receive and encrypt a form submission (respondent, via transaction)
    SubmitForm(SubmitFormInput),
    /// GetMasterPublicKey: Return the master public key (no auth required)
    GetMasterPublicKey(GetMasterPublicKeyInput),
}

/// Input for ReadResponses action
#[derive(Debug, Deserialize)]
pub struct ReadResponsesInput {
    /// Hex-encoded compressed secp256k1 public key for encrypting the response
    pub response_pubkey: String,
    /// Pagination offset (0-based, default: 0)
    #[serde(default)]
    pub offset: u32,
    /// Pagination limit (default: 50, max: 200)
    #[serde(default = "default_page_limit")]
    pub limit: u32,
}

fn default_page_limit() -> u32 {
    50
}

/// Input for SubmitForm action
#[derive(Debug, Deserialize)]
pub struct SubmitFormInput {
    /// Pre-encrypted EC01 blob (hex-encoded) from client-side encryption
    pub encrypted_answers: String,
}

/// Input for GetMasterPublicKey action
#[derive(Debug, Deserialize)]
pub struct GetMasterPublicKeyInput {}

/// WASI module output - union of possible response types.
///
/// Uses `#[serde(untagged)]` so each action returns its own JSON shape without a
/// type discriminator field. The web-ui differentiates responses by checking for
/// action-specific fields (`encrypted_payload`, `success`, `master_public_key`).
///
/// **IMPORTANT for future contributors:** Because this enum is `untagged`, serde tries
/// each variant in declaration order until one serializes successfully. All variants
/// MUST have disjoint top-level field names. If two variants ever share a field name
/// (e.g., both have `success`), serde will silently serialize as the first matching
/// variant, producing incorrect output. Current variants are disjoint:
/// - `EncryptedResponseOutput`: `encrypted_payload`
/// - `SubmitFormOutput`: `success`, `submission_id`
/// - `GetMasterPublicKeyOutput`: `master_public_key`
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Output {
    /// ReadResponses output: encrypted blob containing decrypted submissions
    ReadResponses(EncryptedResponseOutput),
    /// SubmitForm output: confirmation
    SubmitForm(SubmitFormOutput),
    /// GetMasterPublicKey output: hex-encoded compressed public key
    GetMasterPublicKey(GetMasterPublicKeyOutput),
}

/// Output for GetMasterPublicKey action
#[derive(Debug, Serialize)]
pub struct GetMasterPublicKeyOutput {
    pub master_public_key: String,
}

/// Output for ReadResponses action (encrypted wrapper — plaintext never appears on-chain)
#[derive(Debug, Serialize)]
pub struct EncryptedResponseOutput {
    /// Hex-encoded EC01 blob containing the encrypted ReadResponsesPayload JSON
    pub encrypted_payload: String,
}

/// Inner payload encrypted inside EncryptedResponseOutput
#[derive(Debug, Serialize)]
pub struct ReadResponsesPayload {
    /// Decrypted form responses (for this page)
    pub responses: Vec<Response>,
    /// Number of submissions that could not be decrypted (indicates potential data loss)
    pub skipped_count: usize,
    /// Details of skipped submissions so the creator can investigate
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped_submissions: Vec<SkippedSubmission>,
    /// Total number of submissions across all pages (i64 to match PostgreSQL COUNT(*) bigint)
    pub total_count: i64,
    /// Whether there are more submissions beyond this page
    pub has_more: bool,
    /// Authoritative offset for the next page (accounts for size-limit breaks and skipped items)
    pub next_offset: u32,
}

/// A submission that could not be decrypted
#[derive(Debug, Serialize)]
pub struct SkippedSubmission {
    pub submitter_id: String,
    pub error: String,
}

/// Output for SubmitForm action
#[derive(Debug, Serialize)]
pub struct SubmitFormOutput {
    pub success: bool,
    pub submission_id: String,
}

/// Decrypted form submission response
#[derive(Debug, Serialize, Clone)]
pub struct Response {
    /// NEAR account ID of the form submitter (plaintext - intentional)
    pub submitter_id: String,
    /// Decrypted form answers as JSON object
    pub answers: serde_json::Value,
    /// ISO 8601 timestamp when the form was submitted
    pub submitted_at: String,
}

/// Paginated submissions response from db-api
#[derive(Debug, Deserialize)]
pub struct SubmissionsPage {
    /// Submissions for this page
    pub submissions: Vec<EncryptedSubmission>,
    /// Total number of submissions (across all pages; i64 to match PostgreSQL COUNT(*) bigint)
    pub total_count: i64,
}

/// Encrypted form submission from database
#[derive(Debug, Deserialize)]
pub struct EncryptedSubmission {
    /// Wallet address that submitted the form
    pub submitter_id: String,
    /// Hex-encoded EC01 ciphertext (magic + ephemeral_pubkey + nonce + chacha20 ciphertext)
    pub encrypted_blob: String,
    /// ISO 8601 timestamp of submission
    pub submitted_at: String,
}

/// Form metadata from db-api (GET /forms/{form_id})
#[derive(Debug, Deserialize)]
pub struct FormMetadata {
    pub creator_id: String,
}

/// Error response from WASI module
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub success: bool,
    pub error: String,
}
