# Searching Cadri memory

<!-- reminder -->
Before you start, open the candidate memory files above that fit this task (batch the reads with your first code searches). Follow links and directory `MEMORY.md` indexes from there, check the tree from session start if a topic is missing, and stop once you know enough. Don't read memory speculatively.
<!-- /reminder -->

Memory is organized for progressive disclosure: each level tells you enough to decide whether to go deeper.

1. **Start from what is already in context.** Both `MEMORY.md` files and the book tree (paths with descriptions) were loaded at session start. On each prompt, Cadri adds up to five candidate files that share words with the request. Candidates are keyword matches: judge them by their descriptions before opening.
2. **Open the files that fit the task.** Read them in one batch, together with your first code searches. A file's links name related files; a directory's `MEMORY.md` says what is inside and when each file is worth opening.
3. **Go one level deeper only when it pays.** Follow a link or directory index when the task touches that subject and what you have does not answer it. Stop once you know what you need.
4. **Search when the tree does not cover it.** Use a text search across both books for a distinctive term:

   ```bash
   grep -ril "term" "$(cadri memory path)" "$(cadri memory path --scope user)"
   ```

   For a broad investigation, hand the search to an exploration subagent so the reads stay out of your context.
5. **Use what you found.** Apply preferences and decisions as defaults. When memory conflicts with what you see in the code or the user says, trust the current evidence, say so, and let the keeper fix memory afterwards.

Don't load memory for tasks it cannot inform (a one-line edit, a question fully answered by the open file), and don't re-read files already in your context.
