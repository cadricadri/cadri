---
name: cadri-memory-seed
description: Writes the first Cadri team briefing from the repository the seed window names, then team and personal memories from recent agent sessions, when the team book has no lessons yet. Use when cadri init says to follow cadri-memory-seed, or the install docs say to seed memory. A team book that already has topic files is cadri-memory-join.
---

# Seed Cadri memory from the repo and past sessions

Write the first memories and publish them in this same task. `cadri init` returning is not the end. This task writes the books. The memory keeper does not.

`cadri init` cloned both team and personal books and uploaded the seed window for the Sessions page. `cadri show` reads the local session, so read that same window from disk, including a session the upload skipped because it hit the size cap.

**Input.** Both books are cloned. The team briefing comes from `repos.brief` in `cadri discover --json --seed`, a git root the seed window was working in. The current directory is that repo only when it sits inside that root. The default number of sessions is whichever is smaller, the last 60 days or the 100 most recent sessions.

- The team book has a topic file — if any markdown file exists other than the starting `MEMORY.md` or `README.md` then Load and execute the skill `cadri-memory-join`. That is the skill init names when the team book already has memories.
- Init said the books were not cloned. Run `cadri memory clone` and `cadri memory clone --scope personal`, then repeat this check.
- `repos.reason` is `none` and `sessions` is empty. Tell the user there is nothing to seed yet and stop. Do not push.

**Output.** Which repo docs you read, how many subsystems you split, how many sessions you read, the window, and each memory you wrote, one line, with its book. Name each portrait file you skipped, and why. If nothing was worth keeping, say that. They review and edit what you wrote on the Memories page.

- **Team book** (`cadri memory path`): shared by everyone. The repo briefing, then facts about shared code, systems, conventions, and decisions.
- **Personal book** (`cadri memory path --scope personal`): this person only. Preferences, corrections, and ways of working. The repo pass does not write this book.

Read `cadri-memory/references/maintain.md` and `cadri-memory/references/orient.md`. Follow both. The portrait may restate the repo. Session lessons are the corrections, decisions, and preferences the portrait does not already contain. Fewer, sharper memories beat many vague ones. A session that contributes nothing is a fine outcome.

Track this for yourself. The report in section 6 is what the user sees.

- [ ] Tell the user
- [ ] Choose the window and the repo
- [ ] Read that repo and draft the portrait
- [ ] Read every session
- [ ] Write the portrait, then each session lesson, and push each before the next
- [ ] Report

## 1. Tell the user

Say that seeding usually takes several minutes, and longer for a large repo or a large window, before you read. You choose the repo from their sessions, read that repo, then read the sessions.

## 2. Choose the window and the repo

```bash
cadri discover --json --seed
```

Stderr is the count and the same repo choice `cadri init` printed. The JSON is an object. `sessions` is the whole window. `repos` is the choice. Do not sample a subset of `sessions`.

- `repos.reason` is `ask`. Tell the user the paths and counts in `repos.ask`, and wait. Brief the one they name. When they don't pick, skip the portrait.
- `repos.reason` is `none`. Skip the portrait. When `sessions` is also empty, tell the user there is nothing to seed yet and stop. Do not push.
- Otherwise brief `repos.brief`. Leave every other path in `repos.seen` unread.

When `sessions` is empty and there is a repo to brief, skip section 4 and write the portrait from that repo.

If the user's message names a tighter window, pass it and keep the same smaller-of rule:

- "last 10 days or 10 sessions" → `cadri discover --json --seed --days 10 --limit 10`
- only a day count → pass `--days` and leave `--limit` at the default 100
- only a session count → pass `--limit` and leave `--days` at the default 60

A request for a larger window than the default is the same idea: pass the numbers they named. Do not invent a tighter window they did not ask for.

## 3. Read that repository

Follow `cadri-memory/references/orient.md` in the repo section 2 named. Draft the portrait before you open sessions. Hold the draft. The book is still untouched. When section 2 skipped the portrait, skip this section.

## 4. Read them in parallel

