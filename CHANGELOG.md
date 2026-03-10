# Changelog

All notable changes to this project will be documented here.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).  
Versioning follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

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

[Unreleased]: https://github.com/your-org/apex-edge/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/your-org/apex-edge/releases/tag/v0.1.0
