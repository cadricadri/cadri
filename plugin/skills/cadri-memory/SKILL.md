---
name: cadri-memory
description: Use the team's and this person's Cadri memory. Load when you need the full procedure for searching memory, when the user asks you to remember or forget something, or when you edit memory files yourself.
---

# Cadri memory

Cadri keeps two memory books on this machine as git clones:

- **Team book** — facts, decisions, and lessons the whole team shares. Path: `cadri memory path`.
- **Personal book** — this person's preferences and working style. Path: `cadri memory path --scope user`.

Each book has a `MEMORY.md` at its root that every session loads, directories with their own `MEMORY.md` index, and topic files that start with `name` and `description` frontmatter. The Cadri plugin loads both root `MEMORY.md` files and a tree of the books at session start, and points you at likely files on each prompt. The team book's top-level directories are fixed (`architecture`, `components`, `features`, `incidents`, `operations`, `path-to-prod`); subfolders under those are free-form.

## Read

Follow `references/search.md`. In short: open the candidate files you were given that fit the task, follow links and directory `MEMORY.md` indexes from there, and stop once you have what you need. Treat memory as strong defaults, not proof; if something is clearly out of date, say so.

## Write

Most memory upkeep is done by the `cadri-memory-keeper` subagent, in the background, when the Cadri plugin asks you to launch it. The keeper decides *what* is worth keeping by reading the turn digest against `references/maintain.md` — not by keyword rules. Often it changes nothing. Write yourself only when:

- the user explicitly asks you to remember, forget, or correct something; or
- you are the keeper.

Follow `references/maintain.md`. Edit the files in the book directly with targeted edits, then publish:

```bash
cadri memory push -m "What changed and why"            # your own edits
cadri memory push --human -m "What the user asked"     # the user told you to remember this
```

`push` commits only this session's files, rebases, and pushes. If it reports a conflict, load the `cadri-memory-repair` skill. If it says the server was not reachable, the commit is saved and Cadri pushes it after the turn. Never run `git commit`, `git push`, or `git reset` in a book yourself.

If no Cadri memory context is in your conversation (for example the session-start hook did not run), run `cadri memory context` and read its output.
