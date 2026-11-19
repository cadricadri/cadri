---
name: cadri-memory-join
description: Revises an existing shared Cadri team memory book from the repository the seed window names and from this person's recent sessions, and writes or revises their personal book. Use when cadri init says the team book already has memories, or the joining docs say to fold sessions into the shared book. An empty team book is cadri-memory-seed.
---

# Join a team memory book

Revise the team book from this repository and from these sessions, and write or revise the personal book, in this same task. `cadri init` returning is not the end. This task writes the books. The memory keeper does not. Do not load `cadri-memory-seed`. That skill writes the first lessons into an empty team book.

`cadri init` cloned both books and uploaded the seed window for the Sessions page. `cadri show` reads the local session, so read that same window from disk, including a session the upload skipped because it hit the size cap. The team book already belongs to the team. Expect edits where the portrait is missing or stale, and wherever these sessions overlap the book.

**Input.** Both books are cloned. The team book has memories. The personal book may be empty or may already have files. Any new portrait comes from `repos.brief` in `cadri discover --json --seed`, a git root the seed window was working in. The current directory is that repo only when it sits inside that root. The user may name a window. The default is whichever set is smaller, the last 60 days or the 100 most recent sessions. Teammates can push while you work.

**Stop.** `repos.reason` is `none`, `sessions` is empty, and the book needs nothing from this run: tell the user there is nothing to add yet and stop. Do not push. Nothing in the named repo or the sessions is worth keeping: leave the books untouched and do not push.

**Output.** Which repo docs you read, how many subsystems you split, how many sessions you read, the window, which team files you added, which you revised, which portrait files you skipped and why, and which personal memories you wrote. One line each. If nothing was worth keeping, say that. They review and edit what you wrote on the Memories page.

- **Team book** (`cadri memory path`): shared with people who are already using it. The repo briefing where the book does not already own it, then facts about shared code, systems, conventions, and decisions.
- **Personal book** (`cadri memory path --scope personal`): this person only. Preferences, corrections, and ways of working. On a first join it has no lessons. When it already has files, revise those. The repo pass does not write this book.

Read `cadri-memory/references/maintain.md` and `cadri-memory/references/orient.md`. Follow both. The team book you read first is part of what the portrait already owns. Fewer, sharper memories beat many vague ones. A session that contributes nothing is a fine outcome. A file you leave unchanged is a fine outcome.

Track this for yourself. The report in section 8 is what the user sees.

- [ ] Sync, then read the team book
- [ ] Choose the window and the repo
- [ ] Read that repo against the book
- [ ] Read every session
- [ ] Sync again, then revise and push each learning
- [ ] Report

## 1. Tell the user, then sync

Say that joining usually takes several minutes, and longer when the repo, the team book, and the session window are large, before you read anything. You read the team book, choose the repo from their sessions, read that repo, then read the sessions.

```bash
cadri memory sync
```

## 2. Read the team book before the repo

Read the team book's root `MEMORY.md`, then each directory `MEMORY.md`, so you know the map. Then read the topic files those indexes point at. On a team whose history covers the same systems, that is most of the book. Those files are already owned.

Read the personal book the same way when it has files. When it has no lessons, you create it in the write pass.

## 3. Choose the window and the repo

```bash
cadri discover --json --seed
```

Stderr is the count and the same repo choice `cadri init` printed. The JSON is an object. `sessions` is the whole window. `repos` is the choice. Do not sample a subset of `sessions`.

- `repos.reason` is `ask`. Tell the user the paths and counts in `repos.ask`, and wait. Brief the one they name. When they don't pick, skip the portrait.
- `repos.reason` is `none`. Skip the portrait. When `sessions` is also empty and the book needs nothing from this run, stop as above.
- Otherwise brief `repos.brief`. When that path is the repo root `MEMORY.md` already names, revise those files. When it is a different path, add files under that repo's directory name, as `orient.md` describes, and leave the first repo's files in place. Leave every other path in `repos.seen` unread.

When `sessions` is empty and there is a repo to brief, skip section 5 and write the portrait gaps from that repo.

If the user's message names a tighter window, pass it and keep the same smaller-of rule:

- "last 10 days or 10 sessions" → `cadri discover --json --seed --days 10 --limit 10`
- only a day count → pass `--days` and leave `--limit` at the default 100
- only a session count → pass `--limit` and leave `--days` at the default 60

A request for a larger window than the default is the same idea: pass the numbers they named. Do not invent a tighter window they did not ask for.

## 4. Read that repository

