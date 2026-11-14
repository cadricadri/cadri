---
name: cadri-memory-join
description: Revise an existing shared team memory book from this person's recent sessions after they join a team. Use when cadri init says the team book already has memories, or when the joining docs say to fold sessions into the shared book. Finish this in the same task as install.
---

# Join a team memory book

`cadri init` has already uploaded this person's recent sessions and cloned two books. The team book already belongs to the team. Your job is to revise it from what these sessions know, and to write this person's personal book. Do not stop after init. Do not load `cadri-memory-seed` — that skill writes the first lessons into an empty team book.

- **Team book** (`cadri memory path`): shared with people who are already using it. Facts about shared code, systems, conventions, and decisions. Expect substantial edits wherever these sessions overlap that book.
- **Personal book** (`cadri memory path --scope user`): this person only, and it starts empty. Their preferences, corrections, and ways of working.

Use the same judgment as steady state. Read `cadri-memory/references/maintain.md` and follow it, including where a fact goes and how files are shaped. This run is once, over past sessions plus the book that is already there.

Fewer, sharper memories beat many vague ones. Often a session contributes nothing. That is a fine outcome. A file you leave unchanged is also a fine outcome.

## 1. Tell the user, then sync

Say that joining usually takes several minutes, and longer when the team book and the session window are both large, before you read anything.

The other person can push while you work. Start from the latest book:

```bash
cadri memory sync
```

## 2. Read the team book before the sessions

Read the team book's root `MEMORY.md`, then each directory `MEMORY.md`, so you know the map. Then read the topic files those indexes point at when the sessions are likely to touch them. On a team whose history covers the same systems, that is most of the book.

The personal book is empty. You will create it in the write pass.

## 3. Choose the window

Default window: whichever set is smaller, the last 60 days or the 100 most recent sessions.

```bash
cadri discover --json --seed
```

The stderr line is the count. Read every session in the JSON. Do not sample a subset.

If the user's message names a tighter window, pass it and keep the same smaller-of rule:

- "last 10 days or 10 sessions" → `cadri discover --json --seed --days 10 --limit 10`
- only a day count → pass `--days` and leave `--limit` at the default 100
- only a session count → pass `--limit` and leave `--days` at the default 60

A request for a larger window than the default is the same idea: pass the numbers they named. Do not invent a tighter window they did not ask for.

If the command prints no sessions, tell the user there is nothing to add yet and stop. Do not push.

## 4. Read the sessions in parallel

Split the sessions into batches of about five. Run at most three subagents at a time. Give each subagent the team-book map (which files exist, and what each is for). Each subagent reads its batch with `cadri show <thread_id>` and returns candidate changes, naming the existing file to revise when there is one. Subagents must not edit either book and must not run `cadri memory`.

Sessions can be large. Read the human's messages first (search for `"role":"user"` or `"type":"user"`), then the surrounding corrections.

For each session, look for these, in the order `maintain.md` cares about:

1. **Corrections.** The human said the agent was wrong, or undid the work. What rule would have prevented it?
2. **Preferences.** How this person wants things done. Personal book.
3. **Decisions and conventions.** Why a system is shaped this way, team rules, how to ship. Team book.
4. **Durable discoveries.** Non-obvious facts that cost real effort and will matter again.

Write each lesson as a rule or fact that generalizes, not as a story of the session. Drop anything a future agent could learn in seconds by reading the code. Never store secrets.

When a candidate contradicts a file you read in step 2, say so, and say which side the sessions support.

## 5. Sync again, then you write

```bash
cadri memory sync
```

Subagent notes are not memory until you put them in the books. Re-read a file immediately before you change it. Someone else may have pushed since step 2.

**Team book.** Revise files that already cover the topic. Rewrite a section the sessions show is stale. Merge a duplicate instead of adding a second file. Add a file only when no existing file owns the fact, and update that directory's `MEMORY.md` in the same pass. Keep a teammate's lesson unless the sessions show it is wrong. Root `MEMORY.md` stays a short discovery guide.

**Personal book.** Create the files this person needs, including `MEMORY.md`. Preferences and corrections go here even when the same sessions also change the team book.

Team top-level directories are only `architecture/`, `components/`, `features/`, `incidents/`, `operations/`, and `path-to-prod/`. Edit the files in the clones with your file tools.

## 6. One push

Publish once, after the edits:

```bash
cadri memory push --agent cadri-memory-join -m "Join: revise team memory from <N> sessions" -m "Sources: cursor:<id>, claude_code:<id>"
```

Do not pass `--human`. Do not use `cadri memory write`. Leave `--session` unset unless one session is clearly the source of the whole push; the commit message lists the rest.

`--book` defaults to both books. A book with no edits is skipped. This push lands on a branch your teammates already have. Exit status 3 means a conflict: follow `cadri-memory-repair`, keep both sides unless one clearly replaces the other, then `cadri memory push --continue`.

Do not launch `cadri-memory-keeper` for this pass. Push before you stop, so the quiet hook does not publish the files under a generic message.

## 7. Report

Tell the user how many sessions you read, how long the window was, which team files you revised, which team files you added, and which personal memories you wrote. One line each. If nothing was worth keeping, say that. They can review and edit what you wrote on the Memories page.
