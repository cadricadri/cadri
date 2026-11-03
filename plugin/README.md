# Cadri plugin + agent bootstrap

Cross-harness Cadri client: discover local agent sessions, backfill transcripts, install
steady-state hooks/skills, and keep Claude Code (and friends) uploading to Cadri.

Uses **[uv](https://github.com/astral-sh/uv)** for Python. Layout:

```text
plugin/
  bin/cadri              # uv-backed CLI
  shared/                # auth, upload, discover/
  harnesses/
    claude/              # hooks + skills + .claude-plugin
    cursor/ codex/ opencode/
  install.sh
  hooks/                 # shim for `claude --plugin-dir ./plugin`
```

## Hosted install

Follow https://cadri.io/docs/installation — or:

```sh
curl -fsSL https://cadri.io/install.sh | bash
export PATH="$HOME/.local/bin:$PATH"
cadri init
```

## Localhost (Mac)

See [../docs/localhost.md](../docs/localhost.md). Quick path from this checkout:

```sh
./install.sh
export PATH="$HOME/.local/bin:$PATH"
export CADRI_URL=http://127.0.0.1:8080
cadri init --local --start-engine
```

## Claude Code from a checkout

```sh
claude --plugin-dir /path/to/cadri/plugin
```

Sign in (hosted):

```sh
./bin/cadri login
# or: ./bin/cadri-login
```

## CLI

```sh
cadri login
cadri init
cadri init --local [--start-engine]
cadri discover
```

## Tests

```sh
cd plugin
python3 -m unittest discover -s tests
# or: uv run python -m unittest discover -s tests
```