Follow `cadri-memory/references/orient.md` in the repo section 3 named. Draft a file only for a subject no existing file owns. Mark a section the code or docs show is stale. Keep a teammate's lesson unless the repo shows it is wrong. A paraphrase of the README is not a reason to replace their file. Hold the draft. The book is still untouched. When section 3 skipped the portrait, skip this section.

## 5. Read the sessions in parallel

Split the sessions into batches of about five. Run at most three subagents at a time. Give each subagent the team-book map (which files exist, and what each is for) and the portrait draft. Each subagent reads its batch with `cadri show <thread_id>` and returns candidate changes, naming the existing file to revise when there is one. Subagents must not edit either book and must not run `cadri memory`.

Sessions can be large. Read the human's messages first (search for `"role":"user"` or `"type":"user"`), then the surrounding corrections. Also search for a plan: `CreatePlan`, `ExitPlanMode`, a write under `~/.claude/plans` or `~/.cursor/plans`, or a `*.plan.md` file. An accepted plan's decisions follow the Plans section of `maintain.md`. Also search for a canvas: a write whose path contains `/canvases/` and ends in `.canvas.tsx`. An accepted canvas's decisions follow the Canvases section of `maintain.md`. Apply `maintain.md` in this order: corrections, preferences (personal book), decisions and conventions (team book), durable discoveries. Write a rule or fact that generalizes. A session note that only restates the book or the portrait draft is already recorded. Never store secrets.

When a candidate contradicts a file you read in step 2, or the docs you read in step 4, say so, and say which side the sessions support.

## 6. Sync again, then you write

```bash
cadri memory sync
```

Subagent notes are not memory until you put them in the books. Re-read a file immediately before you change it. Someone else may have pushed since step 2.

Push portrait gaps and stale revisions first, one learning at a time, architecture before components and features. Then push each session lesson.

**Team book.** Add a portrait file only when no existing file owns the subject, and update that directory's `MEMORY.md` in the same pass. Revise a section the code or the docs show is stale. Keep a teammate's lesson unless the repo or these sessions show it is wrong. When a session and the docs disagree, the session wins if someone corrected the docs. The current code wins if the session describes a structure that is gone. Merge a duplicate instead of adding a second file. Root `MEMORY.md` stays a short discovery guide: the repo path, a few sentences on what the product is and who it is for, then one line per directory.

**Personal book.** If it already has files, revise them and do not add a second file for the same fact. If it has no lessons, create the files this person needs, including `MEMORY.md`. Preferences and corrections go here even when the same sessions also change the team book. The repo pass does not write this book.

Team top-level directories are only `architecture/`, `components/`, `features/`, `incidents/`, `operations/`, `path-to-prod/`, and `external/`. Edit the files in the clones with your file tools.

## 7. Push each learning

Publish each learning before you edit the next. A learning may touch several files, including the indexes that link a new or revised file. When nothing was worth keeping, do not push.

The subject, body, and `Sources:` rules are in `cadri-memory/references/maintain.md` under How to write the commit, including Repository briefing. A learning that comes only from the repo omits `Sources:` and names the files in the body.

```bash
cadri memory push --agent cadri-memory-join --book team -m "<the learning>" -m "<what the repo says, why it belongs in the book, and the file paths>"
```

When a session justifies the learning, add `Sources:` as below. `Sources:` lists the sessions that justified this learning, not the whole window. A path is not a session id.

```bash
cadri memory push --agent cadri-memory-join --book <team|personal> -m "<the learning>" -m "<what happened and why it should persist>" -m "Sources: cursor:<id>, claude_code:<id>
- cursor:<id> (<YYYY-MM-DD>): <what happened there>"
```

Do not pass `--human`. Do not use `cadri memory write`. Leave `--session` unset unless one session is clearly the source of this learning.

Each push lands on the shared branch your teammates already have. Exit status 3 means a conflict: follow `cadri-memory-repair`, then `cadri memory push --continue --book <team|personal>`, then the next learning.

Do not launch `cadri-memory-keeper`. Push each learning before you stop, so the quiet hook does not publish the files under a generic message.

## 8. Report

```text
Briefed /Users/me/app (40 sessions in the window).
Read the repo (README, CLAUDE.md, 4 subsystems) and 12 sessions, the last 60 days.
- added team architecture/component-map.md — gateway, workers, web
- revised team architecture/conventions.md — the layer rule was stale
- skipped team external/customers.md — README has no real audience
- personal review.md — ask before a refactor
```
