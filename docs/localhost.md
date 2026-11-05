# Cadri on localhost (Mac)

This path is for developers running the engine on your machine. It does **not** use WorkOS, Sign up, or the 30-day trial. Hosted install lives at https://cadri.io/docs/installation.

## Prerequisites

- Rust toolchain (`cargo`)
- Node.js (for the web UI)
- [uv](https://github.com/astral-sh/uv) (for the Cadri plugin CLI)

## Start the engine + UI

From the `cadri/` repo root:

```bash
./scripts/run
```

- Engine: `http://127.0.0.1:8080` (auth mode `none`)
- Web UI: `http://127.0.0.1:5173`

Or start only the engine in the background and point this machine's agents at it, from the `cadri/` repo root:

```bash
./plugin/bin/cadri init --local --start-engine
```

`cadri init` installs the capture hooks and remembers the server URL in `~/.config/cadri/config.json`, so hooks keep uploading to localhost. `--local` skips device login unless you also pass `--login`. Credentials are keyed by URL, so localhost and `https://app.cadri.io` do not overwrite each other. Run `cadri init` without `--local` to switch back to hosted Cadri.

To install the CLI from this checkout instead of the newest release: `bash plugin/install.sh`.

For plugin development, load the hooks straight from the checkout instead of running `cadri init` (doing both uploads every session twice):

```bash
claude --plugin-dir "$(pwd)/plugin"
```

## Metrics

A direct engine process serves Prometheus text at `http://127.0.0.1:8080/metrics`.
The Docker web UI's nginx does not forward that path. Page loads from the Vite app
are posted to `POST /api/telemetry/page`. Hosted Grafana login is documented with the
cluster chart in `cadri-cloud/docs/telemetry.md`.

## Auth

- Default `./scripts/run`: no token.
- Docker / token mode: set `CADRI_API_TOKEN` for hooks and the CLI.
