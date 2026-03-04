//! Database API client for near-forms WASI module
//!
//! Fetches encrypted form submissions and stores new submissions via HTTP API.
//! Uses chunked HTTP writes for POST requests to bypass the ~4KB WASI-HTTP single-write limit.
//! Uses low-level wasi::http for GET requests to set all three timeout types.

use crate::http_chunked;
use crate::types::{FormMetadata, SubmissionsPage};
use std::time::Duration;
use wasi::http::{
    outgoing_handler,
    types::{Headers, Method, OutgoingBody, OutgoingRequest, RequestOptions, Scheme},
};

/// HTTP request timeout (connect, first-byte, and between-bytes)
const TIMEOUT: Duration = Duration::from_secs(30);

/// Build URL for GET /v1/forms/{form_id}
fn form_url(api_url: &str, form_id: &str) -> String {
    format!("{}/v1/forms/{}", api_url, form_id)
}

/// Build URL for GET /v1/forms/{form_id}/submissions?offset=N&limit=N
fn submissions_url(api_url: &str, form_id: &str, offset: u32, limit: u32) -> String {
    format!(
        "{}/v1/forms/{}/submissions?offset={}&limit={}",
        api_url, form_id, offset, limit
    )
}

/// Build URL for POST /v1/submissions
fn create_submission_url(api_url: &str) -> String {
    format!("{}/v1/submissions", api_url)
}

/// Send a GET request with full timeout control (connect, first-byte, between-bytes).
///
/// Uses low-level wasi::http types because wasi-http-client only supports connect_timeout.
fn get_with_timeout(
    url: &str,
    timeout: Duration,
    extra_headers: &[(&str, &str)],
) -> Result<(u16, Vec<u8>), Box<dyn std::error::Error>> {
    let parsed = url::Url::parse(url)
        .map_err(|e| format!("Failed to parse URL '{}': {}", url, e))?;

    let scheme = match parsed.scheme() {
        "https" => Scheme::Https,
        "http" => Scheme::Http,
        s => Scheme::Other(s.to_string()),
    };

    let authority = parsed
        .host_str()
        .map(|h| {
            if let Some(port) = parsed.port() {
                format!("{}:{}", h, port)
            } else {
                h.to_string()
            }
        })
        .ok_or_else(|| format!("No host in URL: {}", url))?;

    let path_and_query = if let Some(q) = parsed.query() {
        format!("{}?{}", parsed.path(), q)
    } else {
        parsed.path().to_string()
    };

    let headers_list: Vec<(String, Vec<u8>)> = extra_headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.as_bytes().to_vec()))
        .collect();

    let headers = Headers::from_list(&headers_list)
        .map_err(|e| format!("Failed to create headers: {:?}", e))?;

    let request = OutgoingRequest::new(headers);
    request.set_method(&Method::Get)
        .map_err(|e| format!("Failed to set method: {:?}", e))?;
    request.set_scheme(Some(&scheme))
        .map_err(|e| format!("Failed to set scheme: {:?}", e))?;
    request.set_authority(Some(&authority))
        .map_err(|e| format!("Failed to set authority: {:?}", e))?;
    request.set_path_with_query(Some(&path_and_query))
        .map_err(|e| format!("Failed to set path: {:?}", e))?;

    let outgoing_body = request.body()
        .map_err(|e| format!("Failed to get outgoing body: {:?}", e))?;
    OutgoingBody::finish(outgoing_body, None)
        .map_err(|e| format!("Failed to finish outgoing body: {:?}", e))?;

    let options = RequestOptions::new();
    let timeout_nanos = timeout.as_nanos() as u64;
    options.set_connect_timeout(Some(timeout_nanos))
        .map_err(|e| format!("Failed to set connect timeout: {:?}", e))?;
    options.set_first_byte_timeout(Some(timeout_nanos))
        .map_err(|e| format!("Failed to set first byte timeout: {:?}", e))?;
    options.set_between_bytes_timeout(Some(timeout_nanos))
        .map_err(|e| format!("Failed to set between bytes timeout: {:?}", e))?;

    let future_response = outgoing_handler::handle(request, Some(options))
        .map_err(|e| format!("Failed to send request: {:?}", e))?;

    let pollable = future_response.subscribe();
    pollable.block();
    drop(pollable);

    let response = future_response
        .get()
        .ok_or("No response received")?
        .map_err(|e| format!("Response error: {:?}", e))?
        .map_err(|e| format!("HTTP error: {:?}", e))?;

    let status = response.status();

    let incoming_body = response.consume()
        .map_err(|e| format!("Failed to consume response body: {:?}", e))?;
    let input_stream = incoming_body.stream()
        .map_err(|e| format!("Failed to get input stream: {:?}", e))?;

    use crate::types::MAX_RESPONSE_SIZE;
    // Conservative upper bound: MAX_RESPONSE_SIZE (10MB) / min read (1 byte) = 10M,
    // but WASI typically reads 64KB chunks so real iterations << 100K.
    const MAX_READ_ITERATIONS: usize = 100_000;
    let mut read_iterations = 0;
    let mut body = Vec::new();
    loop {
        read_iterations += 1;
        if read_iterations > MAX_READ_ITERATIONS {
            return Err("Read loop exceeded maximum iterations".into());
        }
        let pollable = input_stream.subscribe();
        pollable.block();
        drop(pollable);

        match input_stream.read(65536) {
            Ok(chunk) => {
                if chunk.is_empty() { break; }
                if body.len() + chunk.len() > MAX_RESPONSE_SIZE {
                    return Err("Response body too large (>10MB)".into());
                }
                body.extend_from_slice(&chunk);
            }
            Err(wasi::io::streams::StreamError::Closed) => break,
            Err(e) => return Err(format!("Failed to read response: {:?}", e).into()),
        }
    }

    Ok((status, body))
}

