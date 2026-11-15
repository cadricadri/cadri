# Searching Cadri memory

<!-- reminder -->
Before you start, if this prompt says Cadri remembered something, call the Cadri search tool it names before any other tool, so the user sees that sentence (batch the reads with your first code searches). Then search the memory books the way you search a codebase. The search result lists lines that contain words from this prompt. A file you need can be absent, and no match means try another word. When you do not know the words yet, read that directory's `MEMORY.md` from the session-start tree. When the change must fit the existing design, read `architecture/MEMORY.md` and the `features/` index for the area you are changing. Then run grep and read the matching lines. Read a whole file when it is an index, or when the hits show the argument is the whole file. Stop once you know enough. Don't read memory speculatively.
<!-- /reminder -->

The books are ordinary markdown on disk. Look through them the way you look through a repository: a short index to learn the vocabulary, then grep for a word you can name, then the lines around a hit.

On a prompt whose words appear in a book, Cadri prints matching lines, rare words first. It reads the files themselves. It does not rank files, and it does not decide which file holds the answer. An empty result means those words are not in the files.

1. **Read an index when you do not know the words.** Session start loaded both root `MEMORY.md` files and a tree of paths with descriptions. A directory's `MEMORY.md` says what is inside and when each file is worth opening. Read that index, in the same batch as your first code searches, when the request uses words the book may not. Planning a change, or any change that must fit the existing design: read `architecture/MEMORY.md` and the `features/` index for the area you are changing. Prior plans and sequence diagrams are filed there, and the new feature's name usually does not appear in them.

2. **Search for a word you can name, and read the lines.** Run grep across both books:

   ```bash
   grep -n -C 2 -r --include='*.md' --exclude-dir=.git -e 'term' -- "$(cadri memory path)" "$(cadri memory path --scope user)"
   ```

   Pick a distinctive word: a component, a flow, `sequence`. When the lines answer the question, stop. When a hit is the right passage and you need the surrounding argument, read that range. Read a whole file when it is a `MEMORY.md` index, or when the hits show the argument is the whole file.

3. **Revise the search.** A miss, or a crowd of weak hits, means the word was off. Take a better word from the index or from a line you already found, and search again. Hand a broad sweep to an exploration subagent so the hits stay out of your context.

4. **Use what you found.** Apply preferences and decisions as defaults. When memory conflicts with the code or with what the user says, trust the current evidence, say so, and let the keeper fix memory afterwards.

Don't load memory for tasks it cannot inform (a one-line edit, a question fully answered by the open file), and don't re-read files already in your context.
