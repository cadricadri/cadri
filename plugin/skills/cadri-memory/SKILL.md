---
name: cadri-memory
description: Searches the Cadri team and personal memory books, and edits them when the user asks to remember, forget, or correct something. Use when a prompt says Cadri remembered something, or when editing memory files. First memories after install are cadri-memory-seed or cadri-memory-join. A push conflict is cadri-memory-repair.
---

# Cadri memory

Cadri keeps two memory books on this machine as git clones:

- **Team book** — facts, decisions, and lessons the whole team shares. Path: `cadri memory path`.
- **Personal book** — this person's preferences and working style. Path: `cadri memory path --scope personal`.

Each book has a `MEMORY.md` at its root that every session loads, directories with their own `MEMORY.md` index, and topic files that start with `name` and `description` frontmatter. The Cadri plugin loads both root `MEMORY.md` files and a tree of the books at session start. On a prompt that matches, it asks you to call the `cadri_search` tool, which is what the user sees. The team book's top-level directories are fixed (`architecture`, `components`, `features`, `incidents`, `operations`, `path-to-prod`, `external`); subfolders under those are free-form.

## Read

Follow `references/search.md`. In short: when a prompt says Cadri remembered something, call `cadri_search` before other work. Then search the books with grep, the way you search a codebase. Read a directory `MEMORY.md` when you do not know the words, and when the change must fit the existing design read `architecture/MEMORY.md` and the relevant `features/` index. Read the matching lines. Read a whole file when it is an index, or when the hits show you need the whole argument. Stop once you have what you need. Treat memory as strong defaults, not proof; if something is clearly out of date, say so.

## Write

Most memory upkeep is done by the `cadri-memory-keeper` subagent, in the background, when the Cadri plugin asks you to launch it. The keeper decides *what* is worth keeping by reading the turn digest against `references/maintain.md` — not by keyword rules. Often it changes nothing. The first memories after install are written by `cadri-memory-seed` or `cadri-memory-join`, in that task, not by the keeper. Write yourself only when:

- the user explicitly asks you to remember, forget, or correct something
- you are the keeper
- you are following `cadri-memory-seed` or `cadri-memory-join`

Follow `references/maintain.md`. Edit the files in the book directly with targeted edits, then publish:

```bash
cadri memory push -m "What changed and why"            # your own edits
cadri memory push --human -m "What the user asked"     # the user told you to remember this
```

`push` commits only this session's files, rebases, and pushes. If it reports a conflict, load the `cadri-memory-repair` skill. If it says the server was not reachable, the commit is saved and Cadri pushes it after the turn. Never run `git commit`, `git push`, or `git reset` in a book yourself.

If no Cadri memory context is in your conversation (for example the session-start hook did not run), run `cadri memory context` and read its output.
