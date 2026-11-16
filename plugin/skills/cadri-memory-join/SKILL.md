---
name: cadri-memory-join
description: Revises an existing shared Cadri team memory book from this person's recent sessions, and writes or revises their personal book. Use when cadri init says the team book already has memories, or the joining docs say to fold sessions into the shared book. An empty team book is cadri-memory-seed.
---

# Join a team memory book

Revise the team book from these sessions, and write or revise the personal book, in this same task. `cadri init` returning is not the end. This task writes the books. The memory keeper does not. Do not load `cadri-memory-seed`. That skill writes the first lessons into an empty team book.

`cadri init` cloned both books and uploaded the seed window for the Sessions page. `cadri show` reads the local session, so read that same window from disk, including a session the upload skipped because it hit the size cap. The team book already belongs to the team. Expect substantial edits wherever these sessions overlap it.

**Input.** Both books are cloned. The team book has memories. The personal book may be empty or may already have files. The user may name a window. The default is whichever set is smaller, the last 60 days or the 100 most recent sessions. Teammates can push while you work.

**Stop.** `cadri discover` prints no sessions: tell the user there is nothing to add yet and stop. Do not push. Nothing in the sessions is worth keeping: leave the books untouched and do not push.

**Output.** How many sessions you read, the window, which team files you revised, which team files you added, and which personal memories you wrote. One line each. If nothing was worth keeping, say that. They review and edit what you wrote on the Memories page.

- **Team book** (`cadri memory path`): shared with people who are already using it. Facts about shared code, systems, conventions, and decisions.
- **Personal book** (`cadri memory path --scope personal`): this person only. Preferences, corrections, and ways of working. On a first join it has no lessons. When it already has files, revise those.

Read `cadri-memory/references/maintain.md` and follow it, including where a fact goes and how files are shaped. This run is once, over past sessions plus the book that is already there. Fewer, sharper memories beat many vague ones. A session that contributes nothing is a fine outcome. A file you leave unchanged is a fine outcome.

Track this for yourself. The report in section 7 is what the user sees.

- [ ] Sync, then read the team book
- [ ] List the window and read every session
- [ ] Sync again, then revise and push once
- [ ] Report

## 1. Tell the user, then sync

Say that joining usually takes several minutes, and longer when the team book and the session window are both large, before you read anything.

```bash
cadri memory sync
```

## 2. Read the team book before the sessions

Read the team book's root `MEMORY.md`, then each directory `MEMORY.md`, so you know the map. Then read the topic files those indexes point at when the sessions are likely to touch them. On a team whose history covers the same systems, that is most of the book.

Read the personal book the same way when it has files. When it has no lessons, you create it in the write pass.

## 3. Choose the window

```bash
cadri discover --json --seed
```

The stderr line is the count. Read every session in the JSON. Do not sample a subset. If the command prints no sessions, stop as above.

If the user's message names a tighter window, pass it and keep the same smaller-of rule:

- "last 10 days or 10 sessions" → `cadri discover --json --seed --days 10 --limit 10`
- only a day count → pass `--days` and leave `--limit` at the default 100
- only a session count → pass `--limit` and leave `--days` at the default 60

A request for a larger window than the default is the same idea: pass the numbers they named. Do not invent a tighter window they did not ask for.

## 4. Read the sessions in parallel

Split the sessions into batches of about five. Run at most three subagents at a time. Give each subagent the team-book map (which files exist, and what each is for). Each subagent reads its batch with `cadri show <thread_id>` and returns candidate changes, naming the existing file to revise when there is one. Subagents must not edit either book and must not run `cadri memory`.

Sessions can be large. Read the human's messages first (search for `"role":"user"` or `"type":"user"`), then the surrounding corrections. Apply `maintain.md` in this order: corrections, preferences (personal book), decisions and conventions (team book), durable discoveries. Write a rule or fact that generalizes. Drop anything a future agent could learn in seconds by reading the code. Never store secrets.

When a candidate contradicts a file you read in step 2, say so, and say which side the sessions support.

## 5. Sync again, then you write

```bash
cadri memory sync
```

Subagent notes are not memory until you put them in the books. Re-read a file immediately before you change it. Someone else may have pushed since step 2.

**Team book.** Revise files that already cover the topic. Rewrite a section the sessions show is stale. Merge a duplicate instead of adding a second file. Add a file only when no existing file owns the fact, and update that directory's `MEMORY.md` in the same pass. Keep a teammate's lesson unless the sessions show it is wrong. Root `MEMORY.md` stays a short discovery guide.

**Personal book.** If it already has files, revise them and do not add a second file for the same fact. If it has no lessons, create the files this person needs, including `MEMORY.md`. Preferences and corrections go here even when the same sessions also change the team book.

Team top-level directories are only `architecture/`, `components/`, `features/`, `incidents/`, `operations/`, `path-to-prod/`, and `external/`. Edit the files in the clones with your file tools.

## 6. One push

Publish once, after the edits, when a file changed. When nothing was worth keeping, do not push.

```bash
cadri memory push --agent cadri-memory-join -m "Join: revise team memory from <N> sessions" -m "Sources: cursor:<id>, claude_code:<id>"
```

Do not pass `--human`. Do not use `cadri memory write`. Leave `--session` unset unless one session is clearly the source of the whole push; the commit message lists the rest.

`--book` defaults to both books. A book with no edits is skipped. This push lands on the shared branch your teammates already have. Exit status 3 means a conflict: follow `cadri-memory-repair`, then `cadri memory push --continue`.

Do not launch `cadri-memory-keeper`. When you edited, push before you stop, so the quiet hook does not publish the files under a generic message.

## 7. Report

```text
Read 12 sessions, the last 60 days.
- revised team architecture/deploy-freeze.md — freeze is before the release cut
- added team incidents/startup-loops.md — the retry loop
- personal review.md — ask before a refactor
```
