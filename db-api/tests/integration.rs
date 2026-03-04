//! Integration tests for db-api endpoints.
//!
//! Uses `#[sqlx::test]` for per-test database isolation and `tower::ServiceExt::oneshot`
//! for idiomatic axum handler testing without starting a TCP server.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use db_api::{AppState, RateLimiter, build_app};
use http_body_util::BodyExt;
use serde_json::Value;
use sqlx::PgPool;
use std::net::SocketAddr;
use tower::ServiceExt;
use uuid::Uuid;

// ==================== Constants ====================

const TEST_API_SECRET: &str = "test-secret-that-is-at-least-32-characters-long";
const TEST_FORM_ID: &str = "daf14a0c-20f7-4199-a07b-c6456d53ef2d";

// ==================== Helpers ====================

fn test_app(pool: PgPool) -> axum::Router {
    let state = AppState {
        pool,
        api_secret: TEST_API_SECRET.to_string(),
        rate_limiter: RateLimiter::new(1000, 1000),
        trust_proxy: false,
    };
    build_app(state, None)
}

fn test_app_with_state(state: AppState) -> axum::Router {
    build_app(state, None)
}

async fn seed_form(pool: &PgPool) {
    let form_id = Uuid::parse_str(TEST_FORM_ID).unwrap();
    sqlx::query(
        "INSERT INTO forms (id, creator_id, title, questions, created_at) \
         VALUES ($1, 'alice.testnet', 'Test Form', $2, NOW()) \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(form_id)
    .bind(serde_json::json!([{"id": "q1", "text": "Favorite color?", "type": "text"}]))
    .execute(pool)
    .await
    .unwrap();
}

/// Generate a minimal valid EC01 hex blob (130+ chars).
/// Format: EC01 magic (8) + compressed pubkey (66) + nonce (24) + tag+ciphertext (32+)
fn valid_ec01_blob() -> String {
    let mut hex = String::new();
    // EC01 magic bytes
    hex.push_str("45433031");
    // Compressed pubkey (33 bytes = 66 hex): 02 + 32 zero bytes
    hex.push_str("02");
    hex.push_str(&"00".repeat(32));
    // Nonce (12 bytes = 24 hex)
    hex.push_str(&"00".repeat(12));
    // Tag + ciphertext (at least 16 bytes = 32 hex for Poly1305 tag)
    hex.push_str(&"00".repeat(16));
    assert!(hex.len() >= 130);
    hex
}

async fn insert_submission(pool: &PgPool, submitter: &str, blob: &str) {
    let form_id = Uuid::parse_str(TEST_FORM_ID).unwrap();
    sqlx::query(
        "INSERT INTO submissions (id, form_id, submitter_id, encrypted_blob, submitted_at) \
         VALUES ($1, $2, $3, $4, NOW())",
    )
    .bind(Uuid::new_v4())
    .bind(form_id)
    .bind(submitter)
    .bind(blob)
    .execute(pool)
    .await
    .unwrap();
}

/// Inject ConnectInfo extension required by the rate_limit middleware on public routes.
fn with_connect_info(req: Request<Body>) -> Request<Body> {
    let (mut parts, body) = req.into_parts();
    parts
        .extensions
        .insert(axum::extract::ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 9999))));
    Request::from_parts(parts, body)
}

async fn body_json(resp: axum::response::Response) -> Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

// ==================== GET /v1/health ====================

#[sqlx::test(migrations = "./migrations")]
async fn health_returns_ok(pool: PgPool) {
    let app = test_app(pool);
    let req = with_connect_info(
        Request::builder()
            .uri("/v1/health")
            .body(Body::empty())
            .unwrap(),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["status"], "ok");
}

// ==================== GET /v1/forms/:id ====================

#[sqlx::test(migrations = "./migrations")]
async fn get_form_returns_form(pool: PgPool) {
    seed_form(&pool).await;
    let app = test_app(pool);
    let req = with_connect_info(
        Request::builder()
            .uri(format!("/v1/forms/{}", TEST_FORM_ID))
            .body(Body::empty())
            .unwrap(),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["id"], TEST_FORM_ID);
    assert_eq!(json["creator_id"], "alice.testnet");
    assert_eq!(json["title"], "Test Form");
    assert!(json["questions"].is_array());
}

