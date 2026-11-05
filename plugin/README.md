# Cadri CLI and agent hooks

Captures Claude Code, Cursor, Codex, and OpenCode sessions into Cadri, backfills recent ones,
and gives agents the memory books through `cadri memory` and two skills.

Uses **[uv](https://github.com/astral-sh/uv)** for Python. Layout:

```text
plugin/
  bin/cadri_cli.py       # every command, including `cadri hook <harness>`
  bin/cadri              # uv shim for a source checkout
  shared/                # auth, upload, discover/, hooks.py (hook runtime), install.py (hook installers), memory.py
  harnesses/opencode/    # OpenCode plugin template
  skills/                # cadri-install, cadri-memory, cadri-memory-seed
  hooks/ .claude-plugin/ # `claude --plugin-dir ./plugin` for development
  install.sh             # served as https://cadri.io/install.sh
  scripts/release        # publish plugin/ as a GitHub release
```

## How capture works

`cadri init` writes one hook entry per event into each harness's own user config, pointing at
`~/.cadri/install/.venv/bin/python …/cadri_cli.py hook <harness>`:

| Harness | Config | Events |
|---|---|---|
| Claude Code | `~/.claude/settings.json` | SessionStart, Stop, PreCompact, SessionEnd |
| Cursor | `~/.cursor/hooks.json` | sessionStart, stop, preCompact, sessionEnd |
| Codex | `~/.codex/hooks.json` | SessionStart, Stop (trust them once in `/hooks`) |
| OpenCode | `~/.config/opencode/plugins/cadri.ts` | session.created, session.idle |

The hook reads the event from stdin, answers the harness at once, and uploads the session
transcript from a detached child (log: `~/.cadri/hooks.log`). Session start events also start
the memory watcher. Entries are recognized by their `cadri_cli.py … hook` command, so
`cadri init` replaces them and `cadri uninstall` removes only them.

## Install

```sh
curl -fsSL https://cadri.io/install.sh | bash   # newest release
CADRI_REF=main bash plugin/install.sh            # or this checkout / any ref
~/.local/bin/cadri init
```

Localhost: see [../docs/localhost.md](../docs/localhost.md), then `cadri init --local --start-engine`.

## Release

`install.sh` installs the newest `plugin-v*` GitHub release. To publish what is on `main`:

```sh
plugin/scripts/release 0.2.0
```

If `install.sh` changed, copy it to `landingpage/install.sh` and deploy the landing page.

## CLI

```sh
cadri login [--logout]
cadri init [--local [--start-engine]]
cadri uninstall
cadri discover [--json] [--days 30] [--limit 50]
cadri show <thread_id>
cadri memory clone|path|sync [--scope user]
cadri memory write <path> --file f.md -m "message" [--scope user]
cadri hook claude|cursor|codex|opencode   # run by the harnesses
```

## Tests

```sh
python3 -m unittest discover -s plugin/tests
```
