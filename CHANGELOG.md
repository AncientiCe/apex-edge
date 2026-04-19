# Changelog

All notable changes to this project will be documented here.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).  
Versioning follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

---

## [0.6.0] — 2026-04-19

"Retail complete". Turns ApexEdge from a checkout-focused hub into a full day-of-store
back-office: returns & refunds, till & shift management, supervisor approvals,
tamper-evident audit, real-time push to POS, HA-ready topology, and first-class
OpenAPI + SLO observability. All features are offline-first and gated by the
existing outbox.

### Added

#### Flagship features

- **Returns & refunds** (`crates/domain/src/returns.rs`, `crates/storage/src/returns.rs`, `crates/api/src/returns_handler.rs`)
  - Receipted and blind return paths with a dedicated state machine.
  - New `PosCommand`s: `start_return`, `return_line_item`, `refund_tender`, `finalize_return`, `void_return`.
  - Blind returns gated by the new supervisor-approval primitive.
  - `HqReturnSubmissionEnvelope` pushed through the existing outbox with deterministic checksum.
  - Metrics: `apex_edge_returns_total`, `apex_edge_return_duration_seconds`, `apex_edge_refund_tender_total`.
  - Migration `010_returns.sql` (+ rollback).
- **Till & shift management** (`crates/domain/src/shifts.rs`, `crates/storage/src/shifts.rs`, `crates/api/src/shifts_handler.rs`)
  - One open shift per register (unique partial index). Open till, paid-in, paid-out, no-sale, cash count, X-report, close-till.
  - Approvals required for high-value cash movements and large close-of-drawer variance.
  - `HqShiftSubmissionEnvelope` (Z-report) flows through the outbox.
  - Metrics: `apex_edge_shifts_total`, `apex_edge_shift_variance_cents`, `apex_edge_cash_movements_total`.
  - Migration `011_shifts.sql` (+ rollback).
- **Supervisor approvals** (`crates/storage/src/approvals.rs`, `crates/api/src/approvals.rs`)
  - `POST /approvals`, `GET /approvals/:id`, `POST /approvals/:id/grant|deny`.
  - Expiring pending approvals; wired into returns, voids, cash movements, manual discounts.
  - Metrics: `apex_edge_approvals_total`, `apex_edge_approval_wait_duration_seconds`.
  - Migration `013_approvals.sql` (+ rollback).
- **Tamper-evident audit log** (`crates/storage/src/audit.rs`)
  - Hash-chained via HMAC-SHA256 keyed by per-hub `AUDIT_KEY`.
  - `GET /audit/verify` re-walks the chain and returns `{ok, checked, first_bad_id, reason}`.
  - Metrics: `apex_edge_audit_chain_verifications_total`, `apex_edge_audit_chain_length`, `apex_edge_audit_records_total`.
  - Migration `012_audit_chain.sql` (columns applied idempotently in Rust).
- **Real-time POS push** (`crates/api/src/stream.rs`)
  - `GET /pos/stream` (WebSocket) with SSE fallback at `GET /pos/events`.
  - Per-store broadcast fanout with monotonically-sequenced envelopes.
  - Event kinds: `cart_updated`, `approval_requested`, `return_updated`, `shift_updated`, `document_ready`, `sync_progress`, `price_updated`.

#### HA & operations

- **Standby mode** (`crates/api/src/role.rs`)
  - `APEX_EDGE_STANDBY=1` flips the hub into read-only mode.
  - `standby_guard_middleware` rejects writes with `503 + Retry-After: 30 + X-ApexEdge-Role: standby`.
  - Every response is now tagged with `X-ApexEdge-Role`.
  - Metric: `apex_edge_role{role=...}`.
- **Failover / DR runbook** (`docs/runbook/failover.md`)
  - Three operator playbooks: box recovery, disaster recovery (Litestream restore from object store), optional on-LAN standby for high-volume stores.
  - Deployment-agnostic: runs on whatever the store already uses (bare metal, VM, container, systemd).
- **Migration rollback matrix** (`crates/storage/migrations/*.down.sql`, `crates/storage/tests/migration_rollback_matrix.rs`)
  - Every v0.6.0 migration has a `.down.sql` and a forward→back→forward integration test.

#### Developer experience

