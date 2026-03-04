//! Chunked HTTP client for large request bodies
//!
//! Bypasses the hardcoded ~4KB single-write limit in wasmtime-wasi's HTTP client
//! by using low-level WASI HTTP types and writing the body in chunks.
//!
//! Adapted from near-email's http_chunked.rs implementation.

use std::time::Duration;
use wasi::http::{
    outgoing_handler,
    types::{Headers, Method, OutgoingBody, OutgoingRequest, RequestOptions, Scheme},
};

/// Response from a chunked HTTP request
pub struct ChunkedResponse {
    status: u16,
    body: Vec<u8>,
}

impl ChunkedResponse {
    pub fn status(&self) -> u16 {
        self.status
    }

    pub fn body(&self) -> &[u8] {
        &self.body
    }
}

/// Send a POST request with chunked body encoding to bypass WASI 4KB write limit.
///
/// Uses low-level wasi::http types to write the body in chunks via
/// check_write/write/flush rather than a single blocking_write_and_flush.
pub fn post_chunked(
    url: &str,
    content_type: &str,
    body: &[u8],
    timeout: Duration,
    api_secret: Option<&str>,
) -> Result<ChunkedResponse, Box<dyn std::error::Error>> {
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

    let headers_list: Vec<(String, Vec<u8>)> = {
        let mut h = vec![
            ("Content-Type".to_string(), content_type.as_bytes().to_vec()),
            (
                "Content-Length".to_string(),
                body.len().to_string().as_bytes().to_vec(),
            ),
        ];
        if let Some(secret) = api_secret {
            h.push(("API-Secret".to_string(), secret.as_bytes().to_vec()));
        }
        h
    };

    let headers = Headers::from_list(&headers_list)
        .map_err(|e| format!("Failed to create headers: {:?}", e))?;

    let request = OutgoingRequest::new(headers);
    request
        .set_method(&Method::Post)
        .map_err(|e| format!("Failed to set method: {:?}", e))?;
    request
        .set_scheme(Some(&scheme))
        .map_err(|e| format!("Failed to set scheme: {:?}", e))?;
    request
        .set_authority(Some(&authority))
        .map_err(|e| format!("Failed to set authority: {:?}", e))?;
    request
        .set_path_with_query(Some(&path_and_query))
        .map_err(|e| format!("Failed to set path: {:?}", e))?;

    // Write body in chunks
    let outgoing_body = request
        .body()
        .map_err(|e| format!("Failed to get outgoing body: {:?}", e))?;

    {
        let output_stream = outgoing_body
            .write()
            .map_err(|e| format!("Failed to get output stream: {:?}", e))?;

        const MAX_POLL_ITERATIONS: usize = 100_000;
        let mut iterations = 0;
        let mut offset = 0;
        while offset < body.len() {
            iterations += 1;
            if iterations > MAX_POLL_ITERATIONS {
                return Err("Write loop exceeded maximum iterations".into());
            }
            let pollable = output_stream.subscribe();
            pollable.block();
            drop(pollable);

            let writable = output_stream
                .check_write()
                .map_err(|e| format!("check_write failed: {:?}", e))?
                as usize;

            if writable == 0 {
                continue;
            }

            let chunk_end = (offset + writable).min(body.len());
            output_stream
                .write(&body[offset..chunk_end])
                .map_err(|e| format!("write failed at offset {}: {:?}", offset, e))?;
            offset = chunk_end;

            output_stream
                .flush()
                .map_err(|e| format!("flush failed: {:?}", e))?;

            // Wait for flush to complete
            let pollable = output_stream.subscribe();
            pollable.block();
            drop(pollable);
        }

        // Must drop output_stream before finishing body
    }

    OutgoingBody::finish(outgoing_body, None)
        .map_err(|e| format!("Failed to finish outgoing body: {:?}", e))?;

    let options = RequestOptions::new();
    // Safe: 30s = 3e10 nanos, well within u64::MAX (1.8e19)
    let timeout_nanos = timeout.as_nanos() as u64;
    options
        .set_connect_timeout(Some(timeout_nanos))
        .map_err(|e| format!("Failed to set connect timeout: {:?}", e))?;
    options
        .set_first_byte_timeout(Some(timeout_nanos))
        .map_err(|e| format!("Failed to set first byte timeout: {:?}", e))?;
    options
        .set_between_bytes_timeout(Some(timeout_nanos))
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

    let incoming_body = response
        .consume()
        .map_err(|e| format!("Failed to consume response body: {:?}", e))?;

    let input_stream = incoming_body
        .stream()
        .map_err(|e| format!("Failed to get input stream: {:?}", e))?;

    use crate::types::MAX_RESPONSE_SIZE;
    const MAX_READ_ITERATIONS: usize = 100_000;
    let mut read_iterations = 0;
    let mut response_body = Vec::new();
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
                if chunk.is_empty() {
                    break;
                }
                if response_body.len() + chunk.len() > MAX_RESPONSE_SIZE {
                    return Err("Response body too large (>10MB)".into());
                }
                response_body.extend_from_slice(&chunk);
            }
            Err(wasi::io::streams::StreamError::Closed) => break,
            Err(e) => return Err(format!("Failed to read response: {:?}", e).into()),
        }
    }

    Ok(ChunkedResponse {
        status,
        body: response_body,
    })
}
