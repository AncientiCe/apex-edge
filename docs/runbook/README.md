# ApexEdge v0.1.0 — Operational Runbook

This runbook covers deployment, startup, health checking, troubleshooting, and the
go/no-go checklist for the v0.1.0 internal-alpha release.

Related: [README](../../README.md) · [Architecture](../architecture/README.md) · [Contracts](../contracts/README.md) · [Contributing](../../CONTRIBUTING.md) · [Security](../../SECURITY.md)

---

## 1. Prerequisites

| Requirement | Version |
|-------------|---------|
| Rust toolchain | `stable` (see `rust-toolchain.toml` if present, otherwise latest stable) |
| SQLite | bundled via `sqlx` (no separate install needed) |
| Node.js (frontend only) | 18 LTS or later |
| Network access to HQ | Required only when `APEX_EDGE_SYNC_SOURCE_URL` / `APEX_EDGE_HQ_SUBMIT_URL` are set |

---

## 2. Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `APEX_EDGE_DB` | No | `apex_edge.db` (cwd) | Path to SQLite database file. |
| `APEX_EDGE_SEED_DEMO` | No | unset | Set to `1` or `true` to seed demo catalog, customers, and promotions on startup. |
| `APEX_EDGE_SYNC_SOURCE_URL` | No | unset | Base URL of the HQ sync source. If set, sync runs on startup and periodically. |
| `APEX_EDGE_SYNC_INTERVAL_SECONDS` | No | `300` | Sync retry/schedule interval in seconds when `APEX_EDGE_SYNC_SOURCE_URL` is set. |
| `APEX_EDGE_HQ_SUBMIT_URL` | No | unset | URL to POST outbox submissions to HQ. If set, the outbox dispatcher runs every 30 s. |
| `APEX_EDGE_ALLOWED_ORIGINS` | No | unset (wildcard) | Comma-separated list of allowed CORS origins, e.g. `http://localhost:5173,https://pos.internal`. Empty = allow all (logs a warning). Always set this in non-local environments. |
| `APEX_EDGE_AUTH_ENABLED` | No | `false` | Enable edge auth middleware and auth endpoints. When `true`, business routes require bearer access tokens. |
| `APEX_EDGE_AUTH_EXTERNAL_ISSUER` | Yes (if auth enabled) | unset | Expected issuer (`iss`) for external associate token exchange. |
| `APEX_EDGE_AUTH_EXTERNAL_AUDIENCE` | Yes (if auth enabled) | unset | Expected audience (`aud`) for external associate token exchange. |
| `APEX_EDGE_AUTH_EXTERNAL_PUBLIC_KEY_PEM_PATH` | Conditional | unset | Path to PEM public key for verifying external RS256 tokens. |
| `APEX_EDGE_AUTH_EXTERNAL_HS256_SECRET` | Conditional | unset | Shared secret for verifying external HS256 tokens (dev/test mode). |
| `APEX_EDGE_AUTH_SESSION_SIGNING_SECRET` | Yes (if auth enabled) | `dev-hub-secret` | Secret used to sign hub access/refresh tokens. |
| `APEX_EDGE_AUTH_ACCESS_TTL_SECONDS` | No | `300` | Access token lifetime in seconds. |
| `APEX_EDGE_AUTH_REFRESH_TTL_SECONDS` | No | `3600` | Refresh token lifetime in seconds. |
| `APEX_EDGE_AUTH_PAIRING_CODE_TTL_SECONDS` | No | `300` | One-time device pairing code TTL. |
| `APEX_EDGE_AUTH_PAIRING_CODE_LENGTH` | No | `6` | Numeric pairing code length. |
| `APEX_EDGE_AUTH_PAIRING_MAX_ATTEMPTS` | No | `3` | Max pairing attempts per code before rejection. |
| `RUST_LOG` | No | `apex_edge=info` | Tracing log filter (e.g. `apex_edge=debug,sqlx=warn`). |

---

## 3. Building and Running

### Build (release)

```bash
cargo build --release -p apex-edge
```

The binary is at `target/release/apex-edge`.