- **OpenAPI 3.1 + Swagger UI** — `GET /openapi.json` and `GET /docs`.
- **SLO dashboard** — `observability/grafana/dashboards/slo-synthetic.json` with success ratio, error budget, synthetic latency, role, replication lag, returns/shifts/approvals breakdowns.
- **Continuous synthetic probe** — `tools/synthetic-journey` runs a read-only golden path against any hub on a fixed cadence, exposing `apex_edge_synthetic_journey_*` metrics. Run with `make smoke-loop`.
- **Crash-recovery proptest** — `crates/storage/tests/crash_recovery_proptest.rs` randomly SIGKILLs between outbox + audit operations and asserts post-replay invariants (idempotent inserts, delivered-stays-delivered, chain verifies).

### Changed

- `AppState` gained `stream: StreamHub` and `role: HubRole`; both are wired through the router and all tests.
- `audit::record` now transactional, prepending `prev_hash`, `hash`, `hub_key_id` to every entry.
- Main bootstrap loads the audit signing key from `APEX_EDGE_AUDIT_KEY_PATH` or `APEX_EDGE_AUDIT_KEY_SECRET`, or generates one and persists it.

### Migrations

- `010_returns.sql`, `011_shifts.sql`, `012_audit_chain.sql`, `013_approvals.sql` — all additive, idempotent, and ship with `.down.sql` rollbacks.

### Upgrade notes

- Set `APEX_EDGE_AUDIT_KEY_PATH=/data/audit.key` (recommended) or `APEX_EDGE_AUDIT_KEY_SECRET`. On first boot the hub will generate one.
- POS clients should handle `503 + X-ApexEdge-Role: standby` and show a maintenance banner during failover.
- For disaster recovery, run a WAL-shipping sidecar (Litestream is the reference impl) against the existing SQLite file; see `docs/runbook/failover.md`.

---

## [0.5.0] — 2026-03-27

Completes checkout command coverage with idempotent POS command handling, manual promo lifecycle commands, synced coupon definitions, and cart-state customer enrichment.

### Added

#### Runtime

- **Completed checkout command set** — implemented `apply_promo`, `remove_promo`, and `void_cart` handlers in `/pos/command` with full state validation and pricing recalculation.
- **POS idempotency replay** — `/pos/command` now checks persisted idempotency responses and replays prior successful responses for repeated keys.
- **Coupon definition validation path** — `apply_coupon` now validates synced `CouponDefinition` records and eligibility before accepting coupon application.
- **Customer-enriched cart state** — `CartState` now includes `customer_name` and `customer_code` when a customer is attached.

#### Storage

- **Coupon definitions schema** — added migration `009_coupon_definitions.sql` with storage APIs to upsert and fetch coupon definitions by code.

#### Sync

- **Coupons ingest support** — `run_sync_ndjson` now applies `coupons` entities into local storage instead of skipping them as unknown.

#### Quality

- Added behavioral tests for `void_cart`, `apply_promo`/`remove_promo`, idempotency replay, coupon ingestion, coupon eligibility rejection on exhausted limits, and cart customer enrichment.
- Added domain tests covering non-zero discount behavior for `BuyXGetY` and `PriceOverride` promotion types.

#### Documentation

- Updated `docs/architecture/README.md` with a new checkout command completion section and refreshed sync entity coverage to include coupons and print templates.

### Changed

- `BuyXGetY` and `PriceOverride` promotion types now produce real discounts in the promo engine instead of always resolving to zero.
- Pricing pipeline now applies explicitly-selected manual promos in addition to automatic promotions.

---

## [0.4.0] — 2026-03-18

Hardens northbound checkout/document behavior and adds production-ready observability tooling and coverage across backend and frontend.

### Added

#### Runtime

- **Checkout and document hardening** — strengthened POS finalize/document flow and northbound contract behavior.
- **Local observability stack** — added Prometheus + Grafana docker stack with provisioned dashboards and recording rules.
- **Frontend request journey tracking** — added per-request journey tracing in simulator API client and UI summaries.

#### Quality

- Added observability stack validation test coverage and expanded frontend/backend behavioral tests.

#### Documentation

- Updated `README.md`, `docs/architecture/README.md`, and `docs/runbook/README.md` with observability setup, behavior ownership, and operational guidance.

---

## [0.3.0] — 2026-03-16

Adds local hub auth with device trust and integrates the POS simulator into strict auth mode.

### Added

#### Runtime

- **Hub auth endpoints** — added pairing/session endpoints:
  `POST /auth/pairing-codes`, `POST /auth/devices/pair`,
  `POST /auth/sessions/exchange`, `POST /auth/sessions/refresh`,
  and `POST /auth/sessions/revoke`.
