---
name: cadri-capture
description: Remind Codex to keep Cadri credentials configured and re-run cadri init after Codex upgrades if session upload stops.
---

# Cadri + Codex

Cadri installs helper files under `~/.codex/cadri` during `cadri init`.

If transcripts stop appearing in Cadri:

1. `cadri login` (hosted) or set `CADRI_URL` / `CADRI_API_TOKEN` for localhost.
2. Re-run `cadri init` to backfill and refresh the install.
