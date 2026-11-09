---
name: cadri-install
description: Install Cadri on this machine by following cadri.io/docs/installation. Use when the user pastes a prompt pointing at the Cadri installation docs, or asks to set up or repair Cadri.
---

# Install Cadri

1. Read https://cadri.io/docs/installation (or the URL the user gave) and follow it. The short version:

   ```bash
   curl -fsSL https://cadri.io/install.sh | bash
   ~/.local/bin/cadri init
   ```

2. `cadri init` opens a browser once so the user can approve this machine. Tell the user to approve it, and wait.
3. Read the summary `cadri init` prints:
   - Every agent it lists should show where its hooks went. Report any that say "not installed" and why.
   - If Codex is listed, tell the user to open Codex, type `/hooks`, and trust the Cadri hooks.
   - If memory was skipped because git is missing, ask the user to run `xcode-select --install`, then run `cadri memory clone` and `cadri memory clone --scope user`.
4. Tell the user their sessions are showing up on the Sessions page in Cadri.
5. Load the `cadri-memory-seed` skill and follow it.

To remove Cadri's hooks and skills later: `cadri uninstall`.
