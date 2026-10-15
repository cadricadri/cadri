# Cadri plugin for Claude Code

Uploads the full Claude Code session transcript to a Cadri server on `SessionStart`,
`Stop`, `PreCompact`, and `SessionEnd`. The hook never blocks your session: if the server
is unreachable or rejects the upload, it prints a `cadri:` message on stderr and exits 0.

Requires `python3` (3.9 or newer, standard library only).

## Install

Load the plugin from a checkout of this repo:

```sh
claude --plugin-dir /path/to/cadri/plugin
```

## Choose a server

The hook sends transcripts to `CADRI_URL`, which defaults to the hosted service at
`https://app.cadri.io`. To use another server, set it in your shell profile or in the
`env` block of `~/.claude/settings.json`:

```json
{ "env": { "CADRI_URL": "http://127.0.0.1:8080" } }
```

## Hosted Cadri: sign in

Run the login helper once per machine:

```sh
/path/to/cadri/plugin/bin/cadri-login
```

It prints a code and a URL, opens your browser, and waits for you to confirm. Credentials
are saved to `~/.config/cadri/credentials.json` (mode 0600), keyed by server URL, and
refreshed automatically by the hook. Options:

- `--url URL`: sign in to a server other than `CADRI_URL`.
- `--no-browser`: only print the URL (useful over SSH).
- `--logout`: remove the saved credentials for the server.

## Self-hosted Cadri: API token

If your engine runs with `CADRI_API_TOKEN` set, give the hook the same token:

```json
{
  "env": {
    "CADRI_URL": "https://cadri.internal.example",
    "CADRI_API_TOKEN": "<token>"
  }
}
```

`CADRI_API_TOKEN` takes precedence over saved credentials. If neither is present, the hook
sends no `Authorization` header, which works with an engine running in open (`none`) mode.

## Tests

```sh
python3 -m unittest discover -s plugin/tests
```
