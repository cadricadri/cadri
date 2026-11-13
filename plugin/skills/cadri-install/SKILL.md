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
4. Tell the user their sessions are showing up on the Sessions page in Cadri. The page opens as soon as the first upload lands. Seeding continues after that.
5. Before you read any sessions, tell the user that seeding memory usually takes several minutes, and longer when the window is large. Then load the `cadri-memory-seed` skill and finish it in this same task. `cadri init` returning is not the end of the install.

If the user's message names a tighter seed window, pass it through to that skill. Otherwise the default is whichever is smaller: the last 60 days, or the 100 most recent sessions.

To remove Cadri's hooks and skills later: `cadri uninstall`.
