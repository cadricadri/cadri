# Maintaining Cadri memory

Memory is a working model of the team and the person, refined over time. It is not a log. Most turns change nothing; a good edit sharpens, merges, or replaces something already there.

You decide subjectively from the digest (what the user asked, how the agent replied, and the user's next message). There is no keyword checklist that forces a write. When you are the keeper and nothing durable is present, push with a message that says so and stop: that push releases the digest. Seed and join do not push when they have nothing to add.

## What is worth keeping

- **Corrections.** The user said you were wrong, or redid your work. Find the principle the mistake violated and sharpen it.
- **Preferences.** How this person wants to work: tone, review style, tools, what to ask about first. Personal book. A `user` turn in a subagent session is often another agent's dispatch prompt. Keep it as this person's preference only when they drove the session or said it themselves.
- **Decisions and conventions.** Why the system is shaped the way it is, team rules, naming, how to ship. Team book.
- **Durable discoveries.** Non-obvious facts that cost real effort to find and will matter again: a gotcha in a component, the real cause of a failure pattern, why a feature exists.

Leave out what a future agent can cheaply rederive (file contents, command lists, current status), what the transcript already holds (what happened in a session), one-off details, and secrets (refer to them by name, such as `$API_KEY`). Seed and join are the exception in Repository briefing below: that team snapshot may restate the repo, because the book is what later sessions load. An accepted plan is the exception to "the transcript already holds it": the decisions in it belong in memory, because a future agent should learn the design without reopening the session. Update from a pattern or an explicit instruction, not a single ambiguous event.

## Repository briefing

Seed and join write a standalone briefing of one git repository into the team book, the root `cadri discover --seed` names, following `references/orient.md`. The opening sentence of root `MEMORY.md` names that path. A different repository gets its own files and leaves the first in place. An agent that only has the book should be able to say what the system is, how the main parts connect, why the main features exist, and who it is for. The briefing may restate the README, `CLAUDE.md`, or `AGENTS.md`, rewritten as rules and facts with the reason. It is a snapshot.

The keeper does not re-copy the repo on later turns. The portrait changes when a session, a person, or a later join shows it is wrong.

Still leave out secrets, verbatim skill steps, and a file-by-file listing. Commands that change how an agent works go in `architecture/conventions.md` as rules. A dump of every script does not. `operations/` and `incidents/` still come from sessions and runbooks. The personal book is not filled from the repo. A `CLAUDE.md` rule is a team convention unless a session shows this person overriding it.

A learning that comes only from the repository omits `Sources:`. Name the files in the body (`README.md`, `services/gateway/main.ts`). Put paths in the body. The first `Sources:` token is stored as a session id, and a filename such as `README.md` matches that pattern, so a path there links the wrong session. When a session also justifies the learning, `Sources:` lists those sessions and the body names the files.

## Plans

A digest section named **Plan** is a design the person and the agent wrote down before building. The short reply above it is usually just narration. Read the plan, then the user's next message.

- The next message accepts it (implement it, looks good, ship it): the plan's decisions are worth keeping.
- The next message revises it: keep the revision, not the draft. A later turn carries the revised plan.
- The next message rejects it, or nothing shows the plan was followed: it is a proposal. Do not store it.

Do not paste the plan into memory. Store the decisions a future agent cannot cheaply recover from the code: why this shape, what was rejected, how the pieces connect.

Where an accepted plan goes:

- A plan that establishes how components fit, or a design later work assumes: `architecture/`.
- A plan that adds or changes one feature: `features/`.
- A plan that changes how one component behaves: `components/`.
- A plan that is only a sequence of edits, with no decision beyond the code: nothing. The code is the record.

One learning per decision. A plan is not a new file of its own.

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
├── conventions.md
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

The server rejects a push that breaks these limits. Root `MEMORY.md` is at most 3,000 characters. Every other file is at most 65,536 characters. Other root markdown files together stay under 65,536 characters. A file is at most four directories deep (`components/api/http/handlers/gotchas.md` is the deepest). Team pushes also reject a new top-level directory outside the table above. When something does not fit, split it into another file under the same directory. Do not add a directory level past that depth, and do not try to raise a limit.

## How to write it

- Write principles with the reason that makes them generalize, in the imperative: "Verify claims against the running system, because the team trusts evidence over plausibility." Not three narrow rules.
- When several lines share a cause, replace them with the one principle that explains them and delete what it absorbs.
- When a principle keeps being misapplied, attach one short contrastive example (bad, then good) instead of adding another rule.
- Write the lesson, not the session: no dates, no one's name, no quotes, and no play-by-play. An incident file names the failure pattern and the rule that prevents it. Fold new evidence into the existing text. Git keeps the history. Names, dates, and sources belong in the commit, as described in How to write the commit.
- When two statements cannot both be true, keep the one that replaces the other and delete the old one. Facts that can both be true can both stay.
- Plain language, no all-caps or absolute emphasis.

## How to write the commit

Someone reading `git log` or blame should see why the line exists without opening the session. The file states the rule or fact. The commit states the learning, the story, and the sources.

Commit each learning separately. A learning may touch several files, such as a topic file and the indexes that link it. When a turn produces several learnings, finish and push one before editing for the next. `cadri memory push` puts every file this session has not yet pushed into that one commit.

Repeat `-m` once per paragraph. Add `--human` when the user explicitly asked for this change. Pass `--book team` or `--book personal` so a dirty file in the other book is not included.

```bash
cadri memory push --book team \
  -m "Deploys happen on Wednesday, before the release cut." \
  -m "The Thursday rule was leftover from an older runbook. Two sessions corrected it, and the freeze is tied to the release cut rather than a weekday." \
  -m "Sources: cursor:7a9e, claude_code:2f1c
- cursor:7a9e (2026-11-13): the deploy day was corrected to Wednesday
- claude_code:2f1c (2026-11-01): the freeze is before the release cut"
```

- The subject is the learning itself, one line, the rule or fact. `Seed memories from 12 sessions` and `Update operations/deploys.md` name the procedure, not the learning.
- The body tells the story: what the user said or corrected, what failed or succeeded, or what pattern recurred, and why it generalizes beyond that moment. Write the event date here. The git date is when the commit was pushed.
- The first `Sources:` line lists session ids as `harness:id`, separated by commas. The Sessions page links the first id. Then one line per source: harness, id, the event date, and what happened. When the user explicitly asked, cite that request. Take ids from the digest, `cadri discover`, or `cadri show`. Add a message id only when the transcript printed one. Do not invent a source. If no session id can be found, leave `Sources:` out and say in the body that the source could not be identified. A placeholder token after `Sources:` would be stored as a session id. A repository path is not a session id. A seed or join learning that comes only from the repo omits `Sources:` and names the files in the body, as in Repository briefing.
- Never include secrets.
- The keeper passes `--agent cadri-memory-keeper` and `--session`. Pass `--digest` only on its last push; that push releases the digest. When nothing was worth keeping, that only push is `-m "Nothing worth keeping"` and creates no commit.
- Seed and join pass their own `--agent` and omit `--session` unless that one learning came from a single session. `Sources:` lists the sessions that justified that learning, not the whole window. A learning from the repo alone omits `Sources:`.

## Editing safely

Other agents may be editing the same books right now.

- Re-read a file immediately before editing it.
- Make targeted edits to the lines that change. Never rewrite a whole file to change a few lines.
- Publish with the commit above. Push commits only your session's files.
- If `cadri memory push` reports a conflict and you are not already following `cadri-memory-repair`, load that skill. It applies the file rules and "How to write it" above, then continues the push. Do not follow this bullet again from inside that skill. If the push says the server was not reachable, the commit is saved and pushed after the turn.
