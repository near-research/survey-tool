//! Database HTTP API for near-forms.
//!
//! Provides REST endpoints for form management and encrypted submission storage.
//! Single-form MVP with hardcoded form configuration.
//!
//! ## Sections
//!
//! - **Types** — request/response structs (`Form`, `Submission`, etc.)
//! - **Rate Limiting** — per-IP token-bucket `RateLimiter`
//! - **App State** — shared `AppState` (pool, secret, limiter)
//! - **Middleware** — `require_api_secret`, `rate_limit`, `extract_client_ip`
//! - **Validation** — `validate_near_account_id`
//! - **Handlers** — `health`, `get_form`, `get_submissions`, `create_submission`
//! - **App Builder** — `build_app` assembles the axum `Router`

use axum::{
    extract::{Path, Query, Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use axum::http::{HeaderValue, Method};
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tracing::error;
use uuid::Uuid;

// ==================== Types ====================

/// Database row for a form (maps to `forms` table).
#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct Form {
    pub id: Uuid,
    pub creator_id: String,
    pub title: String,
    pub questions: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// JSON response for `GET /v1/forms/:id` (timestamps as RFC 3339 strings).
#[derive(Debug, Serialize, Deserialize)]
pub struct FormResponse {
    pub id: String,
    pub creator_id: String,
    pub title: String,
    pub questions: serde_json::Value,
    pub created_at: String,
}

/// Database row for a submission (maps to `submissions` table).
#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct Submission {
    pub id: Uuid,
    pub form_id: Uuid,
    pub submitter_id: String,
    pub encrypted_blob: String,
    pub submitted_at: chrono::DateTime<chrono::Utc>,
}

/// JSON response for a single submission (used inside `PaginatedSubmissions`).
#[derive(Debug, Serialize, Deserialize)]
pub struct SubmissionResponse {
    pub id: String,
    pub submitter_id: String,
    pub encrypted_blob: String,
    pub submitted_at: String,
}

/// Request body for `POST /v1/submissions` (hex-encoded EC01 ciphertext).
#[derive(Debug, Serialize, Deserialize)]
pub struct CreateSubmissionRequest {
    pub form_id: String,
    pub submitter_id: String,
    pub encrypted_blob: String,
}

/// Standard error envelope returned by all endpoints on failure.
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

/// Response for `GET /v1/health` — includes database connectivity status.
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub struct PaginationParams {
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

/// Paginated response for `GET /v1/forms/:id/submissions` (auth required).
#[derive(Debug, Serialize)]
pub struct PaginatedSubmissions {
    pub submissions: Vec<SubmissionResponse>,
    pub total_count: i64,
}

// ==================== Rate Limiting ====================

/// Per-IP token-bucket rate limiter for public endpoints.
/// Each IP address gets its own token bucket. Stale entries are
/// periodically cleaned up to prevent unbounded memory growth.
///
/// Note: In production behind a CDN/reverse proxy, per-IP rate limiting should
/// be done at the proxy layer (e.g., Cloudflare, nginx). This limiter is a
/// defense-in-depth measure for direct-access scenarios.
#[derive(Clone)]
pub struct RateLimiter {
    inner: Arc<std::sync::Mutex<RateLimiterInner>>,
}

struct BucketState {
    tokens: f64,
    last_check: std::time::Instant,
}

struct RateLimiterInner {
    buckets: HashMap<std::net::IpAddr, BucketState>,
    max_tokens: f64,
    refill_per_sec: f64,
    last_cleanup: std::time::Instant,
}

/// Stale buckets are cleaned up after this duration of inactivity.
const BUCKET_EXPIRY_SECS: f64 = 300.0;

/// Maximum number of tracked IPs. New IPs are rejected with 429 when full.
/// Prevents unbounded memory growth from IP-rotating attackers.
const MAX_BUCKET_COUNT: usize = 10_000;

impl RateLimiter {
    pub fn new(per_second: u32, burst: u32) -> Self {
        Self {
            inner: Arc::new(std::sync::Mutex::new(RateLimiterInner {
                buckets: HashMap::new(),
                max_tokens: burst as f64,
                refill_per_sec: per_second as f64,
                last_cleanup: std::time::Instant::now(),
            })),
        }
    }

    /// Returns true if the request from `ip` is allowed, false if rate-limited.
    fn check(&self, ip: std::net::IpAddr) -> bool {
        let Ok(mut state) = self.inner.lock() else {
            // Mutex poisoned — fail closed (deny) to prevent permanent rate-limit bypass
            tracing::warn!("Rate limiter mutex poisoned — denying request (fail-closed)");
            return false;
        };
        let now = std::time::Instant::now();

        // Periodic cleanup of stale buckets (every 60s)
        if now.duration_since(state.last_cleanup).as_secs_f64() > 60.0 {
            state.buckets.retain(|_, b| {
                now.duration_since(b.last_check).as_secs_f64() < BUCKET_EXPIRY_SECS
            });
            state.last_cleanup = now;
        }

        let max_tokens = state.max_tokens;
        let refill_per_sec = state.refill_per_sec;

        // Cap bucket count to prevent unbounded memory growth from IP rotation
        if !state.buckets.contains_key(&ip) && state.buckets.len() >= MAX_BUCKET_COUNT {
            tracing::warn!("Rate limiter bucket cap reached ({} IPs) — rejecting new IP", MAX_BUCKET_COUNT);
            return false;
        }

        let bucket = state.buckets.entry(ip).or_insert_with(|| BucketState {
            tokens: max_tokens,
            last_check: now,
        });

        let elapsed = now.duration_since(bucket.last_check).as_secs_f64();
        bucket.last_check = now;
        bucket.tokens = (bucket.tokens + elapsed * refill_per_sec).min(max_tokens);

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

// ==================== App State ====================

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub api_secret: String,
    pub rate_limiter: RateLimiter,
    pub trust_proxy: bool,
}

// ==================== Middleware ====================

/// Middleware to verify API-Secret header (constant-time comparison)
async fn require_api_secret(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    use constant_time_eq::constant_time_eq;

    let header = request
        .headers()
        .get("API-Secret")
        .and_then(|h| h.to_str().ok());

    let provided = header.map(|h| h.as_bytes()).unwrap_or(&[]);
    if !constant_time_eq(provided, state.api_secret.as_bytes()) {
        return Err((StatusCode::UNAUTHORIZED, Json(ErrorResponse {
            error: "Invalid or missing API-Secret header".to_string(),
        })));
    }

    Ok(next.run(request).await)
}

/// Extract the client IP, optionally trusting X-Forwarded-For when behind a reverse proxy.
/// Takes the first (leftmost) IP from X-Forwarded-For, falling back to the direct connection IP.
fn extract_client_ip(
    headers: &axum::http::HeaderMap,
    connect_addr: SocketAddr,
    trust_proxy: bool,
) -> std::net::IpAddr {
    if trust_proxy {
        if let Some(xff) = headers.get("X-Forwarded-For").and_then(|h| h.to_str().ok()) {
            if let Some(first_ip) = xff.split(',').next() {
                if let Ok(ip) = first_ip.trim().parse::<std::net::IpAddr>() {
                    return ip;
                }
                tracing::warn!(
                    "X-Forwarded-For first entry {:?} is not a valid IP — falling back to connection IP {}",
                    first_ip.trim(),
                    connect_addr.ip()
                );
            }
        }
    }
    connect_addr.ip()
}

/// Rate limiting middleware for public endpoints (per-IP)
async fn rate_limit(
    State(state): State<AppState>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    let client_ip = extract_client_ip(request.headers(), addr, state.trust_proxy);
    if !state.rate_limiter.check(client_ip) {
        return Err((StatusCode::TOO_MANY_REQUESTS, Json(ErrorResponse {
            error: "Too many requests".to_string(),
        })));
    }
    Ok(next.run(request).await)
}

// ==================== Validation ====================

/// Validate a string as a NEAR account ID (2-64 chars, lowercase alphanumeric + . - _).
/// Rejects implicit accounts (64-char hex strings).
pub fn validate_near_account_id(account_id: &str, field_name: &str) -> Result<(), String> {
    if account_id.len() < 2 || account_id.len() > 64 {
        return Err(format!("{} must be 2-64 characters", field_name));
    }
    if !account_id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-' || c == '_')
    {
        return Err(format!(
            "{} must contain only lowercase alphanumeric characters, dots, hyphens, and underscores",
            field_name
        ));
    }
    // Reject implicit accounts (64-char hex = ed25519 pubkey).
    // Uppercase hex is already rejected by the character whitelist above,
    // so this effectively checks for [0-9a-f]{64}.
    if account_id.len() == 64 && account_id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("{}: implicit accounts are not allowed", field_name));
    }
    Ok(())
}

// ==================== Handlers ====================

/// GET /health - Health check with database verification (no auth required)
async fn health(State(state): State<AppState>) -> Result<Json<HealthResponse>, (StatusCode, Json<ErrorResponse>)> {
    sqlx::query("SELECT 1")
        .execute(&state.pool)
        .await
        .map_err(|e| {
            error!("Health check DB ping failed: {}", e);
            (StatusCode::SERVICE_UNAVAILABLE, Json(ErrorResponse {
                error: "database unreachable".to_string(),
            }))
        })?;
    Ok(Json(HealthResponse {
        status: "ok".to_string(),
    }))
}

/// GET /forms/:form_id - Get form details (public)
async fn get_form(
    State(state): State<AppState>,
    Path(form_id_str): Path<String>,
) -> Result<Json<FormResponse>, (StatusCode, Json<ErrorResponse>)> {
    let form_id = Uuid::parse_str(&form_id_str)
        .map_err(|_| (StatusCode::BAD_REQUEST, Json(ErrorResponse {
            error: "Invalid form ID".to_string(),
        })))?;

    let form = sqlx::query_as::<_, Form>("SELECT * FROM forms WHERE id = $1")
        .bind(form_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| {
            error!("Database error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse {
                error: "Database error".to_string(),
            }))
        })?
        .ok_or((StatusCode::NOT_FOUND, Json(ErrorResponse {
            error: "Form not found".to_string(),
        })))?;

    Ok(Json(FormResponse {
        id: form.id.to_string(),
        creator_id: form.creator_id,
        title: form.title,
        questions: form.questions,
        created_at: form.created_at.to_rfc3339(),
    }))
}

