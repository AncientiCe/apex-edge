# ApexEdge: canonical quality and test targets (CI + local).
# Usage: make check | make fmt | make clippy | make test | make test-journey | make doc-test | make audit
#        make frontend-lint | make frontend-test | make frontend-check
#        make observability-up | make observability-down | make observability-logs | make observability-validate

.PHONY: fmt clippy test test-journey doc-test audit check setup \
        frontend-lint frontend-test frontend-check \
        observability-up observability-down observability-logs observability-validate \
        run-example-sync run-apex-with-example-sync dev-sync

fmt:
	cargo fmt --all -- --check

fmt-fix:
	cargo fmt --all

clippy:
	cargo clippy --workspace --all-targets --all-features -- -D warnings

test:
	cargo test --workspace --all-features

# Full order flow: create cart, search/add product, customer, 20%% promo, payment, finalize, document + HQ payload.
test-journey:
	cargo test -p apex-edge --test orchestrator_journey

doc-test:
	cargo test --doc --workspace

audit:
	cargo audit

frontend-lint:
	cd frontend && npm run lint

frontend-test:
	cd frontend && npm run test

# Frontend quality gate (lint + unit tests).
frontend-check: frontend-lint frontend-test

# Validate local observability compose and provisioning.
observability-validate:
	docker compose -f docker-compose.observability.yml config -q

observability-up:
	docker compose -f docker-compose.observability.yml up -d

observability-down:
	docker compose -f docker-compose.observability.yml down

observability-logs:
	docker compose -f docker-compose.observability.yml logs -f --tail=200

# Full gate: format, lint, tests (incl. journey + docs), dependency audit, frontend (matches CI).
check: fmt clippy test doc-test audit frontend-check observability-validate

# Optional: install cargo-audit if missing (CI can run this or pre-install).
setup:
	cargo install cargo-audit 2>/dev/null || true

# Continuous synthetic probe against a running hub. Exposes its own /metrics at
# 0.0.0.0:9999 so Prometheus can scrape the SLO counters.
# Usage: APEX_EDGE_URL=http://hub:3000 APEX_EDGE_INTERVAL=15 make smoke-loop
smoke-loop:
	cargo run -p synthetic-journey

# Run only the example sync source (default: http://127.0.0.1:3030).
run-example-sync:
	cargo run -p example-sync-source

# Run ApexEdge against the local example sync source.
run-apex-with-example-sync:
	APEX_EDGE_SYNC_SOURCE_URL=http://127.0.0.1:3030 cargo run -p apex-edge

# One-command local stack: example sync source + ApexEdge pointed to it.
# Ctrl+C stops both processes.
dev-sync:
	@set -euo pipefail; \
	trap 'kill 0' INT TERM EXIT; \
	cargo run -p example-sync-source & \
	sleep 1; \
	APEX_EDGE_SYNC_SOURCE_URL=http://127.0.0.1:3030 cargo run -p apex-edge
