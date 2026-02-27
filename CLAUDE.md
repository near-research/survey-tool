# CLAUDE.md - near-forms

## Project Overview

**near-forms** - private, wallet-authenticated form submission system for NEAR. Form creators build forms via web UI, respondents submit with NEAR wallet signatures, answers are encrypted, and only the creator can decrypt responses via OutLayer TEE.

## Architecture

```
┌─────────────┐                                            ┌─────────────┐
│   Web UI    │────────▶ OutLayer (Transaction Mode)  ────▶│  WASI TEE   │
│  (Next.js)  │         SubmitForm & ReadResponses         │   (Ark)     │
└─────────────┘                                            └─────────────┘
     (3000)                    ▲                                 │
                               │                                 │
                               └─────────────┬──────────────────┘
                                             │
                                 ┌───────────────────────┐
                                 │   Internal Services   │
                                 │  ┌──────────────┐    │
                                 │  │   DB API     │    │
                                 │  │(Rust/Axum)   │    │
                                 │  └────────┬─────┘    │
                                 │       (4001)         │
                                 │   ┌─────────────┐    │
                                 │   │ PostgreSQL  │    │
                                 │   └─────────────┘    │
                                 └───────────────────────┘
```

**Data Flow:**

- Web UI (port 3000): Form submission & creator response dashboard (public)
- OutLayer (TEE): Implicit authentication via blockchain transactions, runs WASI module
- WASI Module: Validates encrypted submissions (SubmitForm), decrypts for creator (ReadResponses)
- DB API (port 4001): Internal data layer, stores encrypted submissions, requires API_SECRET

## Components

| Directory              | Description                                                                    | Language           |
| ---------------------- | ------------------------------------------------------------------------------ | ------------------ |
| `wasi-near-forms-ark/` | WASI module - validates encrypted submissions, decrypts for creator via OutLayer TEE | Rust               |
| `db-api/`              | Internal HTTP API for forms database, stores encrypted submissions             | Rust/Axum          |
| `web-ui/`              | Frontend: form submission page + creator response dashboard                    | TypeScript/Next.js |

## Critical Rules

### NEVER Do

- **Don't modify encryption logic** without understanding EC01 format (magic + ephemeral_pubkey + nonce + ChaCha20-Poly1305)
- **Don't change the form ID** constant — it's shared across db-api and WASI module
- **Don't expose private keys** in logs or responses
- **Don't modify key derivation prefix** ("near-forms:v1:") — it's hardcoded in WASI module
- **Don't skip API_SECRET validation** between WASI module and db-api
- **Don't modify dependency versions** in Cargo.toml files (WASI ecosystem is version-sensitive)

### Security Model

1. **Master key** stored in TEE (OutLayer), never exposed in db-api
2. **Form keys** derived: `form_pubkey = master_pubkey + SHA256("near-forms:v1:" + form_id) * G`
3. **EC01 encryption** (ECDH + ChaCha20-Poly1305) for form submissions
4. **Ephemeral keys** generated per submission for perfect forward secrecy
5. **API_SECRET header** required for all db-api requests (shared secret between WASI module and db-api)

### Code Patterns

```rust
// CORRECT - propagate errors to user
return Err(format!("Form submission failed: {}", e).into());

// WRONG - silent failure, user sees nothing
tracing::warn!("Form submission error");
return Ok(());
```

## Commands

```bash
# Build all components
cd wasi-near-forms-ark && cargo build --release --target wasm32-wasip2
cd db-api && cargo build --release
cd web-ui && npm install && npm run build

# Run locally with docker-compose
docker-compose -f docker-compose.yml --env-file .env.testnet up -d

# Or run individually
cd db-api && cargo run                    # Port 4001 (internal only)
cd web-ui && npm run dev                  # Port 3000 (public UI)

# Deploy WASI to OutLayer
cargo run -p upload-fastfs -- \
  --project-id "agency.testnet/near-forms" \
  --file wasi-near-forms-ark/target/wasm32-wasip2/release/wasi_near_forms_ark.wasm
```

