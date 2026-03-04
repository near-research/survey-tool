# NEAR Forms

> **Based on `near-email` from the [OutLayer examples](https://outlayer.fastnear.com/docs/examples#near-email)**.

Private, wallet-authenticated form submission system for NEAR. Form creators build forms via web UI, respondents submit with NEAR wallet signatures, answers are encrypted, and only the creator can decrypt responses via OutLayer TEE.

## Architecture

```
Form Submission Flow:
  web-ui (3000) → OutLayer TEE → db-api (4001) → PostgreSQL

Response Reading Flow:
  web-ui (3000) → OutLayer TEE (requires creator wallet) → db-api (4001) → PostgreSQL
```

**Components:**

- **Web UI** (Next.js, port 3000): Form submission & response dashboard
- **DB API** (Rust/Axum, port 4001): Internal data layer for encrypted submissions
- **PostgreSQL**: Data persistence
- **OutLayer WASI Module**: Validates encrypted submissions & decrypts for creator via TEE

## How It Works

### Submitting a Form

1. User opens form at `http://localhost:3000/forms/{form-id}`
2. Web UI fetches form metadata from db-api (`GET /v1/forms/{form-id}`)
3. User fills out form and clicks "Submit"
4. Web UI encrypts answers **client-side**: derives form public key from `NEXT_PUBLIC_MASTER_PUBLIC_KEY`, encrypts via EC01 (ECDH + ChaCha20-Poly1305)
5. `callOutLayer('SubmitForm', { encrypted_answers })` constructs a NEAR transaction (only ciphertext appears on-chain)
6. NEAR wallet prompts user to approve the transaction
7. OutLayer TEE executes WASI module with `signer_account_id = respondent`
8. WASI module:
   - Validates EC01 format (magic bytes, ephemeral pubkey, minimum size) — does **not** decrypt
   - Stores encrypted blob in db-api via `POST /v1/submissions` with API_SECRET header
9. Confirmation returned to web UI

### Viewing Responses (Creator Only)

1. Creator opens response dashboard at `http://localhost:3000/responses`
2. Creator wallet connects (via "Connect Wallet" button if not signed in)
3. `callOutLayer('ReadResponses', {})` constructs a NEAR transaction
4. NEAR wallet prompts for transaction approval (standard popup)
5. OutLayer TEE executes WASI module with `signer_account_id = creator`
6. WASI module:
   - Verifies creator matches `FORM_CREATOR_ID`
   - Fetches encrypted submissions from db-api
   - Decrypts each submission using form-specific private key
   - Returns plaintext responses to web UI
7. Web UI displays responses in an interactive table

## Key Derivation (BIP32-style)

Form submissions are encrypted with a form-specific key derived from the master secret:

```
Master Keypair (generated in OutLayer TEE):
  master_private_key → stored in PROTECTED_MASTER_KEY secret
  master_public_key  → (derived from private key when needed)

Form Public Key Derivation (used by WASI during encryption):
  form_pubkey = master_pubkey + SHA256("near-forms:v1:" + form_id) * G

Form Private Key Derivation (used by WASI during decryption):
  form_privkey = master_privkey + SHA256("near-forms:v1:" + form_id)
```

This design allows:

- Each form to have a unique encryption key
- Encryption without needing the master private key (public key derivation)
- Only the form creator to decrypt (they control the master private key via OutLayer)

## Quick Start

### With Docker Compose (Recommended)

```bash
# 1. Copy environment template
cp .env.testnet.example .env.testnet

# 2. Edit .env.testnet:
#    - FORM_CREATOR_ID: your NEAR testnet account
#    - API_SECRET: a random secret (must match in WASI module)

# 3. Start all services
docker-compose --env-file .env.testnet up -d

# 4. Open in browser
#    - Form: http://localhost:3000
#    - Responses: http://localhost:3000/responses (creator only)
```

### Manual Setup (Development)

#### 1. Start PostgreSQL

```bash
postgres -D /usr/local/var/postgres
```

#### 2. Run DB API

```bash
cd db-api
cp .env.example .env
DATABASE_URL=postgres://near_forms:password@localhost:5432/near_forms \
  FORM_CREATOR_ID=contributors.testnet \
  FORM_TITLE="My Form" \
  API_SECRET=your-shared-api-secret \
  cargo run
```

#### 3. Build & Deploy WASI Module to OutLayer

```bash
cd wasi-near-forms-ark
cargo build --target wasm32-wasip2 --release

# Upload to OutLayer dashboard:
# 1. Create project: "near-forms"
# 2. Upload WASM: target/wasm32-wasip2/release/wasi_near_forms_ark.wasm
# 3. Add OutLayer secrets:
#    - PROTECTED_MASTER_KEY: (generate 32-byte hex in dashboard)
#    - DATABASE_API_URL: http://db-api:4001
#    - DATABASE_API_SECRET: (same as API_SECRET above)
#    - FORM_CREATOR_ID: (same as db-api FORM_CREATOR_ID)
```

#### 4. Run Web UI

```bash
cd web-ui
npm install

NEXT_PUBLIC_NETWORK_ID=testnet \
  NEXT_PUBLIC_PROJECT_ID=agency.testnet/near-forms \
  NEXT_PUBLIC_DATABASE_API_URL=http://localhost:4001 \
  NEXT_PUBLIC_FORM_ID=daf14a0c-20f7-4199-a07b-c6456d53ef2d \
  npm run dev
```

## Environment Variables

### DB API (Rust/Axum)

| Variable               | Required | Description                                          |
| ---------------------- | -------- | ---------------------------------------------------- |
| `DATABASE_URL`         | Yes      | PostgreSQL connection string                         |
| `API_PORT`             | No       | Port (default: `4001`)                               |
| `API_SECRET`           | Yes      | Shared secret with WASI module                       |
| `FORM_CREATOR_ID`      | Yes      | NEAR account ID of form creator                      |
| `FORM_TITLE`           | No       | Display title of the form (default: `My Form`)       |
| `CORS_ALLOWED_ORIGIN`  | Yes      | Allowed CORS origin (e.g., `https://your-web-ui.app`) — panics without it in production |
| `DATABASE_POOL_SIZE`   | No       | PostgreSQL connection pool size (default: `5`)       |
| `RATE_LIMIT_RPS`       | No       | Rate limit requests per second (default: `10`)       |
| `RATE_LIMIT_BURST`     | No       | Rate limit burst size (default: `30`)                |
| `RATE_LIMIT_TRUST_PROXY` | No    | Trust `X-Forwarded-For` header (default: `false`)    |

### WASI Module (OutLayer Secrets — NOT .env)

Set these in the OutLayer dashboard:

| Secret Name            | Type         | Value                                         |
| ---------------------- | ------------ | --------------------------------------------- |
| `PROTECTED_MASTER_KEY` | Hex 32 bytes | Secp256k1 private key (generate in dashboard) |
| `DATABASE_API_URL`     | Manual       | `http://db-api:4001`                          |
| `DATABASE_API_SECRET`  | Manual       | Same as API_SECRET in db-api                  |
| `FORM_CREATOR_ID`      | Manual       | Same as db-api FORM_CREATOR_ID                |

### Web UI (Next.js)

| Variable                         | Required | Description                                                                        |
| -------------------------------- | -------- | ---------------------------------------------------------------------------------- |
| `NEXT_PUBLIC_NETWORK_ID`         | Yes      | NEAR network: `testnet` or `mainnet`                                               |
| `NEXT_PUBLIC_PROJECT_ID`         | Yes      | OutLayer project ID (e.g., `account.testnet/near-forms`)                           |
| `NEXT_PUBLIC_OUTLAYER_CONTRACT`  | No       | OutLayer contract (`outlayer.near` for mainnet, `outlayer.testnet` for testnet)     |
| `NEXT_PUBLIC_DATABASE_API_URL`   | Yes      | URL to db-api (e.g., `http://db-api:4001` in Docker)                               |
| `NEXT_PUBLIC_FORM_ID`            | Yes      | Same FORM_ID as db-api                                                             |
| `NEXT_PUBLIC_MASTER_PUBLIC_KEY`  | Yes      | Compressed secp256k1 public key (66-char hex) for client-side encryption           |
| `NEXT_PUBLIC_OUTLAYER_DEPOSIT_NEAR` | No    | NEAR deposit per OutLayer transaction (default: `0.025`)                           |
| `NEXT_PUBLIC_SECRETS_PROFILE`    | No       | OutLayer secrets configuration profile (default: `default`)                        |
| `NEXT_PUBLIC_SECRETS_ACCOUNT_ID` | No       | OutLayer secrets scoped account ID                                                 |
| `NEXT_PUBLIC_USE_SECRETS`        | No       | Enable OutLayer secrets configuration (default: `true`)                            |

## Security Model

- **Form responses**: Encrypted with EC01 (ECDH + ChaCha20-Poly1305), only creator can decrypt
- **Master key**: Stored securely in OutLayer TEE (PROTECTED_MASTER_KEY), never exposed to db-api or web UI
- **Respondent privacy**: Answers encrypted before submission; db-api only stores ciphertext
- **Creator authentication**: NEAR wallet transaction signature via OutLayer TEE (no additional fees)
- **API authentication**: API_SECRET header required for all db-api requests from WASI module

## Production Deployment

1. **PostgreSQL**: Running on stable host/cloud service
2. **DB API**: Deploy Rust binary on internal network (port 4001, internal only)
3. **Web UI**: Deploy Next.js build to CDN/static host (port 3000, public)
4. **WASI Module**: Deploy to OutLayer via CLI with secrets configured

See [CLAUDE.md](./CLAUDE.md) for detailed development documentation and common troubleshooting.
