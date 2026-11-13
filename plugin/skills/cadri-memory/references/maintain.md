# Maintaining Cadri memory

Memory is a working model of the team and the person, refined over time. It is not a log. Most turns change nothing; a good edit sharpens, merges, or replaces something already there.

You decide subjectively from the digest (what the user asked, how the agent replied, and the user's next message). There is no keyword checklist that forces a write. When nothing durable is present, push with a message that says so and stop.

## What is worth keeping

- **Corrections.** The user said you were wrong, or redid your work. Find the principle the mistake violated and sharpen it.
- **Preferences.** How this person wants to work: tone, review style, tools, what to ask about first. Personal book.
- **Decisions and conventions.** Why the system is shaped the way it is, team rules, naming, how to ship. Team book.
- **Durable discoveries.** Non-obvious facts that cost real effort to find and will matter again: a gotcha in a component, the real cause of a failure pattern, why a feature exists.

Leave out what a future agent can cheaply rederive (file contents, command lists, current status), what the transcript already holds (what happened in a session), one-off details, and secrets (refer to them by name, such as `$API_KEY`). Update from a pattern or an explicit instruction, not a single ambiguous event.

## Team or personal

Shared facts and decisions go in the team book; one person's taste goes in the personal book. When unsure, ask whether a teammate's agent should act on it.

## Where it goes (team book)

Top-level directories are fixed. Do not invent a new one at the root; put new material under one of these, in any subfolder shape that fits (within the depth limit):

| Directory | Use for |
|---|---|
| `architecture/` | Design decisions, component map, hard-to-recover why |
| `components/` | One directory per service or module |
| `features/` | Incremental features and their raison d'être |
| `incidents/` | Failure patterns and the lesson |
| `operations/` | Live debugging, alerts, runbooks |
| `path-to-prod/` | CI/CD and how code reaches production |

Example shape (create files only when you have something to say):

```text
team/
├── MEMORY.md                 # always loaded into every session
├── architecture/
│   ├── MEMORY.md
│   ├── component-map.md
│   └── design-decisions.md
├── components/
│   ├── MEMORY.md
│   └── auth-service/
│       ├── MEMORY.md
│       ├── how-it-works.md
│       └── gotchas.md
├── features/
│   ├── MEMORY.md
│   └── memory-books/
│       ├── MEMORY.md
│       └── why.md
├── incidents/
│   ├── MEMORY.md
│   └── startup-death-spirals/
│       ├── MEMORY.md
│       └── 2026-05-api-gateway.md
├── operations/
│   ├── MEMORY.md
│   ├── common-alerts.md
│   └── debugging-runbooks.md
└── path-to-prod/
    ├── MEMORY.md
    ├── ci-cd.md
    └── deployment-process.md
```

Rules for files:

- `MEMORY.md` at the book root is loaded into every session. Keep it under about 3,000 characters: what every agent needs up front, plus one line per directory saying when to open it, in the words a future request would use.
- Every directory has its own `MEMORY.md` (no frontmatter): a one-paragraph summary, then links to the files that matter with when each is worth opening. Do not use `README.md` for new indexes.
- Every other file starts with exactly two frontmatter fields:

  ```markdown
  ---
  name: deploys
  description: How the API is deployed, the freeze rules, and how to roll back.
  ---
  ```

  The `description` says what the file is about; agents see it in the tree and decide from it whether to open the file.
- One home per subject. Update the existing file instead of adding a near-duplicate. Name files after their subject, never a session id.
- After adding or moving a file, check that a natural future request would reach it from the root `MEMORY.md` and the directory indexes. Add the link if not.

The server rejects pushes that break these limits: files at most two directories deep (`components/api/gotchas.md` is the deepest allowed), each file at most 20,000 characters, root `*.md` files together under 65,536 characters. Team pushes also reject a new top-level directory outside the table above. When something does not fit, split it or move detail deeper; do not try to raise a limit.

## How to write it

- Write principles with the reason that makes them generalize, in the imperative: "Verify claims against the running system, because the team trusts evidence over plausibility." Not three narrow rules.
- When several lines share a cause, replace them with the one principle that explains them and delete what it absorbs.
- When a principle keeps being misapplied, attach one short contrastive example (bad, then good) instead of adding another rule.
- Write analysis, not logs: no dates, names, quotes, or incident stories. Fold new evidence into the existing text. Git keeps the history, and the commit message says why it changed.
- Check that a new line does not contradict an existing one; resolve the conflict instead of keeping both.
- Plain language, no all-caps or absolute emphasis.

## Editing safely

Other agents may be editing the same books right now.

- Re-read a file immediately before editing it.
- Make targeted edits to the lines that change. Never rewrite a whole file to change a few lines.
- Publish with `cadri memory push -m "<what changed and why>"` (add `--human` when the user explicitly asked for it). Push commits only your session's files.
- If push reports a conflict, follow the `cadri-memory-repair` skill. If it says the server was not reachable, the commit is saved and pushed after the turn.
