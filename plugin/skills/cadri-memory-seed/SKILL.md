---
name: cadri-memory-seed
description: Turn the user's recent agent sessions into Cadri memories right after cadri init. Use when the Cadri install docs say to seed memory, or when the user asks to learn from past sessions into Cadri. Finish this in the same task as install.
---

# Seed Cadri memory from past sessions

`cadri init` has already uploaded recent sessions and cloned two empty memory books. You write the first lessons and publish them. Do not stop after init.

- **Team book** (`cadri memory path`): shared by everyone in the team. Facts about shared code, systems, conventions, and decisions.
- **Personal book** (`cadri memory path --scope user`): this person only. Their preferences, corrections, and ways of working.

Use the same judgment as steady state. Read `cadri-memory/references/maintain.md` and follow it. That is what the memory keeper uses when a hook wakes it. This run is once, over past sessions, instead of over one live digest.

Fewer, sharper memories beat many vague ones. Often a session contributes nothing. That is a fine outcome.

## 1. Tell the user, then choose the window

Say that seeding usually takes several minutes, and longer for a large window, before you read sessions.

Default window: whichever set is smaller, the last 60 days or the 100 most recent sessions.

```bash
cadri discover --json --seed
```

The stderr line is the count. Seed every session in the JSON. Do not sample a subset.

If the user's install message names a tighter window, pass it and keep the same smaller-of rule:

- "last 10 days or 10 sessions" → `cadri discover --json --seed --days 10 --limit 10`
- only a day count → pass `--days` and leave `--limit` at the default 100
- only a session count → pass `--limit` and leave `--days` at the default 60

A request for a larger window than the default is the same idea: pass the numbers they named. Do not invent a tighter window they did not ask for.

If the command prints no sessions, tell the user there is nothing to seed yet and stop. Do not push an empty layout.

## 2. Read them in parallel

Split the sessions into batches of about five. Run at most three subagents at a time. Each subagent reads its batch with `cadri show <thread_id>` and returns candidate lessons, not file edits. Subagents must not edit either book and must not run `cadri memory`.

Sessions can be large. Read the human's messages first (search for `"role":"user"` or `"type":"user"`), then the surrounding corrections.

For each session, look for these, in the order `maintain.md` cares about:

1. **Corrections.** The human said the agent was wrong, or undid the work. What rule would have prevented it?
2. **Preferences.** How this person wants things done. Personal book.
3. **Decisions and conventions.** Why a system is shaped this way, team rules, how to ship. Team book.
4. **Durable discoveries.** Non-obvious facts that cost real effort and will matter again.

Write each lesson as a rule or fact that generalizes, not as a story of the session. Drop anything a future agent could learn in seconds by reading the code. Never store secrets.

## 3. You write, then one push

You integrate the lessons yourself. Subagent notes are not memory until you put them in the books.

Read both books first (`MEMORY.md`, then the directory indexes) so you update a file instead of adding a near-duplicate. A fresh install has no files yet. Create what the lessons need, including each directory's `MEMORY.md`, in this same pass. Team top-level directories are only `architecture/`, `components/`, `features/`, `incidents/`, `operations/`, and `path-to-prod/`. Do not push a scaffold of empty indexes by itself.

Edit the files in the clones with your file tools. Re-read a file immediately before you change it. Then publish once:

```bash
cadri memory push --agent cadri-memory-seed -m "Seed memories from <N> sessions" -m "Sources: cursor:<id>, claude_code:<id>"
```

Do not pass `--human`. Do not use `cadri memory write`. Leave `--session` unset unless one session is clearly the source of the whole push; the commit message lists the rest.

`--book` defaults to both books. A book with no edits is skipped. Exit status 3 means a conflict: follow `cadri-memory-repair`, then `cadri memory push --continue`.

Do not launch `cadri-memory-keeper` for this seed. Push before you stop, so the quiet hook does not publish the files under a generic message.

## 4. Report

Tell the user how many sessions you read, how long the window was, and list the memories you wrote, one line each, with which book they went to. If nothing was worth keeping, say that. They can review and edit what you wrote on the Memories page.
