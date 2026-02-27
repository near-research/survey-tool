//! Database API client for near-forms WASI module
//!
//! Fetches encrypted form submissions and stores new submissions via HTTP API

use crate::types::{EncryptedSubmission, FormMetadata};
use std::time::Duration;
use wasi_http_client::Client;

/// HTTP request timeout
const TIMEOUT: Duration = Duration::from_secs(30);

/// Fetch form metadata from db-api (public endpoint, no auth)
///
/// Calls GET /forms/{form_id}
pub fn get_form(
    api_url: &str,
    form_id: &str,
) -> Result<FormMetadata, Box<dyn std::error::Error>> {
    let url = format!("{}/forms/{}", api_url, form_id);

    let request = Client::new()
        .get(&url)
        .connect_timeout(TIMEOUT);

    let response = request.send()?;
    let status = response.status();

    if status != 200 {
        let body = response.body().unwrap_or_default();
        let snippet = String::from_utf8_lossy(&body[..body.len().min(200)]);
        return Err(format!("Failed to fetch form (status {}): {}", status, snippet).into());
    }

    let body = response.body()?;
    let form: FormMetadata = serde_json::from_slice(&body)
        .map_err(|e| {
            let snippet = String::from_utf8_lossy(&body[..body.len().min(200)]);
            format!("Invalid form JSON: {} (body: {})", e, snippet)
        })?;

    Ok(form)
}

/// Fetch encrypted form submissions from db-api
///
/// Calls GET /forms/{form_id}/submissions with API-Secret header
pub fn get_submissions(
    api_url: &str,
    form_id: &str,
    api_secret: &str,
) -> Result<Vec<EncryptedSubmission>, Box<dyn std::error::Error>> {
    let url = format!("{}/forms/{}/submissions", api_url, form_id);

    let request = Client::new()
        .get(&url)
        .connect_timeout(TIMEOUT)
        .header("API-Secret", api_secret);

    let response = request.send()?;
    let status = response.status();

    if status != 200 {
        let body = response.body().unwrap_or_default();
        let snippet = String::from_utf8_lossy(&body[..body.len().min(200)]);
        return Err(format!("Failed to fetch submissions (status {}): {}", status, snippet).into());
    }

    // Parse JSON response
    let body = response.body()?;

    let submissions: Vec<EncryptedSubmission> = serde_json::from_slice(&body)
        .map_err(|e| {
            let snippet = String::from_utf8_lossy(&body[..body.len().min(200)]);
            format!("Invalid submissions JSON: {} (body: {})", e, snippet)
        })?;

    Ok(submissions)
}

/// Store a new encrypted form submission to db-api
///
/// Calls POST /submissions with API-Secret header
pub fn create_submission(
    api_url: &str,
    form_id: &str,
    submitter_id: &str,
    encrypted_blob: &str,
    api_secret: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let url = format!("{}/submissions", api_url);

    let body = serde_json::json!({
        "form_id": form_id,
        "submitter_id": submitter_id,
        "encrypted_blob": encrypted_blob,
    });

    let body_bytes = serde_json::to_vec(&body)?;

    let request = Client::new()
        .post(&url)
        .connect_timeout(TIMEOUT)
        .header("Content-Type", "application/json")
        .header("API-Secret", api_secret);

    let response = request.body(&body_bytes).send()?;
    let status = response.status();

    if status != 200 && status != 201 {
        // Check for duplicate submission (unique constraint violation)
        if status == 409 {
            return Err("You have already submitted this form. Each account can only submit once.".into());
        }

        return Err(format!("Failed to create submission (status {})", status).into());
    }

    // Extract submission ID from response
    let body = response.body()?;
    let response_json: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|e| format!("Invalid submission response JSON: {}", e))?;

    let submission_id = response_json["id"]
        .as_str()
        .ok_or("Missing submission ID in response")?
        .to_string();

    Ok(submission_id)
}
