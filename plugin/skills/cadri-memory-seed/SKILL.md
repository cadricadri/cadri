---
name: cadri-memory-seed
description: Turn the user's recent agent sessions into Cadri memories right after cadri init. Use when the Cadri install docs say to seed memory, or when the user asks to learn from past sessions into Cadri.
---

# Seed Cadri memory from past sessions

`cadri init` already uploaded recent sessions and cloned two memory books:

- **Team book** (`cadri memory path`): shared by everyone in the team. Facts about shared code, systems, conventions, and decisions.
- **Personal book** (`cadri memory path --scope user`): this person only. Their preferences, corrections, and ways of working.

Your job is to read a batch of past sessions and write the few lessons that will make every future agent better. Fewer, sharper memories beat many vague ones.

## 1. Look at what exists

```bash
cadri memory clone && cadri memory clone --scope user
cat "$(cadri memory path)/MEMORY.md" 2>/dev/null
cat "$(cadri memory path --scope user)/MEMORY.md" 2>/dev/null
```

Read any files the indexes point to before writing, so you update instead of duplicating.

If `cadri memory` says git is missing, ask the user to run `xcode-select --install`, then continue.

## 2. Pick sessions

```bash
cadri discover --json --days 30 --limit 60
```

Choose up to 15 sessions where a human and an agent did real work. Prefer the newest, and prefer sessions with back-and-forth over one-shot prompts. Skip automated runs, empty sessions, and sessions about the Cadri install itself.

## 3. Read them in parallel

Split the sessions into batches of about five. If you can run subagents, give each batch to one subagent (run at most three at a time) and have it return candidate lessons, not file edits. Otherwise read them yourself, one batch at a time.

Read a session with `cadri show <thread_id>`. Sessions can be large: look at the human's messages first (search for `"role":"user"` or `"type":"user"`), then read around the corrections.

For each session, look for these, in priority order:

1. **Corrections and mistakes.** The human said "no", "that's wrong", "I told you", or undid the agent's work. What rule would have prevented it?
2. **Preferences.** How the human wants things done: tools, style, review habits, what to ask first, what never to do.
3. **Durable facts.** How a system, repo, service, or deploy actually works, especially anything surprising or learned the hard way.
4. **Procedures.** Steps that worked and will be repeated (release, deploy, debugging a known failure).
5. **Contradictions.** Anything that disagrees with an existing memory. The newer, explicit statement from the human wins.

Write each lesson as a rule or fact that generalizes, not as a story of what happened. Drop anything a future agent could learn in seconds by reading the code.

Never store secrets: no keys, tokens, passwords, customer data, or personal details beyond names.

## 4. Decide where each lesson goes

- About this person (their taste, their workflow, how they like to be talked to): **personal book**.
- About shared code, infrastructure, products, or team decisions: **team book**.
- Unsure: personal book. Teammates should not see guesses about one person.

## 5. Write

Book layout:

- An empty team book already has `MEMORY.md` plus `architecture/`, `components/`, `features/`, `incidents/`, `operations/`, and `path-to-prod/` after `cadri init`. Use those top-level directories; do not invent another at the root.
- Root `MEMORY.md` is loaded into every session: keep it a short guide to what the book holds and where. Each directory's `MEMORY.md` is that directory's index: one line per file, a relative link, and when to open it. Neither has frontmatter.
- One Markdown file per topic. A component's notes go in `components/<name>/`. A recurring failure goes in `incidents/<pattern>/`. Each file other than a `MEMORY.md` index starts with:

  ```markdown
  ---
  name: Code review preferences
  description: How Nelson wants reviews written. Read before reviewing or summarizing changes.
  ---
  ```

- Keep files short. Update an existing file rather than adding a near-duplicate.

Write each file through Cadri so the edit is attributed and shared at once. Put the source sessions in the commit message, never in the memory text:

```bash
cat > /tmp/cadri-memory.md <<'EOF'
---
name: ...
description: ...
---
...
EOF
cadri memory write preferences/code-review.md --scope user --file /tmp/cadri-memory.md --agent cadri-seed \
  --session 2f1c… -m "Add code review preferences" -m "Sources: claude_code:2f1c…, cursor:7a9e…"
```

`--session` takes the bare id of the session the memory mostly came from; the Sessions page lists the commit under that session's "Memories generated". Leave out `--scope user` for the team book. Update the directory `MEMORY.md` indexes (and the root `MEMORY.md`, if a new area appeared under an existing top-level folder) the same way once the topic files are written.

Exit status 3 means someone changed the same file since your clone: run `cadri memory sync`, re-read the file, merge your change, and write again.

## 6. Report

Tell the user how many sessions you read and list the memories you wrote, one line each, with which book they went to. Mention that they can review and edit them on the Memories page of the Cadri web app.
