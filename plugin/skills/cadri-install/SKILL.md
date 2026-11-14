---
name: cadri-install
description: Install Cadri on this machine by following cadri.io/docs/installation or cadri.io/docs/joining. Use when the user pastes a prompt pointing at those docs, or asks to set up or repair Cadri.
---

# Install Cadri

1. Read the URL the user gave. If they did not give one, read https://cadri.io/docs/installation. The commands are the same either way:

   ```bash
   curl -fsSL https://cadri.io/install.sh | bash
   ~/.local/bin/cadri init
   ```

2. `cadri init` opens a browser once so the user can approve this machine. Tell the user to approve it, and wait.
3. Read the summary `cadri init` prints:
   - Every agent it lists should show where its hooks went. Report any that say "not installed" and why.
   - If Codex is listed, tell the user to open Codex, type `/hooks`, and trust the Cadri hooks.
   - If memory was skipped because git is missing, ask the user to run `xcode-select --install`, then run `cadri memory clone` and `cadri memory clone --scope user`.
4. Tell the user their sessions are showing up on the Sessions page in Cadri. The page opens as soon as this person's sessions arrive. Memory work continues after that.
5. Before you read any sessions, tell the user this usually takes several minutes, and longer when the window is large. When `cadri init` names `cadri-memory-join`, also say you will read the team book they are joining, so a large book takes longer. Then load the skill `cadri init` named and finish it in this same task. `cadri init` returning is not the end of the install.
   - `cadri-memory-join` — the team book already has memories. Revise that shared book from these sessions, and write the personal book.
   - `cadri-memory-seed` — the team book has no lessons yet. Write the first ones.

If git was missing and you clone afterwards, choose from the clone: a markdown file in the team book other than `MEMORY.md` or `README.md` means `cadri-memory-join`.

If the user's message names a tighter window, pass it through to that skill. Otherwise the default is whichever is smaller: the last 60 days, or the 100 most recent sessions.

To remove Cadri's hooks and skills later: `cadri uninstall`.