## Environment Variables

### DB API (Rust/Axum)

| Variable          | Required | Description                                             |
| ----------------- | -------- | ------------------------------------------------------- |
| `DATABASE_URL`    | Yes      | PostgreSQL connection string                            |
| `API_PORT`        | No       | Port (default: `4001`)                                  |
| `API_SECRET`      | Yes      | Shared secret with WASI module                          |
| `FORM_CREATOR_ID` | Yes      | NEAR account ID of form creator (e.g., `alice.testnet`) |
| `FORM_TITLE`      | No       | Display title of the form (default: `My Form`)          |

### WASI Module (OutLayer Secrets)

Set these in OutLayer dashboard (NOT in .env):
| Secret Name | Type | Value |
|-------------|------|-------|
| `PROTECTED_MASTER_KEY` | Hex 32 bytes | Secp256k1 private key (generate in dashboard) |
| `DATABASE_API_URL` | Manual | `http://db-api:4001` (internal Docker URL) |
| `DATABASE_API_SECRET` | Manual | Same as API_SECRET in db-api |
| `FORM_CREATOR_ID` | Manual | Same as db-api FORM_CREATOR_ID |

### Web UI (Next.js)

| Variable                         | Default                            | Description                                                                        |
| -------------------------------- | ---------------------------------- | ---------------------------------------------------------------------------------- |
| `NEXT_PUBLIC_NETWORK_ID`         | `mainnet`                          | NEAR network (`testnet` or `mainnet`)                                              |
| `NEXT_PUBLIC_PROJECT_ID`         | -                                  | OutLayer project ID (e.g., `account.testnet/near-forms`)                           |
| `NEXT_PUBLIC_OUTLAYER_CONTRACT`  | `outlayer.near`/`outlayer.testnet` | OutLayer contract ID (`outlayer.near` for mainnet, `outlayer.testnet` for testnet) |
| `NEXT_PUBLIC_DATABASE_API_URL`   | `http://localhost:4001`            | URL to db-api (e.g., `http://db-api:4001` in Docker)                               |
| `NEXT_PUBLIC_FORM_ID`            | -                                  | Same FORM_ID as db-api (must match FORM_ID env var)                                |
| `NEXT_PUBLIC_SECRETS_PROFILE`    | `default`                          | OutLayer secrets configuration profile                                             |
| `NEXT_PUBLIC_SECRETS_ACCOUNT_ID` | (empty)                            | OutLayer secrets scoped account ID (optional)                                      |
| `NEXT_PUBLIC_USE_SECRETS`        | `true`                             | Enable OutLayer secrets configuration                                              |
| `NEXT_PUBLIC_MASTER_PUBLIC_KEY`  | -                                  | Compressed secp256k1 public key (66-char hex, starts with 02/03) for client-side encryption |

## Key Files

### WASI Module (Rust)

- `src/main.rs` - Action dispatcher: ReadResponses (fetch & decrypt), SubmitForm (validate EC01 format & store)
- `src/types.rs` - API types: Input, Output, Response, EncryptedSubmission
- `src/crypto.rs` - EC01 decryption (ECDH + ChaCha20-Poly1305) with BIP32 key derivation
- `src/db.rs` - HTTP client to fetch/store submissions from db-api

### DB API (Rust)

- `src/main.rs` - Axum router with 4 endpoints, API_SECRET middleware, form seeding
- `migrations/20260226000001_forms_schema.sql` - PostgreSQL schema: forms + submissions tables

### Web UI (Next.js)

- `src/lib/near.ts` - NEAR wallet integration, OutLayer transaction-based calls (callOutLayer)
- `src/pages/index.tsx` - Form listing page (public)
- `src/lib/crypto.ts` - Client-side EC01 encryption (secp256k1 ECDH + ChaCha20-Poly1305) matching WASI decryption
- `src/pages/forms/[id].tsx` - Form submission page (public), encrypts client-side then calls `callOutLayer('SubmitForm', { encrypted_answers })`
- `src/pages/responses.tsx` - Creator dashboard, calls `callOutLayer('ReadResponses', {})`, displays decrypted responses in table