Split the sessions into batches of about five. Run at most three subagents at a time. Give each subagent the portrait outline: which files you will write, and what each will say. Each subagent reads its batch with `cadri show <thread_id>` and returns corrections, decisions, and preferences the portrait does not already contain. Subagents must not edit either book and must not run `cadri memory`.

`cadri discover --seed` omits the session that is running the seed. Do not read that session, and do not cite it. Sessions can be large. Read the human's messages first (search for `"role":"user"` or `"type":"user"`), then the surrounding corrections. Run `cadri show` once per session and return from that read. Do not re-parse a session you already printed, and do not read Cadri's source to change how a commit is linked. Also search for a plan: `CreatePlan`, `ExitPlanMode`, a write under `~/.claude/plans` or `~/.cursor/plans`, or a `*.plan.md` file. An accepted plan's decisions follow the Plans section of `maintain.md`. Also search for a canvas: a write whose path contains `/canvases/` and ends in `.canvas.tsx`. An accepted canvas's decisions follow the Canvases section of `maintain.md`. Apply `maintain.md` in this order: corrections, preferences (personal book), decisions and conventions (team book), durable discoveries. Write a rule or fact that generalizes. A session note that only restates the portrait is already in the draft. Never store a secret. Follow Secrets in `maintain.md`: name the credential, never the value.

## 5. You write, portrait first

You integrate the notes yourself. Subagent notes are not memory until you put them in the books.

Cursor asks for approval every time the editor creates or edits a file outside the workspace, and the books live under `~/.cadri`. Do not use Write, StrReplace, or Edit on those paths. Read a book file with the Read tool. In one shell command, write that learning's files into the clone (`cadri memory path` or `cadri memory path --scope personal`) and run `cadri memory push` for it.

Push the portrait first, one learning at a time. Architecture before components and features, so the root `MEMORY.md` and the directory indexes land with the first real file. Then push each session lesson. A learning may touch several files, including the indexes that link a new file. Re-read a file immediately before you change it.

A new book has no files. Init does not publish a scaffold. Create the root `MEMORY.md` and each directory's `MEMORY.md` in the same push as the first lesson that needs them. Do not push a scaffold of empty indexes by itself. If a book already has files, read the root `MEMORY.md`, the directory indexes, and the topic files you will change, so you update a file instead of adding a near-duplicate.

Team top-level directories are only `architecture/`, `components/`, `features/`, `incidents/`, `operations/`, `path-to-prod/`, and `external/`.

When nothing was worth keeping, leave the books untouched and do not push.

The subject, body, and `Sources:` rules are in `cadri-memory/references/maintain.md` under How to write the commit, including Repository briefing. A learning that comes only from the repo omits `Sources:` and names the files in the body.

```bash
cadri memory push --agent cadri-memory-seed --book team -m "<the learning>" -m "<what the repo says, why it belongs in the book, and the file paths>"
```

When a session justifies the learning, add `Sources:` as below. `Sources:` lists the sessions that justified this learning, not the whole window. A path is not a session id.

```bash
cadri memory push --agent cadri-memory-seed --book <team|personal> -m "<the learning>" -m "<what happened and why it should persist>" -m "Sources: cursor:<id>, claude_code:<id>
- cursor:<id> (<YYYY-MM-DD>): <what happened there>"
```

Do not pass `--human`. Do not use `cadri memory write`. Leave `--session` unset unless one session is clearly the source of this learning.

Exit status 3 means a conflict: follow `cadri-memory-repair`, then `cadri memory push --continue --book <team|personal>`, then the next learning.

Do not launch `cadri-memory-keeper`. Push each learning before you stop, so the quiet hook does not publish the files under a generic message.

## 6. Report

```text
Briefed /Users/me/app (40 sessions in the window).
Read the repo (README, CLAUDE.md, 4 subsystems) and 12 sessions, the last 60 days.
- team architecture/component-map.md — gateway, workers, web
- team components/gateway/how-it-works.md — public entry
- team features/memory-books/why.md — shared lessons across agents
- team external/customers.md — skipped, README has no real audience
- personal review.md — ask before a refactor
```
