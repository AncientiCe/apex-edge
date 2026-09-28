# ApexEdge Packaging

Release packages are built by `.github/workflows/release.yml` when a `v*.*.*` tag is pushed.
Every package wraps the same `apex-edge` release binary.

| Platform | Artifact | Built with | Runner |
|----------|----------|------------|--------|
| Linux x86_64 | `.deb`, `.rpm` | `cargo-deb`, `cargo-generate-rpm` | `ubuntu-latest` |
| Linux aarch64 | `.deb`, `.rpm` | `cargo-deb`, `cargo-generate-rpm` | `ubuntu-24.04-arm` (native) |
| Windows x86_64 | `.msi` | `cargo-wix` (template in `apex-edge/wix/`) | `windows-latest` |

Each job publishes a `SHA256SUMS-<platform>.txt` next to its packages, and the GitHub release
notes are the tag's section of `CHANGELOG.md`. Package metadata lives in `apex-edge/Cargo.toml`
(`[package.metadata.deb]`, `[package.metadata.generate-rpm]`, `[package.metadata.wix]`).

macOS is not packaged.

First run: `apex-edge init` creates the database, applies migrations, loads or generates the audit
key, and prints setup details. See `docs/runbook/README.md` for configuration.
