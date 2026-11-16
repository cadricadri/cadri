---

## name: cadri-memory-seed
description: Writes the first Cadri team and personal memories from recent agent sessions when the team book has no lessons yet. Use when cadri init says to follow cadri-memory-seed, or the install docs say to seed memory. A team book that already has topic files is cadri-memory-join.

# Seed Cadri memory from past sessions

Write the first memories and publish them in this same task. `cadri init` returning is not the end. This task writes the books. The memory keeper does not.

`cadri init` cloned both team and personal books and uploaded the seed window for the Sessions page. `cadri show` reads the local session, so read that same window from disk, including a session the upload skipped because it hit the size cap.

**Input.** Both books are cloned. The default number of sessions is whichever is smaller, the last 60 days or the 100 most recent sessions.

- The team book has a topic file — if any markdown file exists other than the starting `MEMORY.md` or `README.md` then Load and execute the skill `cadri-memory-join`. That is the skill init names when the team book already has memories.
- Init said the books were not cloned. Run `cadri memory clone` and `cadri memory clone --scope personal`, then repeat this check.
- `cadri discover` prints no sessions. Tell the user there is nothing to seed yet and stop. Do not push.

**Output.** How many sessions you read, the window, and each memory you wrote, one line, with its book. If nothing was worth keeping, say that. They review and edit what you wrote on the Memories page.

- **Team book** (`cadri memory path`): shared by everyone. Facts about shared code, systems, conventions, and decisions.
- **Personal book** (`cadri memory path --scope personal`): this person only. Preferences, corrections, and ways of working.

Read `cadri-memory/references/maintain.md` and follow it for what is worth keeping, which book a fact goes in, and how files are shaped. That is the same judgment the memory keeper uses. This run is once, over past sessions. Fewer, sharper memories beat many vague ones. A session that contributes nothing is a fine outcome.

Track this for yourself. The report in section 4 is what the user sees.

- [ ] Tell the user, then list the window
- [ ] Read every session
- [ ] Write the lessons and push once
- [ ] Report

## 1. Tell the user, then choose the window

Say that seeding usually takes several minutes, and longer for a large window, before you read sessions.

```bash
cadri discover --json --seed
```

The stderr line is the count. Read every session in the JSON. Do not sample a subset.

If the user's message names a tighter window, pass it and keep the same smaller-of rule:

- "last 10 days or 10 sessions" → `cadri discover --json --seed --days 10 --limit 10`
- only a day count → pass `--days` and leave `--limit` at the default 100
- only a session count → pass `--limit` and leave `--days` at the default 60

A request for a larger window than the default is the same idea: pass the numbers they named. Do not invent a tighter window they did not ask for.

## 2. Read them in parallel

Split the sessions into batches of about five. Run at most three subagents at a time. Each subagent reads its batch with `cadri show <thread_id>` and returns candidate lessons, not file edits. Subagents must not edit either book and must not run `cadri memory`.

Sessions can be large. Read the human's messages first (search for `"role":"user"` or `"type":"user"`), then the surrounding corrections. Apply `maintain.md` in this order: corrections, preferences (personal book), decisions and conventions (team book), durable discoveries. Write a rule or fact that generalizes. Drop anything a future agent could learn in seconds by reading the code. Never store secrets.

## 3. You write, then one push

You integrate the lessons yourself. Subagent notes are not memory until you put them in the books.

A new book has no files. Init does not publish a scaffold. Create the root `MEMORY.md` and each directory's `MEMORY.md` in the same push as the lessons that need them. Do not push a scaffold of empty indexes by itself. If a book already has files, read the root `MEMORY.md`, the directory indexes, and the topic files you will change, so you update a file instead of adding a near-duplicate.

Team top-level directories are only `architecture/`, `components/`, `features/`, `incidents/`, `operations/`, `path-to-prod/`, and `external/`.

Edit the files in the clones with your file tools. Re-read a file immediately before you change it. Publish once when a file changed. When nothing was worth keeping, leave the books untouched and do not push.

```bash
cadri memory push --agent cadri-memory-seed -m "Seed memories from <N> sessions" -m "Sources: cursor:<id>, claude_code:<id>"
```

Do not pass `--human`. Do not use `cadri memory write`. Leave `--session` unset unless one session is clearly the source of the whole push; the commit message lists the rest.

`--book` defaults to both books. A book with no edits is skipped. Exit status 3 means a conflict: follow `cadri-memory-repair`, then `cadri memory push --continue`.

Do not launch `cadri-memory-keeper`. When you edited, push before you stop, so the quiet hook does not publish the files under a generic message.

## 4. Report

```text
Read 12 sessions, the last 60 days.
- team path-to-prod/deploy-freeze.md — freeze rules before a release
- personal review.md — ask before a refactor
```
