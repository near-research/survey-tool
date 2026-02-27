//! Database HTTP API for near-forms
//!
//! Provides REST endpoints for form management and submission storage.
//! Single-form MVP with hardcoded form configuration.

// Compile-time embed of the question definitions
const QUESTIONS_JSON: &str = include_str!("../seed/questions.json");

use axum::{
    extract::{Path, Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::{postgres::PgPoolOptions, FromRow, PgPool};
use std::{env, net::SocketAddr};
use tower_http::cors::{Any, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tracing::{error, info};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use uuid::Uuid;

// ==================== Hardcoded Single Form ====================

/// Fixed form ID (must match WASI module FORM_ID)
const FORM_ID: &str = "daf14a0c-20f7-4199-a07b-c6456d53ef2d";

// ==================== Types ====================

#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct Form {
    pub id: Uuid,
    pub creator_id: String,
    pub title: String,
    pub questions: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FormResponse {
    pub id: String,
    pub creator_id: String,
    pub title: String,
    pub questions: serde_json::Value,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct Submission {
    pub id: Uuid,
    pub form_id: Uuid,
    pub submitter_id: String,
    pub encrypted_blob: String,
    pub submitted_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SubmissionResponse {
    pub id: String,
    pub submitter_id: String,
    pub encrypted_blob: String,
    pub submitted_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateSubmissionRequest {
    pub form_id: String,
    pub submitter_id: String,
    pub encrypted_blob: String,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
}

// ==================== App State ====================

#[derive(Clone)]
pub struct AppState {
    pool: PgPool,
    api_secret: String,
}

// ==================== Middleware ====================

/// Middleware to verify API-Secret header (constant-time comparison)
async fn require_api_secret(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    use constant_time_eq::constant_time_eq;

    let header = request
        .headers()
        .get("API-Secret")
        .and_then(|h| h.to_str().ok());

    let provided = header.map(|h| h.as_bytes()).unwrap_or(&[]);
    if !constant_time_eq(provided, state.api_secret.as_bytes()) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(next.run(request).await)
}

// ==================== Handlers ====================

/// GET /health - Health check (no auth required)
async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
    })
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

/// GET /forms/:form_id/submissions - Get all submissions for a form (auth required)
async fn get_submissions(
    State(state): State<AppState>,
    Path(form_id_str): Path<String>,
) -> Result<Json<Vec<SubmissionResponse>>, (StatusCode, Json<ErrorResponse>)> {
    let form_id = Uuid::parse_str(&form_id_str)
        .map_err(|_| (StatusCode::BAD_REQUEST, Json(ErrorResponse {
            error: "Invalid form ID".to_string(),
        })))?;

    let submissions = sqlx::query_as::<_, Submission>(
        "SELECT id, form_id, submitter_id, encrypted_blob, submitted_at FROM submissions WHERE form_id = $1 ORDER BY submitted_at DESC"
    )
    .bind(form_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| {
        error!("Database error: {}", e);
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse {
            error: "Database error".to_string(),
        }))
    })?;

    let responses: Vec<SubmissionResponse> = submissions
        .into_iter()
        .map(|s| SubmissionResponse {
            id: s.id.to_string(),
            submitter_id: s.submitter_id,
            encrypted_blob: s.encrypted_blob,
            submitted_at: s.submitted_at.to_rfc3339(),
        })
        .collect();

    Ok(Json(responses))
}

/// POST /submissions - Store a new submission (auth required)
async fn create_submission(
    State(state): State<AppState>,
    Json(payload): Json<CreateSubmissionRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let form_id = Uuid::parse_str(&payload.form_id)
        .map_err(|_| (StatusCode::BAD_REQUEST, Json(ErrorResponse {
            error: "Invalid form ID".to_string(),
        })))?;

    // Validate submitter_id (NEAR accounts: non-empty, max 64 chars)
    if payload.submitter_id.is_empty() || payload.submitter_id.len() > 64 {
        return Err((StatusCode::BAD_REQUEST, Json(ErrorResponse {
            error: "submitter_id must be 1-64 characters".to_string(),
        })));
    }

    // Verify form exists
    let form_exists = sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM forms WHERE id = $1)")
        .bind(form_id)
        .fetch_one(&state.pool)
        .await
        .map_err(|e| {
            error!("Database error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse {
                error: "Database error".to_string(),
            }))
        })?;

    if !form_exists {
        return Err((StatusCode::NOT_FOUND, Json(ErrorResponse {
            error: "Form not found".to_string(),
        })));
    }

    // Enforce size limit on encrypted_blob to prevent storage abuse
    const MAX_BLOB_SIZE: usize = 200 * 1024; // 200 KB (4× WASI cap after hex encoding)
    if payload.encrypted_blob.len() > MAX_BLOB_SIZE {
        return Err((StatusCode::PAYLOAD_TOO_LARGE, Json(ErrorResponse {
            error: "encrypted_blob exceeds maximum size".to_string(),
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
    .bind(&payload.encrypted_blob)
    .execute(&state.pool)
    .await
    .map_err(|e| {
        // Check for unique constraint violation (PostgreSQL error code 23505)
        if let Some(db_err) = e.as_database_error() {
            if db_err.code().as_deref() == Some("23505") {
                return (StatusCode::CONFLICT, Json(ErrorResponse {
                    error: "You have already submitted this form. Each account can only submit once.".to_string(),
                }));
            }
        }
        error!("Database error: {}", e);
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse {
            error: "Failed to create submission".to_string(),
        }))
    })?;

    Ok(Json(serde_json::json!({ "id": submission_id.to_string() })))
}

// ==================== Initialization ====================

/// Initialize database and seed hardcoded form
async fn init_database(pool: &PgPool) -> Result<(), Box<dyn std::error::Error>> {
    // Run migrations
    sqlx::migrate!("./migrations")
        .run(pool)
        .await?;

    // Seed hardcoded form
    let form_id = Uuid::parse_str(FORM_ID)?;
    let creator_id = env::var("FORM_CREATOR_ID")
        .expect("FORM_CREATOR_ID environment variable not set");
    let title = env::var("FORM_TITLE").unwrap_or_else(|_| {
        tracing::warn!("FORM_TITLE not set, using default 'My Form'");
        "My Form".to_string()
    });

    // Parse embedded questions JSON and validate
    let questions: serde_json::Value = serde_json::from_str(QUESTIONS_JSON)
        .map_err(|e| format!("Invalid questions.json: {}", e))?;

    // Upsert form: insert if new, update creator_id, title, and questions if exists
    sqlx::query(
        "INSERT INTO forms (id, creator_id, title, questions, created_at) VALUES ($1, $2, $3, $4, NOW())
         ON CONFLICT (id) DO UPDATE SET creator_id = EXCLUDED.creator_id, title = EXCLUDED.title, questions = EXCLUDED.questions"
    )
    .bind(form_id)
    .bind(&creator_id)
    .bind(&title)
    .bind(&questions)
    .execute(pool)
    .await?;

    info!(
        "Seeded/updated form {} with creator={}, title={}",
        form_id, creator_id, title
    );

    Ok(())
}

// ==================== Main ====================

#[tokio::main]
async fn main() {
    // Load environment variables first so .env RUST_LOG is available to tracing
    dotenvy::dotenv().ok();

    // Initialize tracing with RUST_LOG environment variable support
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt::layer())
        .init();

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let api_port = env::var("API_PORT")
        .unwrap_or_else(|_| "4001".to_string())
        .parse::<u16>()
        .expect("API_PORT must be a valid port number");
    let api_secret =
        env::var("API_SECRET").expect("API_SECRET environment variable not set");
    if api_secret.is_empty() {
        panic!("API_SECRET must not be empty");
    }

    // Create database pool
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .expect("Failed to connect to database");

    // Initialize database
    init_database(&pool)
        .await
        .expect("Failed to initialize database");

    let state = AppState {
        pool,
        api_secret,
    };

    // Build router
    let protected_routes = Router::new()
        .route("/forms/:form_id/submissions", get(get_submissions))
        .route("/submissions", post(create_submission))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_api_secret,
        ));

    let public_routes = Router::new()
        .route("/health", get(health))
        .route("/forms/:form_id", get(get_form));

    let app = Router::new()
        .merge(public_routes)
        .merge(protected_routes)
        .layer(RequestBodyLimitLayer::new(250 * 1024))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any)
        )
        .with_state(state);

    // Run server
    let addr = SocketAddr::from(([0, 0, 0, 0], api_port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind to port");

    info!("Server running on {}", addr);

    axum::serve(listener, app)
        .await
        .expect("Server error");
}
