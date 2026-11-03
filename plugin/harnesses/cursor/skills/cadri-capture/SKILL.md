---
name: cadri-capture
description: Keep Cursor sessions flowing into Cadri after cadri init; re-run init if uploads stop.
---

# Cadri + Cursor

`cadri init` copies Cursor helpers under `~/.cursor/cadri` and an MCP stub.

If the Cadri Transcripts tab stays empty:

1. Confirm `cadri login` completed for your `CADRI_URL`.
2. Re-run `cadri init` to backfill `~/.cursor/projects/*/agent-transcripts`.
