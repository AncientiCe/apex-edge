# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| `main` (latest) | Yes |
| older releases | No — please upgrade |

## Reporting a Vulnerability

**Do not open a public GitHub issue for security vulnerabilities.**

Use [GitHub Security Advisories](https://github.com/AncientiCe/apex-edge/security/advisories/new)
to report a vulnerability privately. This keeps the disclosure confidential until a fix is available.

### What to include

- A clear description of the vulnerability and its potential impact.
- Steps to reproduce or a minimal proof-of-concept.
- The version(s) or commit(s) affected.
- Any suggested mitigations, if you have them.

### What to expect

- We will acknowledge receipt within **3 business days**.
- We aim to triage and provide an initial assessment within **7 days**.
- We will coordinate disclosure timing with you before publishing a fix.

## Scope

In scope:

- The `apex-edge` binary and all workspace crates (`crates/`, `apex-edge/`, `tools/`).
- The POS simulator frontend (`frontend/`).
- Docker image and build pipeline configuration.

Out of scope:

- Third-party dependencies (report upstream; we will bump the dependency promptly once a fix is available).
- Issues requiring physical access to the deployment environment.

## Dependency Advisories

This project runs `cargo audit` as a required CI quality gate. Known advisories in
third-party dependencies are tracked and resolved on a best-effort basis. To audit
your own deployment, run:

```bash
cargo audit
```

---

See also: [Contributing](CONTRIBUTING.md) · [README](README.md)
