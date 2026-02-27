# Deployment Guide for near-forms (Railway)

Railway provides managed hosting with automatic HTTPS, environment variable management, and integrated PostgreSQL — no VPS, nginx, or systemd needed.

## Requirements

- GitHub account (for connecting repo to Railway)
- Railway account (free tier available at [railway.app](https://railway.app))
- NEAR testnet or mainnet account for the form creator
- Rust toolchain (local — for building WASI module)

## Step 1: Prepare Secrets Offline

Generate secrets securely before entering Railway dashboard:

```bash
# Generate PROTECTED_MASTER_KEY (32 random bytes as hex)
openssl rand -hex 32

# Generate API_SECRET
openssl rand -hex 16
```

Store these safely — you'll enter them into Railway and OutLayer dashboard.

## Step 2: Create Railway Project

1. Go to [railway.app](https://railway.app) and log in
2. Click **New Project**
3. Select **Deploy from GitHub repo** (or paste your repo URL)
4. Select the `near-forms` repository
5. Railway will detect the Dockerfiles and create three services:
   - `postgres` — managed database
   - `db-api` — Rust service (port 4001)
   - `web-ui` — Next.js service (port 3000)

## Step 3: Configure PostgreSQL

Railway auto-detects the postgres service from docker-compose.yml. To use Railway's managed Postgres instead:

1. In your Railway project, click **Add Service** → **Database** → **PostgreSQL**
2. Railway creates a `DATABASE_URL` environment variable automatically
3. The `db-api` service will automatically use this `DATABASE_URL`

(Optional: keep docker-compose postgres if you prefer — Railway will use it. But managed Postgres is simpler.)

## Step 4: Set Environment Variables in Railway

For each service, click on it in Railway dashboard and set **Variables**:

### db-api

```
DATABASE_URL=<auto-set by Railway Postgres addon, or your postgres connection string>
API_PORT=4001
API_SECRET=<your generated API_SECRET from Step 1>
FORM_ID=daf14a0c-20f7-4199-a07b-c6456d53ef2d
FORM_CREATOR_ID=contributors.testnet
FORM_TITLE=House of Stake Governance Survey
```

**Note on Questions:** Questions are embedded at compile-time from `db-api/seed/questions.json` and seeded into PostgreSQL on db-api startup (upserted on each deploy). Changing questions requires rebuilding db-api. To update survey questions:

1. Edit `db-api/seed/questions.json`
2. Commit and push your changes to GitHub
3. Railway automatically rebuilds and redeploys db-api
4. New questions are seeded into the database on startup

### web-ui

```
NEXT_PUBLIC_NETWORK_ID=testnet
NEXT_PUBLIC_OUTLAYER_DEPOSIT_NEAR=0.025
NEXT_PUBLIC_PROJECT_ID=agency.testnet/near-forms
NEXT_PUBLIC_FORM_ID=daf14a0c-20f7-4199-a07b-c6456d53ef2d
NEXT_PUBLIC_DATABASE_API_URL=<your Railway db-api public URL>
```

To get the db-api public URL:

- Click on **db-api** service in Railway
- Copy the **Public URL** (looks like `https://near-forms-db-api-xxxxx.railway.app`)
- Paste as `NEXT_PUBLIC_DATABASE_API_URL`

### postgres (if using Railway managed Postgres)

No variables needed — Railway auto-configures.

## Step 5: Deploy Services

Railway auto-deploys when you:

1. Trigger a GitHub push to the repo, OR
2. Manually click **Redeploy** in Railway dashboard for any service

After deployment:

- `db-api` gets a public URL like `https://near-forms-db-api-xxxxx.railway.app`
- `web-ui` gets a public URL like `https://near-forms-web-xxxxx.railway.app`

Verify all services are running: each shows a green status in the Railway dashboard.

## Step 6: Build and Deploy WASI Module

The WASI module runs in OutLayer (not Railway). Build locally:

```bash
cd wasi-near-forms-ark
./build.sh
```

Output: `target/wasm32-wasip2/release/wasi_near_forms_ark.wasm`

Upload to OutLayer dashboard (see Step 7).

## Step 7: Configure OutLayer

### 7.1 Create Project

1. Go to [OutLayer dashboard](https://outlayer.fastnear.com)
2. Create a new project: `agency.testnet/near-forms`
3. Upload WASM: `wasi-near-forms-ark/target/wasm32-wasip2/release/wasi_near_forms_ark.wasm`

### 7.2 Set Secrets

| Secret Name            | Value                            |
| ---------------------- | -------------------------------- |
| `PROTECTED_MASTER_KEY` | Your 32-byte hex key from Step 1 |
| `DATABASE_API_URL`     | Your Railway db-api public URL   |
| `DATABASE_API_SECRET`  | Your API_SECRET from Step 1      |
| `FORM_CREATOR_ID`      | `contributors.testnet`           |

`DATABASE_API_URL` example: `https://near-forms-db-api-xxxxx.railway.app`

## Step 8: Test

### 8.1 Verify Form Loads

Open your Railway web-ui public URL (e.g., `https://near-forms-web-xxxxx.railway.app`) in browser. You should see the form index page.

### 8.2 Submit a Test Response

1. Open `/forms/daf14a0c-20f7-4199-a07b-c6456d53ef2d`
2. Connect a NEAR testnet wallet
3. Fill the form — verify conditional questions:
   - q12 = "Yes" → q12b should appear
   - q12b = "Negative" → q13 should appear
4. Submit → wallet popup appears
5. Confirm transaction

Verify submission stored:

```bash
curl https://near-forms-db-api-xxxxx.railway.app/forms/daf14a0c-20f7-4199-a07b-c6456d53ef2d/submissions | jq length
```

### 8.3 View Responses as Creator

1. Open `/responses`
2. Connect the creator wallet (`FORM_CREATOR_ID`)
3. Wallet popup appears for ReadResponses transaction
4. Decrypted responses appear in the table

### 8.4 Verify Authorization

Connect a non-creator wallet to `/responses`. The WASI module should reject with an auth error.

## Step 9: Monitoring

### View Logs

In Railway dashboard:

- Click any service
- Scroll to **Logs** tab
- View real-time logs or search by service

### Database Backups

Railway Postgres auto-backs up daily. To manually export:

```bash
# Get DATABASE_URL from Railway dashboard
export DATABASE_URL="postgresql://user:pass@host:port/db"

# Export
pg_dump $DATABASE_URL > backup_$(date +%Y%m%d).sql

# Restore
psql $DATABASE_URL < backup_20260226.sql
```

## Step 10: Updates

### Update Survey Questions

1. Edit `db-api/seed/questions.json`
2. Commit and push to GitHub
3. Railway automatically rebuilds and redeploys db-api
4. New questions take effect immediately (no manual Railway variable update needed)

### Update WASI Module

If you change `FORM_ID` or logic in WASI:

```bash
cd wasi-near-forms-ark
./build.sh
# Upload new .wasm to OutLayer dashboard
```

### Update Web UI

Any changes to web-ui code auto-trigger Railway redeploy on GitHub push. If only env vars changed, click **Redeploy** in Railway dashboard.

## Troubleshooting

### "Failed to load form" on web UI

1. Check `NEXT_PUBLIC_DATABASE_API_URL` is set correctly (must be public Railway URL)
2. Verify db-api service status is green in Railway dashboard
3. Check db-api logs in Railway for errors

### "Not authorized" on ReadResponses

1. Verify `FORM_CREATOR_ID` in OutLayer secrets matches your wallet account
2. Verify you're signing in with that account in the browser

### "Decryption failed"

1. Check `PROTECTED_MASTER_KEY` in OutLayer secrets is exactly 64 hex characters (32 bytes)
2. Verify it matches the key used in WASI compile-time (check OutLayer dashboard)

### Submission Transaction Fails

1. Verify all 4 OutLayer secrets are set correctly
2. Verify `DATABASE_API_URL` in OutLayer secrets is your public Railway db-api URL (not `localhost`)
3. Check WASI logs in OutLayer dashboard

### Database Connection Error

1. Verify `DATABASE_URL` is set in Railway db-api settings
2. If using Railway managed Postgres, it auto-generates `DATABASE_URL` — no manual setup needed
3. Check database logs in Railway dashboard

### Can't Connect to db-api from OutLayer

Ensure the Railway db-api public URL is used in OutLayer `DATABASE_API_URL` secret (not localhost or Docker internal address). Railway automatically exposes public URLs.

## Scaling

Railway free tier supports small deployments. To scale:

- Upgrade to paid plan for more resources
- Railway handles auto-scaling and load balancing
- No infrastructure changes needed — just adjust plan in Railway dashboard

## Cost

- **Free tier:** Limited resources, suitable for testing/demo
- **Paid:** Pay-as-you-go. Typical costs:
  - Postgres: ~$10–15/month
  - db-api: ~$5/month
  - web-ui: ~$5/month
  - Total: ~$20–25/month for production

See [railway.app/pricing](https://railway.app/pricing) for details.

## Next Steps

- Add a custom domain to web-ui service (Railway supports CNAME)
- Enable monitoring alerts in Railway dashboard
- Customize `db-api/seed/questions.json` with your survey data
- Deploy WASI to production OutLayer account (not testnet)
- Monitor responses and adjust survey as needed
