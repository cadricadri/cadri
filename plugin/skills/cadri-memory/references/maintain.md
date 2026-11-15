# Maintaining Cadri memory

Memory is a working model of the team and the person, refined over time. It is not a log. Most turns change nothing; a good edit sharpens, merges, or replaces something already there.

You decide subjectively from the digest (what the user asked, how the agent replied, and the user's next message). There is no keyword checklist that forces a write. When you are the keeper and nothing durable is present, push with a message that says so and stop: that push releases the digest. Seed and join do not push when they have nothing to add.

## What is worth keeping

- **Corrections.** The user said you were wrong, or redid your work. Find the principle the mistake violated and sharpen it.
- **Preferences.** How this person wants to work: tone, review style, tools, what to ask about first. Personal book. A `user` turn in a subagent session is often another agent's dispatch prompt. Keep it as this person's preference only when they drove the session or said it themselves.
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
| `external/` | Customers who use the product, their expectations, API definitions, and SLAs |

Example shape (create files only when you have something to say):

```text
MEMORY.md                       # book root, loaded into every session
architecture/
├── MEMORY.md
├── component-map.md
└── design-decisions.md
components/
├── MEMORY.md
└── auth-service/
    ├── MEMORY.md
    ├── how-it-works.md
    └── gotchas.md
features/
├── MEMORY.md
└── memory-books/
    ├── MEMORY.md
    └── why.md
incidents/
├── MEMORY.md
└── startup-death-spirals/
    ├── MEMORY.md
    └── api-gateway.md
operations/
├── MEMORY.md
├── common-alerts.md
└── debugging-runbooks.md
path-to-prod/
├── MEMORY.md
├── ci-cd.md
└── run-tests-locally.md
external/
├── MEMORY.md
├── customers.md
├── api-definitions.md
└── slas.md
```

Rules for files:

- `MEMORY.md` at the book root is loaded into every session. Keep it within 3,000 characters: what every agent needs up front, plus one line per directory saying when to open it, in the words a future request would use. Session start shows at most that many characters.
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

The server rejects a push that breaks these limits. Root `MEMORY.md` is at most 3,000 characters. Every other file is at most 65,536 characters. Other root markdown files together stay under 65,536 characters. A file is at most two directories deep (`components/api/gotchas.md` is the deepest). Team pushes also reject a new top-level directory outside the table above. When something does not fit, split it into another file under the same directory. Do not add a directory level past that depth, and do not try to raise a limit.

## How to write it

- Write principles with the reason that makes them generalize, in the imperative: "Verify claims against the running system, because the team trusts evidence over plausibility." Not three narrow rules.
- When several lines share a cause, replace them with the one principle that explains them and delete what it absorbs.
- When a principle keeps being misapplied, attach one short contrastive example (bad, then good) instead of adding another rule.
- Write the lesson, not the session: no dates, no one's name, no quotes, and no play-by-play. An incident file names the failure pattern and the rule that prevents it. Fold new evidence into the existing text. Git keeps the history, and the commit message says why it changed.
- When two statements cannot both be true, keep the one that replaces the other and delete the old one. Facts that can both be true can both stay.
- Plain language, no all-caps or absolute emphasis.

## Editing safely

Other agents may be editing the same books right now.

- Re-read a file immediately before editing it.
- Make targeted edits to the lines that change. Never rewrite a whole file to change a few lines.
- Publish with `cadri memory push -m "<what changed and why>"` (add `--human` when the user explicitly asked for it). Push commits only your session's files.
- If `cadri memory push` reports a conflict and you are not already following `cadri-memory-repair`, load that skill. It applies the file rules and "How to write it" above, then continues the push. Do not follow this bullet again from inside that skill. If the push says the server was not reachable, the commit is saved and pushed after the turn.