- **Auth middleware and principal context** — protected northbound routes now require
  valid hub session + trusted device when auth is enabled; principal context is attached
  for handler use and audit events.
- **Token exchange model** — external associate token validation and hub-issued access/refresh
  session tokens with refresh rotation and revoke support.
- **Device trust via pairing code** — one-time, short-lived, attempt-limited pairing flow with
  trusted device enrollment.

#### Storage

- **Auth persistence schema** — added `trusted_devices`, `device_pairing_codes`,
  `auth_sessions`, and `associate_identities` with indexed lookups for auth flows.

#### Frontend Simulator

- **Strict auth-only mode** — simulator blocks protected operations until pairing + sign-in completes.
- **Auth bootstrap UI** — connection panel now supports associate/store/device inputs,
  mock token secret, `Pair & Sign In`, auth status, and `Sign Out`.
- **Auth transport wrapper** — protected API calls include bearer token, perform single refresh/retry on `401`,
  and reset auth state on refresh failure.
- **Local mock external token mode** — simulator generates HS256 external token claims
  (`sub`, `iss`, `aud`, `store_id`, `iat`, `exp`) for `/auth/sessions/exchange` in dev/test.

#### Observability

- **Auth metrics** — added auth request/session/pairing counters and latency histogram:
  `apex_edge_auth_requests_total{operation,outcome}`,
  `apex_edge_auth_request_duration_seconds{operation}`,
  `apex_edge_auth_sessions_total{outcome}`,
  `apex_edge_device_pairings_total{outcome}`.
- **Audit events** — pairing/session lifecycle and auth failure paths are emitted.

#### Quality

- Added endpoint/middleware behaviour tests for auth bootstrap, protection, refresh, and revoke.
- Added simulator tests for strict auth gate and sign-in/sign-out lifecycle.

#### Documentation

- `docs/architecture/README.md` adds auth architecture flow section for pairing, token exchange,
  route protection scope, failure paths, and metrics.

## [0.2.0] — 2026-03-16

Adds synced print-template support with PDF receipt generation and updates release quality gates.

### Added

#### Runtime

- **Synced PDF document generation** — receipt and gift receipt generation now loads synced
  templates (`customer_receipt`, `gift_receipt`) and emits `application/pdf` documents when
  templates are available.
- **Headless-Chrome PDF renderer** — `apex-edge-printing` includes HTML-to-PDF generation
  via `headless_chrome` for document rendering.

#### Storage

- **Print template persistence** — migration `006_print_templates.sql` adds `print_templates`
  table keyed by `(store_id, document_type)`.
- **Template storage APIs** — new storage functions to upsert and fetch print templates.

#### Observability

- **Document render metrics** — `apex_edge_document_render_total{document_type,outcome}`
  and `apex_edge_document_render_duration_seconds{document_type}` added to track render
  outcomes and latency.

#### Quality

- **API behavioural coverage** — tests now validate that finalized receipts and gift receipts
  become valid PDF payloads when synced templates exist.
- **Storage and sync coverage** — tests cover `print_templates` upsert/fetch behavior and
  sync entity application for print templates.

#### Documentation

- `docs/architecture/README.md` adds Section 14 ("Synced PDF Receipt Templates") with
  flow diagram, inputs/outputs, failure paths, and metric references.

### Changed

- Receipt template lookup accepts both `customer_receipt` and legacy `receipt` document types.
- Frontend flow updated to open generated PDFs and attempt browser print for generated documents.

### Fixed

- Unit/pipeline stability adjustments in printing/frontend integration ("Fix unit pipelines").

---

## [0.1.0] — 2026-03-10

Internal alpha release for team-only testing in a controlled environment.

### Added

#### Runtime

- **Outbox dispatcher loop** — `apex_edge_outbox::run_dispatcher_loop` ticks every 30 s
  when `APEX_EDGE_HQ_SUBMIT_URL` is set; resilient to individual dispatch failures.
- **Full sync entity application** — `apex_edge_sync` now persists all core entity types
  received from HQ: `catalog`, `categories`, `price_book` (atomic delete-and-replace),
  `tax_rules`, `customers`, and `promotions`. Unknown entity kinds are debug-logged and
  checkpointed without storage.
