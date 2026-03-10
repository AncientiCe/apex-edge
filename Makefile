# ApexEdge: canonical quality and test targets (CI + local).
# Usage: make check | make fmt | make clippy | make test | make test-journey | make doc-test | make audit

.PHONY: fmt clippy test test-journey doc-test audit check setup

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

# Full gate: format, lint, tests (incl. journey + docs), dependency audit (matches CI).
check: fmt clippy test doc-test audit

# Optional: install cargo-audit if missing (CI can run this or pre-install).
setup:
	cargo install cargo-audit 2>/dev/null || true
