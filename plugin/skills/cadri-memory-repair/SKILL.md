---
name: cadri-memory-repair
description: Resolves a Cadri memory push conflict or a memory book that will not sync. Use when cadri memory push or the Cadri plugin reports a conflict or a repair. Routine memory reads and edits stay on cadri-memory.
---

# Repairing a Cadri memory book

A memory book is a git clone shared by every agent session on this machine. Cadri resolves conflicts in a separate scratch worktree so the shared clone stays usable while you work. Never resolve conflicts in the shared clone, and never run `git push`, `git reset`, `git stash`, or `git rebase` there yourself.

**Input.** `cadri memory push` printed a worktree path and the conflicted files, or `cadri memory status` shows a repair in progress or a clone that needs re-cloning.

**Stop.** The conflict needs the user's judgment. Abort, then tell them which files conflict and what each side says.

**Output.** What you resolved, which files, and anything you left for the user.

## See what is wrong

```bash
cadri memory status
```

It lists each book's state: clean, unpushed commits, a repair in progress (with the worktree path and conflicted files), or a clone that needs re-cloning.

## Resolve a conflict

`cadri memory push` printed the worktree path and the conflicted files, for example `~/.cadri/memory/<host>/<org>/team.repair`. In that worktree, for each conflicted file:

1. Read the whole file, including both sides between `<<<<<<<`, `=======`, and `>>>>>>>`.
2. Write the merged result. Keep a fact from either side when both can be true. When one side replaces the other, keep the replacement and drop the old line. Apply the file rules and "How to write it" in `cadri-memory/references/maintain.md`. Skip that file's instruction to load this skill; you are already here. Remove every conflict marker.
3. Keep the frontmatter valid: `MEMORY.md` indexes have none; every other file has exactly `name` and `description`.

Then continue from anywhere:

```bash
cadri memory push --continue --book team    # or --book personal
```

It checks for leftover markers, finishes the rebase, pushes, and moves the shared clone to the result. If it reports more conflicts (the next commit in the rebase, or the server moved again), resolve those the same way and run it again.

## Give up for now

```bash
cadri memory push --abort --book team
```

The unpushed commits stay in the shared clone, and Cadri retries them later. Use this when the conflict needs the user's judgment.

## Other problems

- **Book missing or unreadable:** `cadri memory sync --scope team` clones it again; an unreadable clone is moved aside first, never deleted.
- **Not signed in:** ask the user to run `cadri login`, or to open the sign-in link Cadri showed.
- **Server not reachable:** nothing to repair; commits are pushed once it is.
