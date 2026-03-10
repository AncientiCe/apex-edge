# ApexEdge: canonical quality and test targets (CI + local).
# Usage: make check | make fmt | make clippy | make test | make test-journey | make doc-test | make audit
#        make frontend-lint | make frontend-test | make frontend-check

.PHONY: fmt clippy test test-journey doc-test audit check setup \
        frontend-lint frontend-test frontend-check

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

# Full gate: format, lint, tests (incl. journey + docs), dependency audit, frontend (matches CI).
check: fmt clippy test doc-test audit frontend-check

# Optional: install cargo-audit if missing (CI can run this or pre-install).
setup:
	cargo install cargo-audit 2>/dev/null || true