### Run (local dev with demo data)

```bash
APEX_EDGE_SEED_DEMO=1 cargo run -p apex-edge
```

### Run (against a sync source and HQ)

```bash
APEX_EDGE_DB=/data/apex_edge.db \
APEX_EDGE_SYNC_SOURCE_URL=http://hq.internal:3030 \
APEX_EDGE_HQ_SUBMIT_URL=http://hq.internal/api/orders \
APEX_EDGE_ALLOWED_ORIGINS=http://pos.internal:5173 \
./target/release/apex-edge
```

### Run the POS simulator (frontend)

```bash
cd frontend
npm ci --legacy-peer-deps
npm run dev          # Vite dev server on http://localhost:5173
```

---

## 4. Health Checks

| Endpoint | Method | Success | Description |
|----------|--------|---------|-------------|
| `/health` | GET | `200 {"status":"ok"}` | Process is alive. |
| `/ready` | GET | `200 {"status":"ready"}` | DB is reachable and pool has a connection. Returns `503` if DB probe fails. |
| `/metrics` | GET | `200` Prometheus exposition | Metrics scrape endpoint. Only available when `install_recorder()` succeeds (normal startup). |

### Liveness probe (minimal)

```bash
curl -sf http://localhost:3000/health
```

### Readiness probe (DB check)

```bash
curl -sf http://localhost:3000/ready
```

---

## 5. Logs

The service uses structured logging via `tracing`. Key log events:

| Level | Event | Meaning |
|-------|-------|---------|
| `INFO` | `"ApexEdge listening on ..."` | Server started successfully. |
| `INFO` | `"Sync completed successfully"` | Startup or scheduled sync finished. |
| `WARN` | `"Sync failed: ..."` | Sync cycle failed; will retry on next configured interval (`APEX_EDGE_SYNC_INTERVAL_SECONDS`). |
| `INFO` | `"Outbox dispatcher started ..."` | Dispatcher background task spawned. |
| `INFO` | `"outbox dispatch cycle completed dispatched=N"` | N rows sent to HQ (only logged when N > 0). |
| `ERROR` | `"outbox dispatch cycle error ..."` | Dispatch failed; will retry in 30 s. |
| `WARN` | `"CORS: allowing all origins ..."` | Running in wildcard CORS mode — not for production. |
| `INFO` | `"CORS restricted to N origin(s)"` | CORS is locked to an explicit allowlist. |

Set `RUST_LOG=apex_edge=debug` to see per-row outbox dispatches and sync checkpoint progress.

---

## 6. Common Issues

### DB locked / `SQLITE_BUSY`

SQLite has a single-writer model. Under load, readers may briefly block. If persistent:
- Ensure only one `apex-edge` process writes to the DB at a time.
- Verify `APEX_EDGE_DB` points to a local disk path, not a network share.

### Sync never updates data

1. Confirm `APEX_EDGE_SYNC_SOURCE_URL` is set and the URL is reachable.
2. Check logs for `Sync failed:` errors and the error message.
3. Verify the sync source serves the expected NDJSON format (first line `{"total":N}`, then N base64 lines).
4. Restart the process to trigger an immediate sync cycle.

### Outbox rows accumulate

1. Confirm `APEX_EDGE_HQ_SUBMIT_URL` is set and the HQ endpoint is reachable.
2. Check logs for `outbox dispatch cycle error`.
3. If rows reach `MAX_ATTEMPTS`, they move to the dead-letter queue (`dlq_at` set). Query the DB:
   ```sql
   SELECT * FROM outbox WHERE dlq_at IS NOT NULL;
   ```
4. Investigate and replay DLQ rows manually after fixing the upstream issue.

### CORS errors in browser (preflight fails)

1. If `APEX_EDGE_ALLOWED_ORIGINS` is set, confirm the frontend origin is included exactly
   (scheme + host + port, e.g. `http://localhost:5173`).
2. Check the `access-control-allow-origin` response header:
   ```bash
   curl -si -H "Origin: http://localhost:5173" \
     -H "Access-Control-Request-Method: POST" \
     -X OPTIONS http://localhost:3000/pos/command
   ```