#[sqlx::test(migrations = "./migrations")]
async fn get_form_invalid_uuid(pool: PgPool) {
    let app = test_app(pool);
    let req = with_connect_info(
        Request::builder()
            .uri("/v1/forms/not-a-uuid")
            .body(Body::empty())
            .unwrap(),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert!(json["error"].as_str().unwrap().contains("Invalid form ID"));
}

#[sqlx::test(migrations = "./migrations")]
async fn get_form_not_found(pool: PgPool) {
    let app = test_app(pool);
    let missing_id = Uuid::new_v4();
    let req = with_connect_info(
        Request::builder()
            .uri(format!("/v1/forms/{}", missing_id))
            .body(Body::empty())
            .unwrap(),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ==================== GET /v1/forms/:id/submissions ====================

#[sqlx::test(migrations = "./migrations")]
async fn get_submissions_no_api_secret(pool: PgPool) {
    seed_form(&pool).await;
    let app = test_app(pool);
    let req = Request::builder()
        .uri(format!("/v1/forms/{}/submissions", TEST_FORM_ID))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "./migrations")]
async fn get_submissions_wrong_api_secret(pool: PgPool) {
    seed_form(&pool).await;
    let app = test_app(pool);
    let req = Request::builder()
        .uri(format!("/v1/forms/{}/submissions", TEST_FORM_ID))
        .header("API-Secret", "wrong-secret")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "./migrations")]
async fn get_submissions_empty_result(pool: PgPool) {
    seed_form(&pool).await;
    let app = test_app(pool);
    let req = Request::builder()
        .uri(format!("/v1/forms/{}/submissions", TEST_FORM_ID))
        .header("API-Secret", TEST_API_SECRET)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["submissions"].as_array().unwrap().len(), 0);
    assert_eq!(json["total_count"], 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn get_submissions_invalid_uuid(pool: PgPool) {
    let app = test_app(pool);
    let req = Request::builder()
        .uri("/v1/forms/bad-id/submissions")
        .header("API-Secret", TEST_API_SECRET)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "./migrations")]
async fn get_submissions_default_pagination(pool: PgPool) {
    seed_form(&pool).await;
    let blob = valid_ec01_blob();
    insert_submission(&pool, "bob.testnet", &blob).await;
    insert_submission(&pool, "carol.testnet", &blob).await;
    insert_submission(&pool, "dave.testnet", &blob).await;

    let app = test_app(pool);
    let req = Request::builder()
        .uri(format!("/v1/forms/{}/submissions", TEST_FORM_ID))
        .header("API-Secret", TEST_API_SECRET)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["submissions"].as_array().unwrap().len(), 3);
    assert_eq!(json["total_count"], 3);
}

#[sqlx::test(migrations = "./migrations")]
async fn get_submissions_custom_offset_limit(pool: PgPool) {
    seed_form(&pool).await;
    let blob = valid_ec01_blob();
    insert_submission(&pool, "bob.testnet", &blob).await;
    insert_submission(&pool, "carol.testnet", &blob).await;
    insert_submission(&pool, "dave.testnet", &blob).await;

    let app = test_app(pool);
    let req = Request::builder()
        .uri(format!(
            "/v1/forms/{}/submissions?offset=1&limit=1",
            TEST_FORM_ID
        ))
        .header("API-Secret", TEST_API_SECRET)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["submissions"].as_array().unwrap().len(), 1);
    assert_eq!(json["total_count"], 3);
}

