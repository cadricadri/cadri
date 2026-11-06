---
name: cadri-memory
description: Read and write the team's and this person's Cadri memory. Use at the start of a task on any project, when an unfamiliar system or convention comes up, when the user corrects you, and when you learn something a future agent should know.
---

# Cadri memory

Cadri keeps two memory books on this machine as git clones that stay within a minute of the server:

- Team book: `cadri memory path`
- Personal book: `cadri memory path --scope user`

## Before you work

Read both `README.md` indexes, then open the files they point to for the task at hand. Search when the index doesn't cover it:

```bash
grep -ril "deploy" "$(cadri memory path)" "$(cadri memory path --scope user)"
```

Treat memories as strong defaults. If one is clearly out of date, say so and fix it.

## When you learn something

Write a memory when the user corrects you, states a preference, or you find out something non-obvious that will matter again. Personal taste goes in the personal book, shared facts and decisions in the team book. Write the rule, not the story, and never store secrets.

Update an existing file when one covers the topic. New files start with `name` and `description` frontmatter and get a line in that book's `README.md`.

```bash
cadri memory write path/in/book.md --file /tmp/memory.md -m "What changed" -m "Sources: <harness>:<session id>"
```

Add `--scope user` for the personal book. Exit status 3 means the file changed since your clone: run `cadri memory sync`, re-read it, merge, and write again. Never commit or push in the clone yourself; always write through `cadri memory write`.
