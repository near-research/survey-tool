//! OutLayer WASI module for near-forms
//!
//! Three actions:
//! 1. ReadResponses: Creator reads decrypted form submissions (Transaction mode)
//! 2. SubmitForm: Respondent submits encrypted answers (Transaction mode)
//! 3. GetMasterPublicKey: Returns the master public key (no auth required)

mod crypto;
mod db;
mod http_chunked;
mod types;
mod validation;

use libsecp256k1::{PublicKey, SecretKey};
use outlayer::env;
use types::*;
use validation::{is_implicit_account, sanitize_error, validate_ec01_hex};

// ==================== Hardcoded Single Form Config ====================

/// Fixed form ID — must match `FORM_ID` in `db-api/src/main.rs`.
const FORM_ID: &str = "daf14a0c-20f7-4199-a07b-c6456d53ef2d";

/// Maximum responses per page (caps user-provided limit)
const MAX_PAGE_LIMIT: u32 = 200;

/// Maximum plaintext JSON budget for responses.
/// Pipeline: plaintext (~4MB) → EC01 encrypt (+65B) → hex encode (2×) → ~8MB output.
/// Must stay under NEAR RPC json_payload_max_size (~10MB) after JSON wrapping + base64 (~33%).
/// 4MB plaintext → ~8MB hex is within limits but tight. Pagination handles overflow.
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// Database API URL (internal service)
fn get_database_url() -> Result<String, Box<dyn std::error::Error>> {
    std::env::var("DATABASE_API_URL")
        .map_err(|_| "DATABASE_API_URL environment variable not found".into())
}