/// GET /forms/:form_id/submissions - Get submissions for a form (auth required)
/// Supports pagination via ?offset=N&limit=N query params (default: offset=0, limit=200)
async fn get_submissions(
    State(state): State<AppState>,
    Path(form_id_str): Path<String>,
    Query(pagination): Query<PaginationParams>,
) -> Result<Json<PaginatedSubmissions>, (StatusCode, Json<ErrorResponse>)> {
    let form_id = Uuid::parse_str(&form_id_str)
        .map_err(|_| (StatusCode::BAD_REQUEST, Json(ErrorResponse {
            error: "Invalid form ID".to_string(),
        })))?;

    let offset = pagination.offset.unwrap_or(0).max(0);
    if offset > 1_000_000 {
        return Err((StatusCode::BAD_REQUEST, Json(ErrorResponse {
            error: "offset cannot exceed 1000000".to_string(),
        })));
    }
    let limit = pagination.limit.unwrap_or(200).clamp(1, 200);

    // Get total count for pagination metadata
    let total_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM submissions WHERE form_id = $1"
    )
    .bind(form_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| {
        error!("Database error: {}", e);
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse {
            error: "Database error".to_string(),
        }))
    })?;

    let submissions = sqlx::query_as::<_, Submission>(
        "SELECT id, form_id, submitter_id, encrypted_blob, submitted_at FROM submissions WHERE form_id = $1 ORDER BY submitted_at DESC, id DESC LIMIT $2 OFFSET $3"
    )
    .bind(form_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| {
        error!("Database error: {}", e);
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse {
            error: "Database error".to_string(),
        }))
    })?;

    let items: Vec<SubmissionResponse> = submissions
        .into_iter()
        .map(|s| SubmissionResponse {
            id: s.id.to_string(),
            submitter_id: s.submitter_id,
            encrypted_blob: s.encrypted_blob,
            submitted_at: s.submitted_at.to_rfc3339(),
        })
        .collect();

    Ok(Json(PaginatedSubmissions {
        submissions: items,
        total_count,
    }))
}

