---
name: cadri-install
description: Installs Cadri on this machine from cadri.io/docs/installation or cadri.io/docs/joining, then seeds or joins memory in the same task. Use when the user pastes a prompt from those docs, or asks to set up, install, or repair the Cadri install. A memory push conflict is cadri-memory-repair.
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
   - If memory was skipped because git is missing, or init says the books were not cloned, explain that git is Cadri's memory storage and ask the user to run `xcode-select --install` when git is missing. Then run `cadri memory clone` and `cadri memory clone --scope personal`. Do not seed or join until both clones exist.
4. Tell the user their sessions are showing up on the Sessions page, and print the link: https://app.cadri.io/sessions . The page opens as soon as this person's sessions arrive. Memory work continues after that.
5. Before you read the repo or any sessions, tell the user this usually takes several minutes, and longer when the repo or the window is large. Explain that Cadri reads the repository `cadri init` names — the git root their recent sessions were working in — to write a briefing of the architecture, components, features, and customers, and learns from their previous sessions. When two repos are close, ask which to brief before reading. When init names no repo, skip the briefing. When `cadri init` names `cadri-memory-join`, also say you will read the team book they are joining, so a large book takes longer. Load the skill init names and finish it in this same task. `cadri init` returning is not the end of the install.
   - `cadri-memory-join` — the team book already has memories. Revise that shared book from this repo and these sessions, and write or revise the personal book.
   - `cadri-memory-seed` — the team book has no lessons yet. Write the first briefing from this repo, then the session lessons.

   If init said the books were not cloned, clone them first, then choose: a markdown file in the team book other than `MEMORY.md` or `README.md` means `cadri-memory-join`. Otherwise `cadri-memory-seed`.

If the user's message names a tighter window, pass it through to that skill. Otherwise the default is whichever is smaller: the last 60 days, or the 100 most recent sessions.

To remove Cadri's hooks and skills later: `cadri uninstall`.