## Data Flow

### Submitting a Form (Respondent)

1. User opens form page (web-ui, port 3000)
2. Page fetches form title & questions from db-api GET /forms/{form_id}
3. User fills out answers
4. Web-ui encrypts answers client-side: derives form public key from `NEXT_PUBLIC_MASTER_PUBLIC_KEY`, encrypts via EC01 (ECDH + ChaCha20-Poly1305)
5. User clicks submit → triggers `callOutLayer('SubmitForm', { encrypted_answers })` (only ciphertext appears on-chain)
6. Wallet prompts for transaction approval, signs with user's account
7. OutLayer executes WASI module with `env::signer_account_id()` set to respondent account
8. WASI module validates EC01 format (magic bytes, ephemeral pubkey, min size) — does NOT decrypt
9. WASI module calls db-api POST /submissions with API_SECRET header (internal Docker call)
10. db-api stores encrypted submission in PostgreSQL with submitter_id and timestamp

### Reading Responses (Form Creator)

1. Creator opens responses page (web-ui, port 3000)
2. Web-ui checks wallet connection
3. Web-ui calls `callOutLayer('ReadResponses', {})` → constructs NEAR transaction to OutLayer
4. Wallet prompts for transaction approval (normal signature popup), creator approves
5. OutLayer executes WASI module with `env::signer_account_id()` set to creator account
6. WASI module verifies creator account matches FORM_CREATOR_ID (authorization check)
7. WASI module fetches encrypted submissions from db-api using DATABASE_API_SECRET header
8. WASI module derives form private key: `form_privkey = master_privkey + SHA256("near-forms:v1:" + form_id)`
9. WASI module decrypts each submission using EC01 decryption
10. Returns Vec<Response> with decrypted {submitter_id, answers, submitted_at}
11. Web-ui displays responses in interactive table (filterable, sortable columns)

## Testing

### Local Integration Test

```bash
# 1. Start all services
docker-compose -f docker-compose.testnet.yml --env-file .env.testnet up -d

# 2. Wait for db-api to seed the form (check logs)
docker logs db-api

# 3. Check form exists (using actual FORM_ID)
curl http://localhost:4001/forms/daf14a0c-20f7-4199-a07b-c6456d53ef2d

# 4. Test submission via web-ui (requires browser with NEAR testnet wallet)
# Open http://localhost:3000/forms/your-form-id in browser
# Fill form, click submit → wallet popup → sign transaction

# 5. Test responses page via web-ui (requires browser)
# Open http://localhost:3000/responses → wallet popup → view decrypted responses

# 6. Check database directly
psql "postgresql://near_forms:password@localhost:5432/near_forms" \
  -c "SELECT submitter_id, submitted_at FROM submissions LIMIT 5;"
```

### Component-Specific Tests

```bash
# Test db-api alone
cd db-api
cargo test

# Test WASI module build
cd wasi-near-forms-ark
cargo build --target wasm32-wasip2 --release
```

## Common Issues

1. **"API_SECRET not found"** - Ensure API_SECRET env var is set in db-api, and DATABASE_API_SECRET in WASI module (OutLayer dashboard)
2. **"Invalid signature"** - Verify NEAR wallet signed the correct transaction and account is correct
3. **"Decryption failed"** - Check PROTECTED_MASTER_KEY in OutLayer matches the form key being used
4. **"Form not found"** - Verify FORM_ID matches across db-api and WASI module
5. **"Creator not authorized"** - Ensure account calling ReadResponses matches FORM_CREATOR_ID in db-api
6. **Port conflicts** - Verify ports 3000 (web-ui) and 4001 (db-api) are available
7. **Changing survey questions** - Edit `db-api/seed/questions.json` then rebuild and redeploy db-api (questions are embedded at compile-time via `include_str!`)
