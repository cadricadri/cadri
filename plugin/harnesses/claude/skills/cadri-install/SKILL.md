---
name: cadri-install
description: Follow Cadri hosted installation docs — run install.sh / cadri init, confirm harness plugins, then seed memories. Use when the user pastes a prompt pointing at cadri.io/docs/installation.
---

# Cadri install (agent)

1. Open https://cadri.io/docs/installation (or the URL the user named) and follow it.
2. Typical hosted steps:
   - `curl -fsSL https://cadri.io/install.sh | bash` (or run `plugin/install.sh` from a checkout)
   - Ensure `~/.local/bin` is on `PATH`
   - `cadri init` (browser device login once, then backfill + harness install)
3. Tell the user the Cadri Transcripts tab should update.
4. Run `cadri-memory-seed` for initial memories.
5. Confirm detected harness plugins were installed; finish any that failed.
