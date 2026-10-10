# Cadri

See [cadri.io](https://www.cadri.io/)

**Turn AI agent sessions into team knowledge.**

Engineering teams now do a lot of their work alongside AI agents, and almost none of what those sessions teach ever gets written down. The fix for a flaky test, the setup step that has to happen first, the way a certain kind of ticket gets resolved: it all lives in a transcript nobody reads again, while the runbook, the PR description, and the design doc never get written.

Cadri captures what agents and developers learn while working, distills it into small reusable knowledge units, and makes it searchable by both people and future agents. Every unit links back to the sessions it came from, so you can always see why Cadri believes something.

> **Status: early development.** Nothing here is stable. Interfaces, schemas, and this README will change. Items marked _(planned)_ do not exist yet.

---

## Repository layout

```
cadri/
  README.md
  LICENSE              # default license, with a note on ee/ exceptions
  plugin/              # where claude code/codex/etc integrations live
  engine/              # all Rust
    Cargo.toml         # workspace root
    ee/                # commercially licensed code
  webui/                 # all TypeScript (dashboard)
    package.json
    src/
    ee/                # commercially licensed UI code
  deploy/              # Dockerfile, docker compose, Helm _(planned)_
  docs/
```


- Rust lives under `engine/`, TypeScript under `webui/`.
- `ee/` is where enterprise gated features live, code in `ee/` may depend on the core, but the core must never depend on `ee/`.

## Getting started

### Prerequisites

- Rust (stable toolchain)
- Node.js and pnpm

_(These recipes are placeholders until the `justfile` is written.)_

## Deployment

Cadri is designed to run in two ways from the same codebase:

- **Self-hosted.** One container image plus Postgres and S3-compatible object storage, started with a single `docker compose up`. Intended for teams whose transcripts cannot leave their own environment. _(planned)_
- **Hosted.** A managed multi-tenant service for teams who prefer not to run it themselves. Tenant isolation is built into the data model from the start (`org_id` on every row). _(planned)_


## Contributing

Contributions are welcome! Please reach out to Cadri team beforehand, to make sure features make sense before building.

## License

This repository is MIT licensed, except for the `ee` folders, where enterprise features are held.

## Security

Cadri handles data that can include source code and credentials. If you find a vulnerability, please report it privately rather than opening a public issue. A security contact and policy (`SECURITY.md`) will be added before any public release.
