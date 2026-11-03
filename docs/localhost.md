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

Or start only the engine in the background via the plugin CLI:

```bash
cd plugin
./install.sh
export PATH="$HOME/.local/bin:$PATH"
export CADRI_URL=http://127.0.0.1:8080
cadri init --local --start-engine
```

## Plugin / harnesses

```bash
cd plugin
./install.sh   # or: uv sync && ./bin/cadri …
CADRI_URL=http://127.0.0.1:8080 cadri init --local
```

`--local` skips WorkOS device login unless you also pass `--login`. Credentials are keyed by URL, so localhost and `https://app.cadri.io` do not overwrite each other.

Claude Code (from a checkout):

```bash
claude --plugin-dir "$(pwd)/plugin"
```

## Auth

- Default `./scripts/run`: no token.
- Docker / token mode: set `CADRI_API_TOKEN` for hooks and the CLI.