- **`Customer` contract type** — `apex-edge-contracts` exposes `Customer` with `id`,
  `code`, `name`, `email`, and `version` fields for use in sync ingest.
- **Configurable CORS** — `build_router` accepts `allowed_origins: Vec<HeaderValue>`.
  Empty list = wildcard (local dev); non-empty = restricted to listed origins.
  Set `APEX_EDGE_ALLOWED_ORIGINS` (comma-separated) at startup to enable restriction.
  Main logs a warning when running in wildcard mode.

#### Storage

- `update_catalog_item_description` — separate function to set description after insert,
  avoiding the Clippy `too_many_arguments` limit on `insert_catalog_item`.
- `replace_price_book_entries` — atomically replaces all price-book rows for a store
  (snapshot sync semantics).
- `insert_customer` now accepts `email: Option<&str>`.

#### Observability

- `OUTBOX_DISPATCHER_CYCLES_TOTAL` counter in `apex-edge-metrics` tracks every
  dispatcher cycle labelled by `outcome` (success / error).
- HTTP route labels in `request_path_to_route` extended to cover all documented routes
  (`/pos/cart/:cart_id`, `/catalog/products`, `/catalog/categories`, `/customers`,
  `/orders/:order_id/documents/gift-receipt`, `/sync/status`).

#### Quality

- **Backend negative-path tests** — metrics endpoint returns 404 when no recorder is
  installed; outbox row moves to DLQ after `MAX_ATTEMPTS`.
- **Dispatcher loop test** — verifies messages are dispatched and the loop is cancellable.
- **Sync entity application tests** — `crates/sync/tests/sync_entity_application_tests.rs`
  covers catalog, categories, price book, tax rules, customers, and invalid-payload
  failure path.
- **CORS tests** — `cors_restricted_trusted_origin_is_allowed` and
  `cors_restricted_unknown_origin_is_rejected` verify both CORS branches.
- **Frontend quality gates** — ESLint (flat config, TypeScript + React Hooks) and Vitest
  (`jsdom` environment) integrated into `frontend/`; `npm run lint` and `npm run test`
  scripts added.
- **CI frontend job** — `.github/workflows/ci.yml` extended with a `frontend` job that
  runs lint and tests on every push.
- **Makefile** — `frontend-lint`, `frontend-test`, `frontend-check` targets added;
  top-level `check` target includes `frontend-check`.

#### Documentation

- `docs/architecture/README.md` updated: Runtime Bootstrap diagram includes CORS
  configuration step and `APEX_EDGE_ALLOWED_ORIGINS`; new Section 11 (Internal Security
  Baseline) diagrams the CORS posture; outbox and sync ingest sections updated to reflect
  dispatcher loop and full entity application.
- `docs/runbook/README.md` — operational runbook for v0.1.0 internal alpha (environment
  variables, startup, health checks, troubleshooting, go/no-go checklist).
- `CHANGELOG.md` — this file.

### Changed

- `insert_catalog_item` reverted to 7-parameter signature (without `description`) to
  satisfy Clippy `too_many_arguments` limit; callers updated.
- All callers of `build_router` updated to pass the new `allowed_origins` argument.
- `SyncError::NotImplemented` removed; unknown entities are now logged and skipped.

### Fixed

- `dispatcher_loop_dispatches_pending_rows_and_can_be_cancelled` test used an
  in-memory SQLite pool with `max_connections(1)` which lost state on future cancellation.
  Fixed by using a named shared in-memory DB (`sqlite:file:{id}?mode=memory&cache=shared`)
  with `max_connections(5)`.
- `metrics_endpoint_returns_404_when_recorder_not_installed` test incorrectly called
  `expect_err()` on an `axum::http::Response` (not a `Result`). Fixed to check
  `response.status()` directly.
- Mock NDJSON sync server served raw bytes for catalog items; updated to serve valid
  `CatalogItem` JSON payloads so `apply_entity_batch` deserialization succeeds in tests.

[Unreleased]: https://github.com/AncientiCe/apex-edge/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/AncientiCe/apex-edge/releases/tag/v0.5.0
[0.4.0]: https://github.com/AncientiCe/apex-edge/releases/tag/v0.4.0
[0.3.0]: https://github.com/AncientiCe/apex-edge/releases/tag/v0.3.0
[0.2.0]: https://github.com/AncientiCe/apex-edge/releases/tag/v0.2.0
[0.1.0]: https://github.com/AncientiCe/apex-edge/releases/tag/v0.1.0
