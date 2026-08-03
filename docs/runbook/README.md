# ApexEdge — Operational Runbook

This runbook covers deployment, startup, health checking, troubleshooting, and the
go/no-go checklist for ApexEdge releases. See `CHANGELOG.md` for the version currently
being deployed.

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
| `APEX_EDGE_SYNC_STALENESS_DEGRADED_SECONDS` | No | `900` | Sync staleness (seconds) beyond which `/sync/status` reports `degraded: true` and the UI shows a degraded banner. |
| `APEX_EDGE_RESERVATION_TTL_SECONDS` | No | `3600` | Lifetime of a stock reservation before the sweeper may release it (frees stock from abandoned carts). |
| `APEX_EDGE_RESERVATION_SWEEP_INTERVAL_SECONDS` | No | `60` | How often the background sweeper expires stale reservations. A sweep also runs once on startup (crash recovery). |
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
| `INFO` | `"Seeded inventory ledger for N item(s)"` | Real-time oversell ledger initialised from local catalog stock on startup. |
| `INFO` | `"Released N stale stock reservation(s)"` | Reservation TTL sweeper freed stock from abandoned/crashed carts. |
| `WARN` | `"Reservation sweep failed: ..."` | Sweeper cycle errored; stock release retried next interval (sale path unaffected). |

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

### Store stuck in "Degraded mode" (stale stock baselines)

The hub reports `degraded: true` on `GET /sync/status` (and shows a banner in the POS) when the last
successful HQ sync is older than `APEX_EDGE_SYNC_STALENESS_DEGRADED_SECONDS` (default 900s), or when
no sync has ever succeeded. The store keeps selling on the local ledger; this is a freshness
warning, not an outage.

1. Confirm HQ reachability and that `APEX_EDGE_SYNC_SOURCE_URL` is set/correct.
2. Check logs for `Sync failed:` and the underlying error.
3. Inspect freshness directly:
   ```bash
   curl -s http://localhost:3000/sync/status | jq '{degraded, sync_staleness_seconds, last_sync_at}'
   ```
4. Once a sync succeeds, `degraded` clears automatically (a durable `sync_run(id='last_success')`
   marker drives the calculation, so a later failed run will not re-trigger it).

### Stock looks wrong / reservations seem stuck

Live availability is `available_to_sell = hq_baseline + local_adjust - reserved - sold_since_sync`.

1. Fetch the authoritative snapshot:
   ```bash
   curl -s http://localhost:3000/pos/snapshot | jq '.stock, .registers, .parked_carts'
   ```
2. Abandoned-cart reservations are released by the sweeper within
   `APEX_EDGE_RESERVATION_SWEEP_INTERVAL_SECONDS`; watch for `Released N stale stock reservation(s)`.
3. After a crash, a startup sweep plus the durable ledger restore consistent state; a reconnecting
   POS that missed events receives `resnapshot_required` and refetches `/pos/snapshot`.
4. A persistent gap between local availability and HQ on-hand surfaces as
   `apex_edge_inventory_drift_total` increments at reconcile time — investigate shrinkage/receiving.

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

### Local observability stack (Prometheus + Grafana)

For local transparency and live troubleshooting, run ApexEdge and the observability stack together.

1. Start ApexEdge (local host process):
   ```bash
   cargo run -p apex-edge
   ```
2. In another terminal, start observability:
   ```bash
   make observability-up
   ```
3. Open:
   - Prometheus: `http://localhost:9090`
   - Grafana: `http://localhost:3001` (default local credentials: `admin` / `admin`)
4. Validate wiring:
   ```bash
   make observability-validate
   ```

### Provisioned dashboards

- **Edge System Health**: request throughput, 4xx/5xx rates, p95/p99 latency, in-flight requests, health/ready route outcomes, auth outcomes.
- **Dependencies & Data Flows**: DB error/latency, outbox dispatch outcomes + DLQ growth + cycle health, sync ingest outcomes/latency by entity.
- **Transaction Journey**: command funnel (`create_cart` -> `add_line_item` -> `set_tendering` -> `add_payment` -> `finalize_order`), stage outcomes, finalize latency, drop-off ratios, top failures.

