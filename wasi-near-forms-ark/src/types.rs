//! Type definitions for near-forms WASI module

use serde::{Deserialize, Serialize};

/// WASI module input - determines which action to perform
#[derive(Debug, Deserialize)]
#[serde(tag = "action")]
pub enum Input {
    /// ReadResponses: Fetch and decrypt all submissions (creator only, via transaction)
    ReadResponses(ReadResponsesInput),
    /// SubmitForm: Receive and encrypt a form submission (respondent, via transaction)
    SubmitForm(SubmitFormInput),
}

/// Input for ReadResponses action
#[derive(Debug, Deserialize)]
pub struct ReadResponsesInput {
    // Empty for now - future expansion could add filtering
}

/// Input for SubmitForm action
#[derive(Debug, Deserialize)]
pub struct SubmitFormInput {
    /// Pre-encrypted EC01 blob (hex-encoded) from client-side encryption
    pub encrypted_answers: String,
}

/// WASI module output - union of possible response types
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Output {
    /// ReadResponses output: decrypted submissions
    ReadResponses(ReadResponsesOutput),
    /// SubmitForm output: confirmation
    SubmitForm(SubmitFormOutput),
}

/// Output for ReadResponses action
#[derive(Debug, Serialize)]
pub struct ReadResponsesOutput {
    /// Decrypted form responses
    pub responses: Vec<Response>,
    /// Number of submissions that could not be decrypted (indicates potential data loss)
    #[serde(default)]
    pub skipped_count: usize,
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
