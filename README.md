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
  docker/              # self-hosted Compose file
  docs/
```


- Rust lives under `engine/`, TypeScript under `webui/`.
- `ee/` is where enterprise gated features live, code in `ee/` may depend on the core, but the core must never depend on `ee/`.

## Run it

From the repository root. Each command starts the engine and the web UI, then prints the address to open.

**On this machine.** Requires [Rust](https://rustup.rs) and Node.js. There is no login, and transcripts are stored in `engine/data/`, which git ignores. The first run compiles the engine, which can take a few minutes. Ctrl-C stops both processes.

```sh
./scripts/run
```

**Self-hosted, with Docker.** Requires Docker Compose. The first run builds the images. The script creates `docker/.env` with an API token when that file is missing, then prints the token. Paste it into the web UI when the page asks.

```sh
./scripts/run-with-docker
```

Stop the Docker setup with `docker compose -f docker/docker-compose.yml down`.

## Deployment

Cadri runs in two ways from the same codebase:

- **Self-hosted.** `./scripts/run-with-docker` runs the engine and the web UI on one machine. Transcripts stay in a SQLite file on that machine.
- **Hosted.** A managed multi-tenant service for teams who prefer not to run it themselves. Tenant isolation is built into the data model from the start (`org_id` on every row).


## Contributing

Contributions are welcome! Please reach out to Cadri team beforehand, to make sure features make sense before building.

## License

This repository is MIT licensed, except for the `ee` folders, where enterprise features are held. We made this decision, as we believe deeply in open source (hence very permissive MIT license), but have some paranoia about a large cloud company ripping us off if we become successful (e.g. what happened to Elasticsearch/Opensearch). 

What's current in the `ee` folders allows us to provide a fully hosted, managed service (e.g. we handle hosting and auth, etc.). If you'd like us to host Cadri within your team's private cloud, please reach out to `nelson@cadri.io`, and he'll help you out. 

## Security

Cadri handles data that can include source code and credentials. If you find a vulnerability, please report it privately rather than opening a public issue. A security contact and policy (`SECURITY.md`) will be added before any public release.