#[sqlx::test(migrations = "./migrations")]
async fn get_submissions_limit_clamped(pool: PgPool) {
    seed_form(&pool).await;
    let blob = valid_ec01_blob();
    insert_submission(&pool, "bob.testnet", &blob).await;

    let app = test_app(pool);
    // limit=0 should be clamped to 1
    let req = Request::builder()
        .uri(format!(
            "/v1/forms/{}/submissions?limit=0",
            TEST_FORM_ID
        ))
        .header("API-Secret", TEST_API_SECRET)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["submissions"].as_array().unwrap().len(), 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn get_submissions_desc_sort_order(pool: PgPool) {
    seed_form(&pool).await;
    let blob = valid_ec01_blob();
    // Insert with slight delay to ensure ordering
    insert_submission(&pool, "first.testnet", &blob).await;
    // Use a direct insert with explicit timestamps to guarantee order
    let form_id = Uuid::parse_str(TEST_FORM_ID).unwrap();
    sqlx::query(
        "INSERT INTO submissions (id, form_id, submitter_id, encrypted_blob, submitted_at) \
         VALUES ($1, $2, 'second.testnet', $3, NOW() + interval '1 second')",
    )
    .bind(Uuid::new_v4())
    .bind(form_id)
    .bind(&blob)
    .execute(&pool)
    .await
    .unwrap();

    let app = test_app(pool);
    let req = Request::builder()
        .uri(format!("/v1/forms/{}/submissions", TEST_FORM_ID))
        .header("API-Secret", TEST_API_SECRET)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let json = body_json(resp).await;
    let subs = json["submissions"].as_array().unwrap();
    assert_eq!(subs.len(), 2);
    // DESC order: second should come first
    assert_eq!(subs[0]["submitter_id"], "second.testnet");
    assert_eq!(subs[1]["submitter_id"], "first.testnet");
}

// ==================== POST /v1/submissions ====================

fn post_submission(form_id: &str, submitter: &str, blob: &str) -> Request<Body> {
    let body = serde_json::json!({
        "form_id": form_id,
        "submitter_id": submitter,
        "encrypted_blob": blob,
    });
    Request::builder()
        .method("POST")
        .uri("/v1/submissions")
        .header("Content-Type", "application/json")
        .header("API-Secret", TEST_API_SECRET)
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn create_submission_happy_path(pool: PgPool) {
    seed_form(&pool).await;
    let app = test_app(pool);
    let req = post_submission(TEST_FORM_ID, "bob.testnet", &valid_ec01_blob());
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json = body_json(resp).await;
    assert!(json["id"].as_str().is_some());
    // Verify it's a valid UUID
    Uuid::parse_str(json["id"].as_str().unwrap()).unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn create_submission_no_api_secret(pool: PgPool) {
    seed_form(&pool).await;
    let app = test_app(pool);
    let body = serde_json::json!({
        "form_id": TEST_FORM_ID,
        "submitter_id": "bob.testnet",
        "encrypted_blob": valid_ec01_blob(),
    });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/submissions")
        .header("Content-Type", "application/json")
        // No API-Secret header
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "./migrations")]
async fn create_submission_invalid_form_id(pool: PgPool) {
    seed_form(&pool).await;
    let app = test_app(pool);
    let req = post_submission("not-a-uuid", "bob.testnet", &valid_ec01_blob());
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert!(json["error"].as_str().unwrap().contains("Invalid form ID"));
}

#[sqlx::test(migrations = "./migrations")]
async fn create_submission_form_not_found(pool: PgPool) {
    seed_form(&pool).await;
    let app = test_app(pool);
    let missing_form = Uuid::new_v4().to_string();
    let req = post_submission(&missing_form, "bob.testnet", &valid_ec01_blob());
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let json = body_json(resp).await;
    assert!(json["error"].as_str().unwrap().contains("Form not found"));
}

#[sqlx::test(migrations = "./migrations")]
async fn create_submission_uppercase_submitter(pool: PgPool) {
    seed_form(&pool).await;
    let app = test_app(pool);
    let req = post_submission(TEST_FORM_ID, "Bob.testnet", &valid_ec01_blob());
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert!(json["error"].as_str().unwrap().contains("lowercase"));
}

#[sqlx::test(migrations = "./migrations")]
async fn create_submission_special_chars_submitter(pool: PgPool) {
    seed_form(&pool).await;
    let app = test_app(pool);
    let req = post_submission(TEST_FORM_ID, "bob@testnet", &valid_ec01_blob());
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "./migrations")]
async fn create_submission_submitter_too_short(pool: PgPool) {
    seed_form(&pool).await;
    let app = test_app(pool);
    let req = post_submission(TEST_FORM_ID, "a", &valid_ec01_blob());
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert!(json["error"].as_str().unwrap().contains("2-64"));
}

#[sqlx::test(migrations = "./migrations")]
async fn create_submission_implicit_account_rejected(pool: PgPool) {
    seed_form(&pool).await;
    let app = test_app(pool);
    // 64-char hex string = implicit account (ed25519 pubkey)
    let implicit = "a".repeat(64);
    let req = post_submission(TEST_FORM_ID, &implicit, &valid_ec01_blob());
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert!(json["error"].as_str().unwrap().contains("implicit"));
}

#[sqlx::test(migrations = "./migrations")]
async fn create_submission_blob_too_large(pool: PgPool) {
    seed_form(&pool).await;
    let app = test_app(pool);
    // 400KB + 2 hex chars over the limit
    let mut blob = valid_ec01_blob();
    while blob.len() <= 400 * 1024 {
        blob.push_str("aa");
    }
    let req = post_submission(TEST_FORM_ID, "bob.testnet", &blob);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[sqlx::test(migrations = "./migrations")]
async fn create_submission_odd_hex(pool: PgPool) {
    seed_form(&pool).await;
    let app = test_app(pool);
    let mut blob = valid_ec01_blob();
    blob.push('a'); // make it odd
    let req = post_submission(TEST_FORM_ID, "bob.testnet", &blob);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert!(json["error"].as_str().unwrap().contains("even length"));
}

#[sqlx::test(migrations = "./migrations")]
async fn create_submission_blob_too_short(pool: PgPool) {
    seed_form(&pool).await;
    let app = test_app(pool);
    // Valid hex but too short (< 130 chars)
    let blob = "45433031".to_string() + &"00".repeat(50); // 108 chars
    let req = post_submission(TEST_FORM_ID, "bob.testnet", &blob);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert!(json["error"].as_str().unwrap().contains("too short"));
}

#[sqlx::test(migrations = "./migrations")]
async fn create_submission_non_hex_chars(pool: PgPool) {
    seed_form(&pool).await;
    let app = test_app(pool);
    let mut blob = valid_ec01_blob();
    // Replace last 2 chars with non-hex
    blob.replace_range(blob.len() - 2.., "zz");
    let req = post_submission(TEST_FORM_ID, "bob.testnet", &blob);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert!(json["error"].as_str().unwrap().contains("valid hex"));
}

#[sqlx::test(migrations = "./migrations")]
async fn create_submission_missing_ec01_magic(pool: PgPool) {
    seed_form(&pool).await;
    let app = test_app(pool);
    // Valid hex, correct length, but wrong magic bytes
    let blob = "00000000".to_string() + &"00".repeat(61); // 130 chars
    let req = post_submission(TEST_FORM_ID, "bob.testnet", &blob);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert!(json["error"].as_str().unwrap().contains("EC01 magic"));
}

#[sqlx::test(migrations = "./migrations")]
async fn create_submission_duplicate_conflict(pool: PgPool) {
    seed_form(&pool).await;
    let blob = valid_ec01_blob();
    // First submission succeeds
    insert_submission(&pool, "bob.testnet", &blob).await;

    let app = test_app(pool);
    let req = post_submission(TEST_FORM_ID, "bob.testnet", &blob);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let json = body_json(resp).await;
    assert!(json["error"].as_str().unwrap().contains("already submitted"));
}

#[sqlx::test(migrations = "./migrations")]
async fn create_submission_hex_normalized_to_lowercase(pool: PgPool) {
    seed_form(&pool).await;
    // Create blob with uppercase hex
    let blob = valid_ec01_blob().to_ascii_uppercase();
    let app = test_app(pool.clone());
    let req = post_submission(TEST_FORM_ID, "bob.testnet", &blob);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Verify it was stored as lowercase
    let stored: (String,) = sqlx::query_as(
        "SELECT encrypted_blob FROM submissions WHERE submitter_id = 'bob.testnet'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored.0, stored.0.to_ascii_lowercase());
}

// ==================== E2E: submit then read ====================

#[sqlx::test(migrations = "./migrations")]
async fn e2e_submit_then_read(pool: PgPool) {
    seed_form(&pool).await;
    let blob = valid_ec01_blob();

    // Submit via POST
    let app = test_app(pool.clone());
    let req = post_submission(TEST_FORM_ID, "bob.testnet", &blob);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Read via GET
    let app = test_app(pool);
    let req = Request::builder()
        .uri(format!("/v1/forms/{}/submissions", TEST_FORM_ID))
        .header("API-Secret", TEST_API_SECRET)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let subs = json["submissions"].as_array().unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0]["submitter_id"], "bob.testnet");
    assert_eq!(subs[0]["encrypted_blob"], blob);
}

// ==================== Rate Limiting ====================

#[sqlx::test(migrations = "./migrations")]
async fn rate_limit_rejects_after_burst(pool: PgPool) {
    let state = AppState {
        pool,
        api_secret: TEST_API_SECRET.to_string(),
        rate_limiter: RateLimiter::new(0, 3), // zero refill, burst of 3
        trust_proxy: false,
    };

    for i in 0..4 {
        let app = test_app_with_state(state.clone());
        let req = with_connect_info(
            Request::builder()
                .uri("/v1/health")
                .body(Body::empty())
                .unwrap(),
        );
        let resp = app.oneshot(req).await.unwrap();
        if i < 3 {
            assert_eq!(
                resp.status(),
                StatusCode::OK,
                "Request {} should succeed",
                i
            );
        } else {
            assert_eq!(
                resp.status(),
                StatusCode::TOO_MANY_REQUESTS,
                "Request {} should be rate-limited",
                i
            );
            let body: serde_json::Value = serde_json::from_slice(
                &axum::body::to_bytes(resp.into_body(), 1024).await.unwrap(),
            )
            .unwrap();
            assert_eq!(body["error"], "Too many requests");
        }
    }
}
