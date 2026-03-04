//! Database HTTP API for near-forms — binary entrypoint.

use db_api::{build_app, validate_near_account_id, AppState, RateLimiter};
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::{env, net::SocketAddr, time::Duration};
use tracing::info;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use uuid::Uuid;

// Compile-time embed of the question definitions
const QUESTIONS_JSON: &str = include_str!("../seed/questions.json");

/// Fixed form ID — must match `FORM_ID` in `wasi-near-forms-ark/src/main.rs`.
const FORM_ID: &str = "daf14a0c-20f7-4199-a07b-c6456d53ef2d";

/// Initialize database and seed hardcoded form
async fn init_database(pool: &PgPool) -> Result<(), Box<dyn std::error::Error>> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await?;

    let form_id = Uuid::parse_str(FORM_ID)?;
    let creator_id = env::var("FORM_CREATOR_ID")
        .expect("FORM_CREATOR_ID environment variable not set");
    validate_near_account_id(&creator_id, "FORM_CREATOR_ID")
        .map_err(|e| format!("Invalid FORM_CREATOR_ID: {}", e))?;
    let title = env::var("FORM_TITLE").unwrap_or_else(|_| {
        tracing::warn!("FORM_TITLE not set, using default 'My Form'");
        "My Form".to_string()
    });

    let questions: serde_json::Value = serde_json::from_str(QUESTIONS_JSON)
        .map_err(|e| format!("Invalid questions.json: {}", e))?;

    // Check if form exists with a different creator before upserting
    let existing_creator: Option<String> = sqlx::query_scalar(
        "SELECT creator_id FROM forms WHERE id = $1"
    )
    .bind(form_id)
    .fetch_optional(pool)
    .await?;

    // Use PostgreSQL xmax trick: xmax=0 means freshly inserted, xmax>0 means updated existing row
    let was_inserted: bool = sqlx::query_scalar(
        "INSERT INTO forms (id, creator_id, title, questions, created_at) VALUES ($1, $2, $3, $4, NOW())
         ON CONFLICT (id) DO UPDATE SET creator_id = EXCLUDED.creator_id, title = EXCLUDED.title, questions = EXCLUDED.questions
         RETURNING (xmax = 0)"
    )
    .bind(form_id)
    .bind(&creator_id)
    .bind(&title)
    .bind(&questions)
    .fetch_one(pool)
    .await?;

    if was_inserted {
        info!("Seeded new form {} with creator={}, title={}", form_id, creator_id, title);
    } else {
        if let Some(ref old) = existing_creator {
            if old != &creator_id {
                tracing::warn!(
                    "Form {} creator changed from '{}' to '{}' — verify FORM_CREATOR_ID is correct",
                    form_id, old, creator_id
                );
            }
        }
        info!("Updated existing form {} with creator={}, title={}", form_id, creator_id, title);
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

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
    if api_secret.len() < 32 {
        panic!("API_SECRET must be at least 32 characters (got {})", api_secret.len());
    }

    let pool_size: u32 = env::var("DATABASE_POOL_SIZE")
        .unwrap_or_else(|_| "5".to_string())
        .parse()
        .expect("DATABASE_POOL_SIZE must be a valid number");
    if pool_size == 0 {
        panic!("DATABASE_POOL_SIZE must be > 0");
    }
    let pool = PgPoolOptions::new()
        .max_connections(pool_size)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&database_url)
        .await
        .expect("Failed to connect to database");

    init_database(&pool)
        .await
        .expect("Failed to initialize database");

    let rate_limit_rps: u32 = env::var("RATE_LIMIT_RPS")
        .unwrap_or_else(|_| "10".to_string())
        .parse()
        .expect("RATE_LIMIT_RPS must be a valid number");
    let rate_limit_burst: u32 = env::var("RATE_LIMIT_BURST")
        .unwrap_or_else(|_| "30".to_string())
        .parse()
        .expect("RATE_LIMIT_BURST must be a valid number");
    if rate_limit_rps == 0 {
        panic!("RATE_LIMIT_RPS must be > 0");
    }
    if rate_limit_burst == 0 {
        panic!("RATE_LIMIT_BURST must be > 0");
    }
    let rate_limiter = RateLimiter::new(rate_limit_rps, rate_limit_burst);
    let trust_proxy: bool = env::var("RATE_LIMIT_TRUST_PROXY")
        .unwrap_or_else(|_| "false".to_string())
        .parse()
        .expect("RATE_LIMIT_TRUST_PROXY must be 'true' or 'false'");
    info!(
        "Rate limiting: {} req/s, burst {}, trust_proxy={}",
        rate_limit_rps, rate_limit_burst, trust_proxy
    );

    let state = AppState {
        pool,
        api_secret,
        rate_limiter,
        trust_proxy,
    };

    let cors_origin = env::var("CORS_ALLOWED_ORIGIN")
        .expect("CORS_ALLOWED_ORIGIN must be set (e.g., http://localhost:3000 for local dev, https://forms.example.com for production)");
    // Validate early: CORS origin is user-provided and must be a valid HTTP header value.
    // build_app() would panic at request time if this is invalid — fail at startup instead.
    cors_origin.parse::<axum::http::HeaderValue>().unwrap_or_else(|_| {
        panic!("CORS_ALLOWED_ORIGIN is not a valid header value (check for invalid characters)")
    });
    info!("CORS restricted to origin: {}", cors_origin);

    let app = build_app(state, Some(&cors_origin));

    let addr = SocketAddr::from(([0, 0, 0, 0], api_port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind to port");

    info!("Server running on {}", addr);

    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("Server error");

    info!("Server shut down gracefully");
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to install CTRL+C signal handler");
    info!("Received shutdown signal, draining in-flight requests...");
}