### What to monitor

| Signal | Why it matters | Likely failure mode | Expected threshold/trend |
|--------|----------------|---------------------|--------------------------|
| `sum(rate(apex_edge_http_requests_total{status_class="5xx"}[5m]))` | User-facing API reliability | Handler/storage/runtime faults | Near zero in steady state |
| `histogram_quantile(0.99, sum by (le) (rate(apex_edge_http_request_duration_seconds_bucket[5m])))` | End-user responsiveness | DB contention, sync contention, host pressure | Typically < 0.5s p99 in local dev |
| `sum(apex_edge_http_requests_in_flight)` | Live pressure indicator | Request backlog or stuck handlers | Returns to baseline after load |
| `sum(rate(apex_edge_db_operations_total{outcome="error"}[5m]))` | Storage correctness and reliability | SQL errors, DB lock/write pressure | Zero baseline; investigate any sustained value |
| `sum(rate(apex_edge_outbox_dispatch_attempts_total{outcome=~"http_error|timeout|rejected"}[5m]))` | Southbound order submission health | HQ connectivity, schema rejection, timeout | Zero or brief spikes; sustained non-zero is unhealthy |
| `sum(increase(apex_edge_outbox_dlq_total[15m]))` | Data-loss risk / manual intervention | Persistent submit failures exhausting retries | Always zero; non-zero requires immediate triage |
| `sum(rate(apex_edge_sync_ingest_batches_total{outcome="invalid_payload"}[5m]))` | Northbound data contract integrity | Invalid HQ payload/contract drift | Always zero |
| `sum(rate(apex_edge_pos_commands_total{operation="finalize_order",outcome="success"}[5m])) / clamp_min(sum(rate(apex_edge_pos_commands_total{operation="finalize_order"}[5m])), 0.001)` | Checkout completion quality | Pricing/payment/order finalization regressions | Close to 1.0 under normal operation |
| `100 * (1 - (sum(rate(apex_edge_pos_commands_total{operation="finalize_order", outcome="success"}[15m])) / clamp_min(sum(rate(apex_edge_pos_commands_total{operation="add_payment", outcome="success"}[15m])), 0.001)))` | Transaction funnel drop-off after payment | Finalize path bugs, downstream write failures | Close to 0%; investigate growth trend |
| `max(apex_edge_edge_degraded_mode)` | Selling on stale baselines (HQ/WAN down) | HQ unreachable, sync failing | `0` in steady state; `1` means investigate sync/HQ |
| `max(apex_edge_sync_staleness_seconds)` | Freshness of HQ baselines | Sync stalled | Below `APEX_EDGE_SYNC_STALENESS_DEGRADED_SECONDS` |
| `sum(rate(apex_edge_inventory_oversell_prevented_total[5m]))` | Oversell pressure across registers | Hot item near zero stock | Brief spikes ok; sustained means restock/replenish |
| `sum(increase(apex_edge_inventory_drift_total[1h]))` | Local-vs-HQ stock discrepancy | Shrinkage, unrecorded movement | Near zero; sustained growth needs a stock audit |
| `max(apex_edge_register_presence)` | Live registers online | Mass disconnect / network issue | Matches expected lane count |

### Shutdown

```bash
make observability-down
```

---

## 8. Go / No-Go Checklist (Release Deployment)

Before deploying a new release, verify each item:

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
- [ ] `CHANGELOG.md` entry for the release version being deployed is present and accurate; `apex-edge/Cargo.toml` version and `crates/api/src/openapi.rs::APEX_EDGE_RELEASE_VERSION` match it.

### Operational
- [ ] DB path points to a durable volume (not `/tmp` or in-memory).
- [ ] Log output is captured (stdout/stderr to a persistent sink or journal).
- [ ] A process supervisor (systemd, Docker, etc.) will restart apex-edge on crash.
- [ ] Stakeholders have been briefed on the release scope, any migrations, and rollback plan.