/// POST /submissions - Store a new submission (auth required)
async fn create_submission(
    State(state): State<AppState>,
    Json(payload): Json<CreateSubmissionRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<ErrorResponse>)> {
    let form_id = Uuid::parse_str(&payload.form_id)
        .map_err(|_| (StatusCode::BAD_REQUEST, Json(ErrorResponse {
            error: "Invalid form ID".to_string(),
        })))?;

    // Validate submitter_id as a NEAR account ID
    validate_near_account_id(&payload.submitter_id, "submitter_id")
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: e })))?;

    // Enforce size limit on encrypted_blob to prevent storage abuse.
    // The blob is hex-encoded, so 400 KB hex = 200 KB binary — matching the WASI module's
    // MAX_BLOB_SIZE which checks decoded byte length.
    const MAX_BLOB_HEX_SIZE: usize = 400 * 1024; // 400 KB hex = 200 KB binary
    if payload.encrypted_blob.len() > MAX_BLOB_HEX_SIZE {
        return Err((StatusCode::PAYLOAD_TOO_LARGE, Json(ErrorResponse {
            error: "encrypted_blob exceeds maximum size".to_string(),
        })));
    }

    // Validate that encrypted_blob is valid hex to reject garbage early
    // (WASI module would fail at hex::decode later, resulting in a skipped submission)
    if payload.encrypted_blob.len() % 2 != 0 {
        return Err((StatusCode::BAD_REQUEST, Json(ErrorResponse {
            error: "encrypted_blob must have even length (hex-encoded bytes)".to_string(),
        })));
    }
    // Minimum size: EC01 header (4) + ephemeral pubkey (33) + nonce (12) + Poly1305 tag (16) = 65 bytes = 130 hex chars
    if payload.encrypted_blob.len() < 130 {
        return Err((StatusCode::BAD_REQUEST, Json(ErrorResponse {
            error: "encrypted_blob too short to be a valid EC01 ciphertext".to_string(),
        })));
    }
    if !payload.encrypted_blob.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err((StatusCode::BAD_REQUEST, Json(ErrorResponse {
            error: "encrypted_blob must be valid hex".to_string(),
        })));
    }

    // Normalize hex to lowercase for consistent storage
    let normalized_blob = payload.encrypted_blob.to_ascii_lowercase();

    // Validate EC01 magic bytes (first 4 bytes = "45433031" in hex)
    if !normalized_blob.starts_with("45433031") {
        return Err((StatusCode::BAD_REQUEST, Json(ErrorResponse {
            error: "encrypted_blob must start with EC01 magic bytes".to_string(),
        })));
    }

    // Insert submission
    let submission_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO submissions (id, form_id, submitter_id, encrypted_blob, submitted_at)
         VALUES ($1, $2, $3, $4, NOW())"
    )
    .bind(submission_id)
    .bind(form_id)
    .bind(&payload.submitter_id)
    .bind(&normalized_blob)
    .execute(&state.pool)
    .await
    .map_err(|e| {
        if let Some(db_err) = e.as_database_error() {
            if let Some(code) = db_err.code().as_deref() {
                const PG_UNIQUE_VIOLATION: &str = "23505";
                const PG_FOREIGN_KEY_VIOLATION: &str = "23503";
                if code == PG_UNIQUE_VIOLATION {
                    return (StatusCode::CONFLICT, Json(ErrorResponse {
                        error: "You have already submitted this form. Each account can only submit once.".to_string(),
                    }));
                }
                if code == PG_FOREIGN_KEY_VIOLATION {
                    return (StatusCode::NOT_FOUND, Json(ErrorResponse {
                        error: "Form not found".to_string(),
                    }));
                }
            }
        }
        error!("Database error: {}", e);
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse {
            error: "Failed to create submission".to_string(),
        }))
    })?;

    Ok((StatusCode::CREATED, Json(serde_json::json!({ "id": submission_id.to_string() }))))
}