/// Shared secret for db-api authentication.
/// Prefers DATABASE_API_SECRET, falls back to API_SECRET.
fn get_api_secret() -> Result<String, Box<dyn std::error::Error>> {
    let primary = std::env::var("DATABASE_API_SECRET").ok();
    let fallback = std::env::var("API_SECRET").ok();

    // Warn if both are set with different values (likely misconfiguration)
    if let (Some(ref p), Some(ref f)) = (&primary, &fallback) {
        if p != f {
            eprintln!("WARNING: DATABASE_API_SECRET and API_SECRET are both set with different values. Using DATABASE_API_SECRET.");
        }
    }

    if primary.is_none() && fallback.is_some() {
        eprintln!("NOTE: DATABASE_API_SECRET not set, using fallback API_SECRET.");
    }

    primary
        .or(fallback)
        .ok_or_else(|| "API_SECRET or DATABASE_API_SECRET environment variable not found".into())
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
            // Panic on serialization failure — if we can't write output, the caller
            // gets no response regardless, so failing loudly is better than silent nothing.
            env::output_json(&output).expect("Failed to serialize output JSON");
        }
        Err(e) => {
            // Log full error details to stderr for debugging (TEE-internal only)
            eprintln!("near-forms error: {}", e);
            // Return sanitized error to on-chain response (visible to anyone)
            let user_message = sanitize_error(&format!("{}", e));
            let error_response = ErrorResponse {
                success: false,
                error: user_message,
            };
            env::output_json(&error_response).expect("Failed to serialize error response JSON");
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
        Input::ReadResponses(read_input) => handle_read_responses(read_input),
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
/// Response is encrypted to the caller's ephemeral public key so plaintext never appears on-chain.
/// Supports pagination via offset/limit and response size budgeting.
fn handle_read_responses(input: ReadResponsesInput) -> Result<Output, Box<dyn std::error::Error>> {
    // 1. Authenticate via OutLayer TEE (transaction mode) — before parsing untrusted input
    let caller_id = env::signer_account_id()
        .ok_or("Authentication required - signer_account_id not available")?;

    // 1b. Reject implicit accounts (consistency with SubmitForm — defense-in-depth)
    if is_implicit_account(&caller_id) {
        return Err("Implicit accounts (64-char hex) are not allowed. Please use a named NEAR account.".into());
    }

    // 2. Load master private key early (fail fast before DB round-trip)
    let master_privkey = load_master_key()?;

    // 3. Fetch form metadata and verify caller is the creator (before parsing untrusted input)
    let db_url = get_database_url()?;
    let form = db::get_form(&db_url, FORM_ID)?;
    if caller_id != form.creator_id {
        return Err("Not authorized to read responses".into());
    }

    // 4. Parse and validate response_pubkey (after authorization — unauthorized callers
    //    should always see "Not authorized", not "Invalid response_pubkey")
    let response_pubkey = crypto::parse_public_key(&input.response_pubkey)
        .map_err(|e| format!("Invalid response_pubkey: {}", e))?;

    // 5. Fetch paginated encrypted submissions from db-api
    let api_secret = get_api_secret()?;
    let limit = input.limit.clamp(1, MAX_PAGE_LIMIT);
    // Reject absurdly large offsets instead of silently clamping (confusing pagination)
    // Prevents memory exhaustion from absurdly large page offsets
    const MAX_OFFSET: u32 = 1_000_000;
    if input.offset > MAX_OFFSET {
        return Err(format!("Offset too large: {} (max: {})", input.offset, MAX_OFFSET).into());
    }
    let offset = input.offset;
    let page = db::get_submissions(&db_url, FORM_ID, &api_secret, offset, limit)?;

    // 6. Derive form-specific private key
    let form_privkey = crypto::derive_form_privkey(&master_privkey, FORM_ID)?;

    // 7. Decrypt each submission with size budgeting
    let mut responses: Vec<Response> = Vec::new();
    let mut skipped_count = 0usize;
    let mut skipped_submissions: Vec<SkippedSubmission> = Vec::new();
    let mut accumulated_size = 0usize;
    let mut size_limit_hit = false;

    for submission in page.submissions.iter() {
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
            Ok(response) => {
                // Estimate serialized size: submitter_id + answers JSON + submitted_at + overhead
                let estimated_size = response.submitter_id.len()
                    .saturating_add(response.submitted_at.len())
                    .saturating_add(response.answers.to_string().len())
                    .saturating_add(64); // JSON structural overhead

                // Always include at least one response to avoid returning empty pages
                if accumulated_size.saturating_add(estimated_size) > MAX_RESPONSE_BYTES && !responses.is_empty() {
                    size_limit_hit = true;
                    break;
                }

                accumulated_size = accumulated_size.saturating_add(estimated_size);
                responses.push(response);
            }
            Err(e) => {
                eprintln!("Skipping submission from {}: {}", submission.submitter_id, e);
                skipped_count += 1;
                skipped_submissions.push(SkippedSubmission {
                    submitter_id: submission.submitter_id.clone(),
                    error: format!("Could not decrypt: {}", e),
                });
            }
        }
    }

    // 8. Determine if there are more results (from pagination or size limit)
    // Safe cast: both values bounded by MAX_PAGE_LIMIT (200) << u32::MAX
    let returned_count = (responses.len() + skipped_count) as u32;
    let next_offset = offset.saturating_add(returned_count);
    let has_more = size_limit_hit || (next_offset as i64) < page.total_count;

    // 9. Serialize the plaintext payload, then encrypt it to the caller's ephemeral key
    let payload = ReadResponsesPayload {
        responses,
        skipped_count,
        skipped_submissions,
        total_count: page.total_count,
        has_more,
        next_offset,
    };
    let payload_json = serde_json::to_vec(&payload)
        .map_err(|e| format!("Failed to serialize response payload: {}", e))?;

    if payload_json.len() > MAX_RESPONSE_BYTES {
        return Err("Response payload too large. Try using a smaller page size (limit parameter).".into());
    }

    let encrypted = crypto::encrypt_blob(&response_pubkey, &payload_json)
        .map_err(|e| format!("Failed to encrypt response: {}", e))?;

    Ok(Output::ReadResponses(EncryptedResponseOutput {
        encrypted_payload: hex::encode(encrypted),
    }))
}

/// Handle SubmitForm action (respondent submits pre-encrypted form)
/// Answers are encrypted client-side using EC01 format so plaintext never appears on-chain.
/// Requires: caller has a valid NEAR wallet (authenticated by OutLayer transaction)
fn handle_submit_form(input: SubmitFormInput) -> Result<Output, Box<dyn std::error::Error>> {
    // 1. Authenticate respondent via OutLayer TEE
    let submitter_id = env::signer_account_id()
        .ok_or("Authentication required - wallet signature not valid")?;

    // 2. Reject implicit accounts (64-char hex = ed25519 pubkey, defense-in-depth)
    if is_implicit_account(&submitter_id) {
        return Err("Implicit accounts (64-char hex) are not allowed to submit forms. Please use a named NEAR account.".into());
    }

    // 3. Validate the pre-encrypted EC01 blob (format, size, pubkey)
    let _encrypted_bytes = validate_ec01_hex(&input.encrypted_answers)?;

    // 4. Store pre-encrypted blob to db-api (uses chunked HTTP writes)
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