3. If the header is absent, add the origin to `APEX_EDGE_ALLOWED_ORIGINS` and restart.

### Metrics endpoint returns 404

`/metrics` returns 404 when no Prometheus recorder is installed. This happens in test
setups that pass `None` for `metrics_handle`. In normal production startup,
`install_recorder()` is called before `build_router`, so this should not occur.

### Auth exchange fails with 401

1. Verify `APEX_EDGE_AUTH_ENABLED=true`.
2. Confirm external token `iss` and `aud` match `APEX_EDGE_AUTH_EXTERNAL_ISSUER` / `APEX_EDGE_AUTH_EXTERNAL_AUDIENCE`.
3. Confirm exactly one verification mechanism is configured correctly:
   - RS256: valid `APEX_EDGE_AUTH_EXTERNAL_PUBLIC_KEY_PEM_PATH`, or
   - HS256: valid `APEX_EDGE_AUTH_EXTERNAL_HS256_SECRET`.
4. Ensure device is paired first:
   - `POST /auth/pairing-codes` -> `POST /auth/devices/pair` -> `POST /auth/sessions/exchange`.
5. If refresh succeeds but API calls fail, verify session revoke/expiry and clock skew.

---

## 7. Monitoring

Expose `/metrics` to a Prometheus scraper. Key metrics to alert on:

| Metric | Type | Alert condition |
|--------|------|-----------------|
| `apex_edge_outbox_dispatcher_cycles_total{outcome="error"}` | Counter | Sudden increase |
| `apex_edge_http_requests_total{status="5xx"}` | Counter | > 0 in steady state |
| `apex_edge_http_request_duration_seconds` | Histogram | p99 > 500 ms |
| `apex_edge_outbox_dispatcher_cycles_total{outcome="success"}` | Counter | Stops incrementing (dispatcher stalled) |

---

## 8. Go / No-Go Checklist (v0.1.0 Internal Alpha)

Before deploying to internal-alpha testers, verify each item:

### Runtime Correctness
- [ ] `cargo test --workspace --all-features` — all tests pass (0 failures).
- [ ] Outbox dispatcher runs when `APEX_EDGE_HQ_SUBMIT_URL` is set; verify with log `"Outbox dispatcher started"`.
- [ ] Sync applies entity data on startup when `APEX_EDGE_SYNC_SOURCE_URL` is set; verify catalog/products appear in `/catalog/products` response.
- [ ] Price-book entries are atomically replaced on each sync cycle (no stale entries).

### Quality Gates
- [ ] `cargo fmt --check` — no formatting drift.
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings` — zero warnings.
- [ ] `cargo audit` — no known security advisories.
- [ ] `cd frontend && npm run lint && npm run test` — ESLint and Vitest both pass.

### Security
- [ ] `APEX_EDGE_ALLOWED_ORIGINS` is set to the expected frontend origin(s) in the deployment config.
- [ ] Log line `"CORS restricted to N origin(s)"` appears on startup (not the wildcard warning).
- [ ] Preflight from an unrelated origin returns no `access-control-allow-origin` header (verify manually with `curl`).

### Observability
- [ ] `/metrics` endpoint returns Prometheus exposition (not 404).
- [ ] `apex_edge_outbox_dispatcher_cycles_total` counter increments over time.
- [ ] HTTP request duration histogram appears in scrape output.

### Documentation
- [ ] `docs/architecture/README.md` reflects current runtime components and CORS posture.
- [ ] `docs/runbook/README.md` (this file) is accurate for the deployed configuration.
- [ ] `CHANGELOG.md` entry for `[0.1.0]` is present and accurate.

### Operational
- [ ] DB path points to a durable volume (not `/tmp` or in-memory).
- [ ] Log output is captured (stdout/stderr to a persistent sink or journal).
- [ ] A process supervisor (systemd, Docker, etc.) will restart apex-edge on crash.
- [ ] Internal testers have been briefed: this is alpha; data may be reset between releases.
