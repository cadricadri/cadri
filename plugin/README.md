# Cadri CLI and agent hooks

Captures Claude Code, Cursor, Codex, and OpenCode sessions into Cadri, backfills recent ones,
and gives agents the memory books through `cadri memory` and the skills in `skills/`.

Uses **[uv](https://github.com/astral-sh/uv)** for Python. Layout:

```text
plugin/
  bin/cadri_cli.py       # entry point; the CLI lives in cadri/cli.py
  bin/cadri              # uv shim for a source checkout
  cadri/                 # auth, upload, capture, discover/, hooks/ (events and harness adapters),
                         # memory/ (books, sync, push, repair, context, digests), install/ (bundles)
  harnesses/             # per-harness manifests, the Cursor rule, and the OpenCode plugin
  agents/                # the cadri-memory-keeper subagent, rendered per harness
  skills/                # cadri-install, cadri-memory, cadri-memory-seed, cadri-memory-join, cadri-memory-repair
  install.sh             # served as https://cadri.io/install.sh
  scripts/build-plugins  # build every harness bundle into dist/plugins
  scripts/release        # publish plugin/ as a GitHub release
```

## How it works

Agents work with a team book and a personal book of Markdown, cloned under `~/.cadri/memory`.
Every harness gets the same four moments:

- **Session start:** checks sign-in (and starts a browser sign-in in the background if it lapsed),
  fast-forwards both books, and gives the agent both root `MEMORY.md` files, a tree of each book,
  and how to use them.
- **Each prompt:** lists memory files that look relevant and reminds the agent to read them first.
  When recent turns are worth remembering, it asks the agent to launch the `cadri-memory-keeper`
  subagent in the background.
- **Each turn's end:** saves a digest of the turn for the keeper, uploads the session, and commits
  and pushes memory edits that have gone quiet.
- **The keeper:** edits the books from the digest and runs `cadri memory push`. A push that
  conflicts leaves a repair worktree and printed directions; `cadri memory push --continue` finishes it.

`cadri init` installs this into each harness. Every hook runs `~/.cadri/bin/cadri hook <event>
--harness <name>` with an explicit timeout; there are no session-end hooks.

| Harness | Installed as | Events |
|---|---|---|
| Claude Code | plugin `cadri@cadri` from the local marketplace `~/.cadri/plugins/claude` (hooks in `~/.claude/settings.json` without the `claude` CLI) | SessionStart, UserPromptSubmit, PostToolUse, Stop, PreCompact |
| Cursor | plugin in `~/.cursor/plugins/local/cadri` | sessionStart, beforeSubmitPrompt, postToolUse, afterFileEdit, afterAgentResponse, stop, preCompact |
| Codex | `~/.codex/hooks.json`, `~/.codex/agents/cadri-memory-keeper.toml`, skills in `~/.agents/skills` (trust the hooks once in `/hooks`) | SessionStart, UserPromptSubmit, PostToolUse, Stop, PreCompact |
| OpenCode | `~/.config/opencode/plugins/cadri.ts` and `agents/cadri-memory-keeper.md` | context, prompt, compaction, tool `execute.after`, session execution events |

Hooks answer the harness at once and do uploads and pushes in a detached child (log:
`~/.cadri/hooks.log`). Init also lets agents edit `~/.cadri/memory` and run `cadri memory`
without prompting (Claude Code permissions, a Codex writable root). Hook entries are
recognized by their `cadri … hook` command, so `cadri init` replaces them and
`cadri uninstall` removes only them, along with the entries and files it added.

## Install

```sh
curl -fsSL https://cadri.io/install.sh | bash   # newest release
CADRI_REF=main bash plugin/install.sh            # or this checkout / any ref
~/.local/bin/cadri init
```

`~/.cadri/install` is a symlink to `~/.cadri/versions/<version>`. After init, a session start checks for a newer release once a day and flips that symlink, then refreshes each harness's skills, hooks, and keeper. `cadri update` does it now. `CADRI_NO_UPDATE=1` skips the daily check. A shim that points at a source checkout is left on that checkout.

Localhost: https://cadri.io/docs/localhost/, or `curl -fsSL https://cadri.io/install-local.sh | bash`.

## Release

`install.sh` installs the newest `plugin-v*` GitHub release. Installed copies check that release once a day and only move forward, so cut one when the plugin is ready to leave running for a day, not on every commit. A broken intermediate commit would be what some machines run until their next check. Engine and web UI commits are not plugin releases.

```sh
plugin/scripts/release 0.2.0
```

If `install.sh` changed, copy it to `landingpage/install.sh` and deploy the landing page. If `scripts/install-local.sh` changed, copy it to `landingpage/install-local.sh` and deploy the landing page.

## CLI

```sh
cadri login [--logout]
cadri init [--local [--start-engine]]
cadri update [--apply]   # download the latest release, or only refresh harness copies
cadri uninstall
cadri discover [--json] [--days 30] [--limit 50]
cadri discover --seed [--json] [--days 60] [--limit 100]   # smaller of the day window and the recency cap; --json includes which git repo to brief
cadri show <thread_id>
cadri memory context [--harness claude]   # what session start gives the agent
cadri memory status                       # sign-in, books, pending edits, repairs
cadri memory sync [--scope team|personal|all]
cadri memory push [-m "message"] [--human] [--continue | --abort]
cadri memory clone|path [--scope personal]
cadri memory write <path> --file f.md -m "message" [--scope personal]
cadri hook <event> --harness <name>       # run by the harnesses
```

## Tests

```sh
python3 -m unittest discover -s plugin/tests
```

Harness-level e2e on a clean macOS VM (Tart), not the host: `scripts/vm-e2e`. Directions and harness gotchas live in the sibling checkout at `cadri-cloud/memories/plugin_vm_e2e.md`.
