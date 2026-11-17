# Orienting from the repository

`cadri-memory-seed` and `cadri-memory-join` follow this before they write the team portrait. You do the reading. Subagents return notes. You write the files.

The portrait is a standalone briefing in the team book. An agent that only has that book should be able to say what the system is, how the main parts connect, why the main features exist, and who it is for. Rewrite the repo's docs into Cadri's files. A briefing may restate the README. Pasting the README, a skill body, or a directory listing is not a briefing.

This is a snapshot. The memory keeper does not re-copy the repo on later turns. Seed and join refresh a file when the repo or a session shows it is wrong. File shape, limits, and commits are in `maintain.md`. The Repository briefing section there is why this snapshot may restate what a later turn would leave out.

Leave the user's repository as you found it. The team book is the handoff. Do not add `init.sh`, a progress log, or a feature checklist there.

## Which repository

Brief one git repository: `repos.brief` from `cadri discover --json --seed`. That is a root the seed window was actually working in. The current directory is the one to read only when it sits inside that root.

`cadri init` prints the same choice. When `repos.reason` is `ask`, the counts are close: tell the user the paths and counts in `repos.ask`, and wait. Brief the one they name. When they don't pick, write no portrait. When `repos.reason` is `none`, write no portrait. Leave every other root in `repos.seen` unread. A later run from inside one of them adds that repo.

The opening sentence of root `MEMORY.md` names the path this briefing is about. The first repo uses the file names below. A later run for a different path adds files that start with that repo's directory name (`architecture/landingpage-component-map.md`, `components/landingpage-web/how-it-works.md`, `features/landingpage-signup/why.md`, `external/landingpage-customers.md`) and leaves the first repo's files in place. When that directory name is already the prefix for a different path, use the parent directory too (`wt-m11`).

## 1. Read the docs yourself

Read these in the repository named above. This first pass stays with you.

- The README, and any doc it points at for architecture, customers, or shipping.
- `AGENTS.md`, `CLAUDE.md`, and nested copies.
- Repo skill files (`SKILL.md`, `skills/`, `.agents/skills/`, `.cursor/skills/`): enough to know what each skill owns. Leave the steps in the skill.
- Package manifests, and the entry points they name.
- `git log --oneline -30`.

Note what those files already state, including which doc owns each convention. On a join, the team book you already read is part of that list. That is how you avoid a second file for the same subject, and how `architecture/conventions.md` names the owning doc.

## 2. Split the repo and fan out

Split the repository into the subsystem areas the README, the manifests, and the top-level layout name. Group tiny packages into one area.

When those docs already name the subsystems, fan out only into areas you cannot brief from the docs. Otherwise fan out across the areas.

Run at most three subagents at a time. Each subagent reads only its area and returns a briefing: what the subsystem is, how a request or main flow crosses it, which user-visible features it implements, which customers or contracts it serves, and the gotchas that are not obvious from the names. Every claim cites a file path. Subagents must not edit either memory book, must not edit the user's repository, and must not run `cadri memory`.

## 3. Trace one feature

Before you write, trace one real user-visible feature from its entry point to the implementation yourself. If you cannot, read more. The portrait waits until that trace works.

## 4. Which files to write

Write a file when the repo supports it. Skip a file rather than invent it, and name the skip in the report. A file is at most two directories deep (`components/api/how-it-works.md` is the deepest).

- `architecture/component-map.md` — subsystems, boundaries, entry points, and how a main flow crosses them.
- `architecture/conventions.md` — layer rules, file placement, and commands from the README, `CLAUDE.md`, `AGENTS.md`, and repo skills, rewritten as rules with the reason. Name the owning doc in the file. Commands that change how an agent works belong here. A dump of every package script does not.
- `components/<area>/how-it-works.md` — one directory per major subsystem, named for that subsystem. One file per source file is too fine.
- `features/<feature>/why.md` — what the feature does and why it exists, for each user-visible feature the docs or the trace reveal.
- `external/customers.md` — who it is for and what they expect, taken from the README or docs. A vague tagline is not a customer list. Skip the file and report that.
- `external/api-definitions.md` — when a public API exists. Summarize it. An OpenAPI dump is too long.
- `path-to-prod/ci-cd.md` — when a real shipping path exists. Summarize how code is built and shipped.

`operations/` and `incidents/` come from sessions and runbooks, not from this pass.

The personal book is not part of this pass. A rule in `CLAUDE.md` or `AGENTS.md` is a team convention. It is a personal preference only when a session shows this person overriding it.

Root `MEMORY.md` stays within 3,000 characters: the repo path, a few sentences on what the product is and who it is for, then one line per directory that has files. Each topic file's `description` carries the fact that decides whether a later agent opens it. Session start shows the root in full and only those descriptions.

## What to leave out

- Secrets. Refer to them by name, such as `$API_KEY`.
- Verbatim skill steps.
- A file-by-file tree of the repo.
- A second file for a subject the team book already owns.

## When sources disagree

On a join, keep a teammate's lesson unless the repo or these sessions show it is wrong.

When a session and the docs disagree, the session wins if someone corrected the docs. The current code wins if the session describes a structure that is gone.

Sessions still supply corrections, decisions, and preferences the portrait does not already contain. Give session subagents the portrait outline so they return that delta.

## Commits

Push the portrait one learning at a time. Architecture before components and features, so the root `MEMORY.md` and the directory indexes land with the first real file. Create those indexes in that same push. A scaffold of empty indexes is not a push.

A learning that comes only from the repo omits `Sources:`. Name the files in the commit body (`README.md`, `services/gateway/main.ts`). Put paths in the body. The first `Sources:` token is stored as a session id, and a filename such as `README.md` matches that pattern, so a path there links the wrong session. When a session also justifies the learning, `Sources:` lists those sessions, as in `maintain.md`, and the body can name the files as well.
