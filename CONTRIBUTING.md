# Contributing to ApexEdge

Thank you for considering a contribution. This document describes the development
workflow, quality expectations, and how to submit changes.

---

## Table of Contents

- [Getting started](#getting-started)
- [Development workflow](#development-workflow)
- [Quality gates](#quality-gates)
- [Commit messages](#commit-messages)
- [Pull requests](#pull-requests)
- [Reporting bugs and requesting features](#reporting-bugs-and-requesting-features)
- [Security issues](#security-issues)
- [Code of conduct](#code-of-conduct)
- [License](#license)

---

## Getting started

### Prerequisites

| Tool | Version |
|------|---------|
| Rust toolchain | `stable` (see `rust-toolchain.toml` if present) |
| Docker | any recent version (optional, for container builds) |
| Node.js | 18 LTS or later (frontend simulator only) |
| `cargo-audit` | install with `cargo install cargo-audit` or `make setup` |

### Clone and build

```bash
git clone https://github.com/AncientiCe/apex-edge.git
cd apex-edge
cargo build
```

### Run locally with demo data

```bash
# Linux / macOS
APEX_EDGE_SEED_DEMO=1 cargo run -p apex-edge

# Windows PowerShell
$env:APEX_EDGE_SEED_DEMO = "1"; cargo run -p apex-edge
```

See [README](README.md) for full environment-variable reference and Docker instructions.

---

## Development workflow

This project follows **test-driven development** (TDD):

1. **Write a failing test** that defines the expected behaviour.
2. **Confirm it fails** — run the test suite and see the red state.
3. **Implement** the minimum code to make the test pass.
4. **Confirm it passes** — run the test suite and see the green state.

Do not implement behaviour without a failing test that defines it.

---

## Quality gates

All of the following must pass before a pull request can merge:

```bash
make check
```

This runs (in order):

| Command | What it checks |
|---------|----------------|
| `cargo fmt --all -- --check` | Formatting |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Lints (warnings are errors) |
| `cargo test --workspace --all-features` | Unit, integration, smoke, and journey tests |
| `cargo audit` | Known security advisories in dependencies |
| `cd frontend && npm run lint && npm run test` | Frontend ESLint + Vitest |

Run individual targets:

- `make fmt-fix` — auto-format code.
- `make clippy` — lint only.
- `make test` — tests only.
- `make test-journey` — orchestrator journey tests only.
- `make audit` — dependency advisories only.
- `make frontend-check` — frontend lint + tests.

Fix every failure before marking a PR ready for review.

### No dead code or unused variables

Compiler warnings for unused items are treated as errors by CI. Remove or replace with
`_` instead of suppressing with `#[allow(dead_code)]`.

### Observability

Every new code path must emit metrics. At minimum:

- A **count** of how many times the operation is invoked.
- An **error count** labelled by error kind where possible.
- **Latency** for critical operations (DB queries, external calls).

Use the existing `apex-edge-metrics` crate and follow the naming convention
`apex_edge_<subsystem>_<operation>_<unit>` (e.g. `apex_edge_orders_created_total`).

### Architecture documentation

Every **new user-facing feature** must include a diagram in
[`docs/architecture/README.md`](docs/architecture/README.md):

- A Mermaid flowchart or sequence diagram showing the feature flow.
- A short **Purpose** description.
- **Notes** covering inputs, outputs, and failure paths.

---

## Commit messages

- Use the imperative mood: _"Add X"_, _"Fix Y"_, _"Remove Z"_.
- Keep the subject line under 72 characters.
- Reference issues or PRs in the body when relevant (e.g. `Fixes #42`).

---

## Pull requests

1. Fork the repository and create a branch from `main`.
2. Make sure `make check` passes locally before pushing.
3. Open a pull request against `main`.
4. Describe **what** changed and **why** in the PR description.
5. Link any related issues.
6. A maintainer will review your PR. Feedback may be requested; please respond promptly.

For substantial changes (new features, architecture changes, breaking API changes)
consider opening an issue to discuss the approach before investing significant effort.

---

## Reporting bugs and requesting features

Open a [GitHub issue](https://github.com/AncientiCe/apex-edge/issues). For bugs,
include reproduction steps, the affected version, and relevant log output.

---

## Security issues

**Do not open a public issue for security vulnerabilities.** See [SECURITY.md](SECURITY.md)
for the private disclosure process.

---

## Code of conduct

All contributors are expected to follow the [Code of Conduct](CODE_OF_CONDUCT.md).

---

## License

By contributing, you agree that your contributions will be licensed under the same
terms as this project: **MIT OR Apache-2.0**. See [LICENSE-MIT](LICENSE-MIT) and
[LICENSE-APACHE](LICENSE-APACHE).