/// Fetch form metadata from db-api (public endpoint, no auth)
///
/// Calls GET /forms/{form_id}
pub fn get_form(
    api_url: &str,
    form_id: &str,
) -> Result<FormMetadata, Box<dyn std::error::Error>> {
    let url = form_url(api_url, form_id);

    let (status, body) = get_with_timeout(&url, TIMEOUT, &[])?;

    if status != 200 {
        let snippet = String::from_utf8_lossy(&body[..body.len().min(200)]);
        return Err(format!("Failed to fetch form (status {}): {}", status, snippet).into());
    }

    let form: FormMetadata = serde_json::from_slice(&body)
        .map_err(|e| {
            let snippet = String::from_utf8_lossy(&body[..body.len().min(200)]);
            format!("Invalid form JSON: {} (body: {})", e, snippet)
        })?;

    Ok(form)
}

/// Fetch a page of encrypted form submissions from db-api
///
/// Calls GET /forms/{form_id}/submissions?offset={offset}&limit={limit} with API-Secret header.
/// Returns submissions and total count for pagination.
pub fn get_submissions(
    api_url: &str,
    form_id: &str,
    api_secret: &str,
    offset: u32,
    limit: u32,
) -> Result<SubmissionsPage, Box<dyn std::error::Error>> {
    let url = submissions_url(api_url, form_id, offset, limit);

    let (status, body) = get_with_timeout(&url, TIMEOUT, &[("API-Secret", api_secret)])?;

    if status != 200 {
        let snippet = String::from_utf8_lossy(&body[..body.len().min(200)]);
        return Err(format!("Failed to fetch submissions (status {}): {}", status, snippet).into());
    }

    let page: SubmissionsPage = serde_json::from_slice(&body)
        .map_err(|e| {
            let snippet = String::from_utf8_lossy(&body[..body.len().min(200)]);
            format!("Invalid submissions JSON: {} (body: {})", e, snippet)
        })?;

    Ok(page)
}

/// Store a new encrypted form submission to db-api
///
/// Calls POST /submissions with API-Secret header.
/// Uses chunked HTTP writes to bypass the ~4KB WASI single-write limit,
/// since encrypted blobs can exceed 4KB when hex-encoded.
pub fn create_submission(
    api_url: &str,
    form_id: &str,
    submitter_id: &str,
    encrypted_blob: &str,
    api_secret: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let url = create_submission_url(api_url);

    let body = serde_json::json!({
        "form_id": form_id,
        "submitter_id": submitter_id,
        "encrypted_blob": encrypted_blob,
    });

    let body_bytes = serde_json::to_vec(&body)?;

    let response = http_chunked::post_chunked(
        &url,
        "application/json",
        &body_bytes,
        TIMEOUT,
        Some(api_secret),
    )?;

    let status = response.status();

    if status != 200 && status != 201 {
        // Check for duplicate submission (unique constraint violation)
        if status == 409 {
            return Err("You have already submitted this form. Each account can only submit once.".into());
        }

        return Err(format!("Failed to create submission (status {})", status).into());
    }

    // Extract submission ID from response
    let response_json: serde_json::Value = serde_json::from_slice(response.body())
        .map_err(|e| format!("Invalid submission response JSON: {}", e))?;

    let submission_id = response_json["id"]
        .as_str()
        .ok_or("Missing submission ID in response")?
        .to_string();

    Ok(submission_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_form_url() {
        let url = form_url("http://db-api:4001", "daf14a0c-20f7-4199-a07b-c6456d53ef2d");
        assert_eq!(url, "http://db-api:4001/v1/forms/daf14a0c-20f7-4199-a07b-c6456d53ef2d");
    }

    #[test]
    fn test_submissions_url_no_double_v1() {
        let url = submissions_url("http://db-api:4001", "daf14a0c-20f7-4199-a07b-c6456d53ef2d", 0, 200);
        assert_eq!(
            url,
            "http://db-api:4001/v1/forms/daf14a0c-20f7-4199-a07b-c6456d53ef2d/submissions?offset=0&limit=200"
        );
        // Ensure no double /v1/ prefix
        assert!(url.matches("/v1/").count() == 1,
            "URL must not contain double /v1/ prefix");
    }

    #[test]
    fn test_submissions_url_pagination() {
        let url = submissions_url("http://localhost:4001", "abc-123", 100, 50);
        assert!(url.contains("offset=100"));
        assert!(url.contains("limit=50"));
    }

    #[test]
    fn test_create_submission_url() {
        let url = create_submission_url("http://db-api:4001");
        assert_eq!(url, "http://db-api:4001/v1/submissions");
    }
}