// ==================== App Builder ====================

/// Build the axum Router. When `cors_origin` is None, uses permissive CORS (for tests).
pub fn build_app(state: AppState, cors_origin: Option<&str>) -> Router {
    let cors = match cors_origin {
        Some(origin) => CorsLayer::new()
            .allow_origin(origin.parse::<HeaderValue>().expect("Invalid CORS origin"))
            .allow_methods([Method::GET])
            .allow_headers([axum::http::header::CONTENT_TYPE]),
        None => CorsLayer::permissive(),
    };

    let protected_routes = Router::new()
        .route("/forms/:form_id/submissions", get(get_submissions))
        .route("/submissions", post(create_submission))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_api_secret,
        ));

    let public_routes = Router::new()
        .route("/health", get(health))
        .route("/forms/:form_id", get(get_form))
        .layer(cors)
        .layer(middleware::from_fn_with_state(state.clone(), rate_limit));

    Router::new()
        .nest("/v1", Router::new().merge(public_routes).merge(protected_routes))
        .layer(RequestBodyLimitLayer::new(500 * 1024))
        .with_state(state)
}

// ==================== Unit Tests ====================

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== validate_near_account_id ====================

    #[test]
    fn valid_simple_accounts() {
        assert!(validate_near_account_id("alice.testnet", "f").is_ok());
        assert!(validate_near_account_id("bob.near", "f").is_ok());
        assert!(validate_near_account_id("a-b_c.near", "f").is_ok());
    }

    #[test]
    fn valid_min_length() {
        assert!(validate_near_account_id("ab", "f").is_ok());
    }

    #[test]
    fn valid_max_non_hex() {
        // 64 chars but contains non-hex char → not an implicit account
        let account = "a".repeat(63) + "z";
        assert_eq!(account.len(), 64);
        assert!(validate_near_account_id(&account, "f").is_ok());
    }

    #[test]
    fn invalid_too_short() {
        let err = validate_near_account_id("a", "submitter_id").unwrap_err();
        assert!(err.contains("2-64"));
        assert!(err.contains("submitter_id"));
    }

    #[test]
    fn invalid_too_long() {
        let account = "a".repeat(65);
        assert!(validate_near_account_id(&account, "f").is_err());
    }

    #[test]
    fn invalid_uppercase() {
        let err = validate_near_account_id("Alice.testnet", "f").unwrap_err();
        assert!(err.contains("lowercase"));
    }

    #[test]
    fn invalid_special_chars() {
        assert!(validate_near_account_id("bob@testnet", "f").is_err());
        assert!(validate_near_account_id("bob testnet", "f").is_err());
        assert!(validate_near_account_id("bob/testnet", "f").is_err());
    }

    #[test]
    fn invalid_implicit_account() {
        let hex64 = "a".repeat(64);
        let err = validate_near_account_id(&hex64, "f").unwrap_err();
        assert!(err.contains("implicit"));
    }

    #[test]
    fn valid_all_allowed_chars() {
        assert!(validate_near_account_id("a.b-c_d", "f").is_ok());
        assert!(validate_near_account_id("test123.near", "f").is_ok());
        assert!(validate_near_account_id("0123456789", "f").is_ok());
    }

    #[test]
    fn error_includes_field_name() {
        let err = validate_near_account_id("A", "my_field").unwrap_err();
        assert!(err.contains("my_field"));
    }

    // ==================== RateLimiter ====================

    #[test]
    fn rate_limiter_allows_within_burst() {
        let limiter = RateLimiter::new(1, 5);
        let ip: std::net::IpAddr = "1.2.3.4".parse().unwrap();
        for _ in 0..5 {
            assert!(limiter.check(ip));
        }
    }

    #[test]
    fn rate_limiter_denies_after_burst() {
        let limiter = RateLimiter::new(0, 3);
        let ip: std::net::IpAddr = "1.2.3.4".parse().unwrap();
        assert!(limiter.check(ip));
        assert!(limiter.check(ip));
        assert!(limiter.check(ip));
        assert!(!limiter.check(ip));
    }

    #[test]
    fn rate_limiter_independent_per_ip() {
        let limiter = RateLimiter::new(0, 2);
        let ip1: std::net::IpAddr = "1.1.1.1".parse().unwrap();
        let ip2: std::net::IpAddr = "2.2.2.2".parse().unwrap();
        assert!(limiter.check(ip1));
        assert!(limiter.check(ip1));
        assert!(!limiter.check(ip1)); // exhausted
        assert!(limiter.check(ip2)); // ip2 unaffected
        assert!(limiter.check(ip2));
        assert!(!limiter.check(ip2));
    }

    #[test]
    fn rate_limiter_bucket_cap_rejects_new_ip() {
        let limiter = RateLimiter::new(100, 100);
        // Fill all bucket slots
        for i in 0..MAX_BUCKET_COUNT as u32 {
            let ip: std::net::IpAddr = std::net::Ipv4Addr::from(i).into();
            assert!(limiter.check(ip), "IP {} should be allowed", i);
        }
        // New IP beyond cap should be rejected
        let overflow_ip: std::net::IpAddr =
            std::net::Ipv4Addr::from(MAX_BUCKET_COUNT as u32).into();
        assert!(!limiter.check(overflow_ip));
        // Existing IP still works
        let existing_ip: std::net::IpAddr = std::net::Ipv4Addr::from(0u32).into();
        assert!(limiter.check(existing_ip));
    }

    // ==================== extract_client_ip ====================

    #[test]
    fn extract_ip_direct_connection() {
        let headers = axum::http::HeaderMap::new();
        let addr: SocketAddr = "10.0.0.1:1234".parse().unwrap();
        assert_eq!(
            extract_client_ip(&headers, addr, false),
            "10.0.0.1".parse::<std::net::IpAddr>().unwrap()
        );
    }

    #[test]
    fn extract_ip_xff_trusted() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("X-Forwarded-For", "1.2.3.4, 5.6.7.8".parse().unwrap());
        let addr: SocketAddr = "10.0.0.1:1234".parse().unwrap();
        assert_eq!(
            extract_client_ip(&headers, addr, true),
            "1.2.3.4".parse::<std::net::IpAddr>().unwrap()
        );
    }

    #[test]
    fn extract_ip_xff_not_trusted() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("X-Forwarded-For", "1.2.3.4".parse().unwrap());
        let addr: SocketAddr = "10.0.0.1:1234".parse().unwrap();
        assert_eq!(
            extract_client_ip(&headers, addr, false),
            "10.0.0.1".parse::<std::net::IpAddr>().unwrap()
        );
    }

    #[test]
    fn extract_ip_xff_multiple_returns_leftmost() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            "X-Forwarded-For",
            "1.1.1.1, 2.2.2.2, 3.3.3.3".parse().unwrap(),
        );
        let addr: SocketAddr = "10.0.0.1:1234".parse().unwrap();
        assert_eq!(
            extract_client_ip(&headers, addr, true),
            "1.1.1.1".parse::<std::net::IpAddr>().unwrap()
        );
    }

    #[test]
    fn extract_ip_xff_invalid_falls_back() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("X-Forwarded-For", "not-an-ip, 1.2.3.4".parse().unwrap());
        let addr: SocketAddr = "10.0.0.1:1234".parse().unwrap();
        assert_eq!(
            extract_client_ip(&headers, addr, true),
            "10.0.0.1".parse::<std::net::IpAddr>().unwrap()
        );
    }

    #[test]
    fn extract_ip_xff_single_trusted() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("X-Forwarded-For", "1.2.3.4".parse().unwrap());
        let addr: SocketAddr = "10.0.0.1:1234".parse().unwrap();
        assert_eq!(
            extract_client_ip(&headers, addr, true),
            "1.2.3.4".parse::<std::net::IpAddr>().unwrap()
        );
    }
}
