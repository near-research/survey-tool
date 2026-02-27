//! OutLayer WASI module for near-forms
//!
//! Two actions:
//! 1. ReadResponses: Creator reads decrypted form submissions (Transaction mode)
//! 2. SubmitForm: Respondent submits encrypted answers (Transaction mode)

mod crypto;
mod db;
mod types;

use libsecp256k1::{PublicKey, SecretKey};
use outlayer::env;
use types::*;

// ==================== Hardcoded Single Form Config ====================

/// Fixed form ID (same across db-api and WASI module)
const FORM_ID: &str = "daf14a0c-20f7-4199-a07b-c6456d53ef2d";

/// Database API URL (internal service)
fn get_database_url() -> Result<String, Box<dyn std::error::Error>> {
    std::env::var("DATABASE_API_URL")
        .map_err(|_| "DATABASE_API_URL environment variable not found".into())
}

/// Shared secret for db-api authentication
fn get_api_secret() -> Result<String, Box<dyn std::error::Error>> {
    std::env::var("DATABASE_API_SECRET")
        .or_else(|_| std::env::var("API_SECRET"))
        .map_err(|_| "API_SECRET or DATABASE_API_SECRET environment variable not found".into())
}

/// Load master private key from env
fn load_master_key() -> Result<SecretKey, Box<dyn std::error::Error>> {
    if let Ok(master_key_hex) = std::env::var("PROTECTED_MASTER_KEY") {
        return crypto::parse_private_key(&master_key_hex);
    }
    Err("Master key (PROTECTED_MASTER_KEY) not found in env".into())
}

fn main() {
    let result = process();

    match result {
        Ok(output) => {
            let _ = env::output_json(&output);
        }
        Err(e) => {
            let error_response = ErrorResponse {
                success: false,
                error: format!("{}", e),
            };
            let _ = env::output_json(&error_response);
        }
    }
}

fn process() -> Result<Output, Box<dyn std::error::Error>> {
    // Get the input (determines which action to perform)
    // env::input() returns Vec<u8>, return error if parsing fails
    let body = env::input();
    let input: Input = serde_json::from_slice(&body)
        .map_err(|e| format!("Invalid input JSON: {}", e))?;

    match input {
        Input::ReadResponses(_) => handle_read_responses(),
        Input::SubmitForm(submit_input) => handle_submit_form(submit_input),
        Input::GetMasterPublicKey(_) => handle_get_master_public_key(),
    }
}

/// Handle GetMasterPublicKey action (returns compressed secp256k1 public key)
/// No auth required — the public key is not sensitive.
fn handle_get_master_public_key() -> Result<Output, Box<dyn std::error::Error>> {
    let master_privkey = load_master_key()?;
    let master_pubkey = PublicKey::from_secret_key(&master_privkey);
    let pubkey_hex = hex::encode(master_pubkey.serialize_compressed());
    Ok(Output::GetMasterPublicKey(GetMasterPublicKeyOutput {
        master_public_key: pubkey_hex,
    }))
}

/// Handle ReadResponses action (creator reads decrypted submissions)
/// Requires: signer is the form creator
fn handle_read_responses() -> Result<Output, Box<dyn std::error::Error>> {
    // 1. Authenticate via OutLayer TEE (transaction mode)
    let caller_id = env::signer_account_id()
        .ok_or("Authentication required - signer_account_id not available")?;

    // 2. Fetch form metadata and verify caller is the creator
    let db_url = get_database_url()?;
    let form = db::get_form(&db_url, FORM_ID)?;
    if caller_id != form.creator_id {
        return Err("Not authorized to read responses".into());
    }

    // 3. Load master private key
    let master_privkey = load_master_key()?;

    // 4. Fetch encrypted submissions from db-api
    let api_secret = get_api_secret()?;
    let submissions = db::get_submissions(&db_url, FORM_ID, &api_secret)?;

    // 5. Derive form-specific private key
    let form_privkey = crypto::derive_form_privkey(&master_privkey, FORM_ID)?;

    // 6. Decrypt each submission, skipping corrupted entries and tracking skipped count
    let mut responses: Vec<Response> = Vec::new();
    let mut skipped_count = 0usize;

    for submission in submissions.iter() {
        // Try to decrypt and parse this submission
        match (|| -> Result<Response, String> {
            let ciphertext = hex::decode(&submission.encrypted_blob)
                .map_err(|e| format!("Invalid hex ciphertext: {}", e))?;

            let plaintext = crypto::decrypt_blob(&form_privkey, &ciphertext)
                .map_err(|e| format!("Decryption failed: {}", e))?;

            let answers: serde_json::Value = serde_json::from_slice(&plaintext)
                .map_err(|e| format!("Invalid JSON in decrypted answers: {}", e))?;

            Ok(Response {
                submitter_id: submission.submitter_id.clone(),
                answers,
                submitted_at: submission.submitted_at.clone(),
            })
        })() {
            Ok(response) => responses.push(response),
            Err(e) => {
                // Log the error but continue processing other submissions
                eprintln!("Skipping corrupted submission {}: {}", submission.submitter_id, e);
                skipped_count += 1;
            }
        }
    }

    Ok(Output::ReadResponses(ReadResponsesOutput {
        responses,
        skipped_count,
    }))
}

/// Handle SubmitForm action (respondent submits pre-encrypted form)
/// Answers are encrypted client-side using EC01 format so plaintext never appears on-chain.
/// Requires: caller has a valid NEAR wallet (authenticated by OutLayer transaction)
fn handle_submit_form(input: SubmitFormInput) -> Result<Output, Box<dyn std::error::Error>> {
    // 1. Authenticate respondent via OutLayer TEE
    let submitter_id = env::signer_account_id()
        .ok_or("Authentication required - wallet signature not valid")?;

    // 2. Validate the pre-encrypted EC01 blob
    let encrypted_bytes = hex::decode(&input.encrypted_answers)
        .map_err(|e| format!("Invalid hex in encrypted_answers: {}", e))?;

    // Verify EC01 format header
    const MIN_EC01_SIZE: usize = 4 + 33 + 12 + 16; // magic + pubkey + nonce + tag
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

    // Enforce size limit
    const MAX_BLOB_SIZE: usize = 200 * 1024; // 200 KB
    if encrypted_bytes.len() > MAX_BLOB_SIZE {
        return Err(format!(
            "encrypted_answers too large: {} bytes (max: {} bytes)",
            encrypted_bytes.len(), MAX_BLOB_SIZE
        ).into());
    }

    // 3. Store pre-encrypted blob to db-api
    let db_url = get_database_url()?;
    let api_secret = get_api_secret()?;
    let submission_id = db::create_submission(
        &db_url,
        FORM_ID,
        &submitter_id,
        &input.encrypted_answers,
        &api_secret,
    )?;

    Ok(Output::SubmitForm(SubmitFormOutput {
        success: true,
        submission_id,
    }))
}
