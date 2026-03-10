# ApexEdge: canonical quality and test targets (CI + local).
# Usage: make check | make fmt | make clippy | make test | make audit

.PHONY: fmt clippy test audit check setup

fmt:
	cargo fmt --all -- --check

fmt-fix:
	cargo fmt --all

clippy:
	cargo clippy --workspace --all-targets --all-features -- -D warnings

test:
	cargo test --workspace --all-features

audit:
	cargo audit

# Full gate: format, lint, tests, dependency audit (matches CI).
check: fmt clippy test audit

# Optional: install cargo-audit if missing (CI can run this or pre-install).
setup:
	cargo install cargo-audit 2>/dev/null || true
