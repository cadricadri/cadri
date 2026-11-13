---
name: cadri-initializing-memory
description: Reorganize an existing Cadri memory book. Use when the user asks to restructure memory or run a deep memory init. A fresh Cadri install uses cadri-memory-seed instead.
---

# Memory Initialization

Your memory is two git clones of Markdown: the team book at `$(cadri memory path)` and your personal book at `$(cadri memory path --scope user)`. Restructure them with ordinary file tools, then publish with `cadri memory push`.
**You own this task through completion**: research, ask, synthesize, write, commit, verify. Delegate research, but integrate the results yourself — launching a background task is not completion. If a `cadri-memory-keeper` you launched is still editing, wait for it, then re-read before editing.

## Principles

**`MEMORY.md` is always loaded.** Cadri puts each book's root `MEMORY.md` into every session at start, followed by a tree of the book with each file's description. Spend it on what shapes ordinary turns: identity, preferences, behavioral rules, orientation, routes to everything else. Transient items (a ticket, a commit hash, session notes) dilute it.

**Progressive disclosure.** Nested Markdown is deferred until something reads it. Each directory's `MEMORY.md` describes its *immediate* children and when to read them, so you never load a whole topic to answer one question.

**Don't duplicate context you can point to.** `AGENTS.md`, `CLAUDE.md`, `README`, and repo skills belong to the environment; any agent there reads them first-hand, and your copy goes stale first. Link the owner and keep only your delta: which rules you keep breaking, what they get wrong or omit. The same fact in two core files is the same tax twice. This is not licence to compress away what only you hold — stable preferences, chronic corrections, and real gotchas earn their space.

**Identity and continuity.** Build a self that survives a model swap: what you value, your perspective, the quotes and traits that make you recognizably you. Past sessions are your experience — but other coding agents' `user` turns are not necessarily your human collaborator speaking.

**Generalize, don't memorize, and be specific.** Store the pattern, not the episode, and give every preference or gotcha a concrete command, path, or the failure it prevents. "**Always use `uv` for Python** — chronic failure, never bare `python` or `pip`" is memory; "Prefers terse responses" and "on March 3rd we debugged a crash" are not.

## Harness Constraints

Validation enforces these; the server also rejects a file over 20,000 characters, root Markdown over 65,536 characters combined, or a path more than two directories deep. Team pushes reject a new top-level directory outside the fixed set below.

- Root `MEMORY.md` must exist. Neither it nor any directory `MEMORY.md` may have YAML frontmatter.
- **Every directory on the path to a memory file needs its own frontmatter-free `MEMORY.md`.** `components/api-gateway/gotchas.md` requires both `components/MEMORY.md` and `components/api-gateway/MEMORY.md`. A directory without one is not memory. Do not create new `README.md` indexes.
- Every other memory file must have exactly `name` and `description` frontmatter — those two keys, no others. The `description` states **purpose and category**, not contents: you read it to decide whether to load the file.
- No file and directory sharing a stem (`human.md` beside `human/`). Skills live at `skills/{skill_name}/SKILL.md` and stay out of memory indexes.
- **Team book top-level directories are fixed:** `architecture/`, `components/`, `features/`, `incidents/`, `operations/`, `path-to-prod/`. Subfolders under those are free-form.

Create a missing top-level directory when the first lesson needs it, in the same push as that lesson. Put a lesson in the directory a future search would look in. Do not paste it into the root `MEMORY.md`.

**Budget**: keep `MEMORY.md` short — every session pays for it (Cadri trims it past about 6,000 characters). When it crowds that, move detail into an indexed child directory and leave a link — don't delete it.

## Structure

Derive structure from what you found. Put material in the core tier by how often you need it, not by how much of it there is. Use the project's real name (`orchard/overview.md`, not `project/overview.md`). Split when a topic needs separate retrieval; combine when splitting leaves two files of three lines each.

Root `MEMORY.md` is a **discovery guide** teaching agents how to find information, not an exhaustive listing:

```markdown
# Team Memory

## How to Find Information

**Start with grep when you have a keyword:**
```bash
grep -ri "concept" "$(cadri memory path)"
```

**Component-specific knowledge:** When working on a service, check `components/[service-name]/`
- `implementation.md` - How it works
- `gotchas.md` - Known footguns and failure modes

**Understanding past failures:** Check `incidents/[pattern-name]/` for analysis
- Incidents are grouped by pattern (e.g., startup-death-spirals, database-deadlocks)
- Each contains full analysis with what happened, root cause, and lesson

**Operational knowledge:** Check `operations/`
- `runbooks.md` - Common procedures
- `alerts.md` - What alerts mean and how to respond

**Architectural decisions:** Check `architecture/` for rationale
**Features and why they exist:** Check `features/`
**How code ships:** Check `path-to-prod/`

## Directory Guide

- **architecture/** - Design decisions, technical rationale, tradeoffs
- **components/** - Per-service knowledge, gotchas, implementation notes
- **features/** - Incremental features and their raison d'être
- **incidents/** - Grouped by failure pattern, with lessons
- **operations/** - Runbooks, alert guides, debugging procedures
- **path-to-prod/** - CI/CD and deployment process
```

**Focus on discovery mechanisms** (grep, component paths) not exhaustive content listing. An index pointing at nothing is worse than the content it displaced.

### Example Structures

Illustrations, **not templates to fill in**.

**Minimal** — a new agent, a small project, little or no approved history:

```
MEMORY.md     # Discovery guide + navigation
```

**Team memory (expanded)** — fixed top-level areas; free-form underneath:

```
team/
├── MEMORY.md
├── architecture/
│   ├── MEMORY.md
│   ├── component-map.md
│   └── design-decisions.md
├── components/
│   ├── MEMORY.md
│   ├── auth-service/
│   │   ├── MEMORY.md
│   │   ├── how-it-works.md
│   │   └── gotchas.md
│   └── api-gateway/
│       └── ...
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

**Key points:**
- **Fixed top-level categories** — do not invent a seventh root folder
- **Component-based organization** enables discovery when working on specific services
- **Incidents grouped by pattern** (startup-death-spirals, not individual incidents at top level)
- **Root MEMORY.md focuses on discovery guidance** not exhaustive content listing

## Initialization Flow

### 1. Inspect existing memory
Read what exists before changing anything. A fresh agent has defaults to replace; an existing one is a reorganization, and some files may be shared with other agents.

### 2. Detect historical session data
Check which recent sessions Cadri can see:
```bash
cadri discover --days 30
```

### 3. Identify the user from git
Infer rather than ask: `git shortlog -sn --all | head -5`, `git log --format="%an <%ae>" | sort -u | head -10`, cross-referenced with `git config user.email`.

### 4. Ask upfront questions
Ask one bundle of questions: research depth (standard or deep); other repositories you should know about; communication style; and — only if Step 2 found sessions — whether to analyze them. Don't ask what you can discover from files, git, or history. Wait for the user's reply.

### 5. Analyze approved history
Only if the user approved in Step 4. Skip entirely otherwise; Step 6 still runs. These sessions are evidence of what happened, not proof of who wrote each prompt.

Use the sessions it lists (`cadri show <thread_id>` prints one) and analyze them for patterns, preferences, chronic mistakes, and architectural decisions. Check authorship carefully — `user` turns in worker sessions may be agent-written dispatch prompts, not human preferences.

### 6. Research the codebase first-hand
Read the README, agent docs (`AGENTS.md`, `CLAUDE.md`, nested ones), the package manifest, entry points, and recent git history yourself. By the end you should be able to trace a key feature from entry point to implementation; if you can't, you haven't read enough.

**Write down what those docs already own** — conventions, layer rules, file placement, commands, gotchas. That is your no-copy list for Step 8 and your gap list for Step 7. Then split the repository into subsystem areas the docs do *not* explain, plus any related repos named in Step 4. If the docs cover the codebase well, fan out narrowly or not at all. In deep mode go further: more areas, git history for conventions, end-to-end tracing, architecture notes in deferred memory.

### 7. Synthesize findings
Combine evidence from history analysis and codebase research. Look for:
- **Identity and personality**: values, perspective, communication style
- **Hard rules and preferences** with quotes and evidence
- **Conventions and gotchas** with concrete examples
- **Project context** that's not in the README

**Check authorship before inferring preferences.** Agent-authored dispatch prompts describe how an *agent* was instructed to work, not human preferences. Corroborate from sessions you know the human drove, or store as observed patterns with uncertainty stated. Repetition does not establish authorship: a template reused across fifty sessions repeats fifty times.

### 8. Write canonical memory
**Weigh validation.** Store confirmed claims as fact; for stale ones store the current fact, keeping the history only when the change is itself a useful gotcha. Unverifiable claims need your own check before entering always-loaded memory.

**Combine, then deduplicate.** Keep each fact exactly once, and push what you don't need every turn into deferred memory with a discovery link from the core tier.

**Promote into canonical memory.** Write the survivors into the files their topics belong in, with supporting evidence deferred. Cover all three of identity and personality, hard rules and preferences with the quotes behind them, and project context; skip generic repo facts unless they change how you execute. If the output reads generically, the analysis failed for that area — re-read those transcripts or that code yourself. Keep stable `sessionId`s beside significant findings.

**Consider skills.** If the history surfaces genuinely repeatable multi-step procedures, create them now or note the candidates in memory. Don't force it.

### 9. Verify
- **Structure**: check root `MEMORY.md`, per-directory `MEMORY.md` indexes, no frontmatter on either, and exactly `name`+`description` elsewhere. Avoid `foo.md` beside `foo/`:
  `find "$(cadri memory path)" "$(cadri memory path --scope user)" -name '*.md' | sed 's/\.md$//' | while read f; do [ -d "$f" ] && echo "VIOLATION: $f"; done`
- **Root earns its place**: does root `MEMORY.md` mostly point at things a session would not find on its own? If nearly everything sits in root with one thin page behind it, move the detail down and keep the links.
- **No duplicated documentation**: grep your memory for rules `AGENTS.md`, `CLAUDE.md`, the README, or a repo skill already owns — especially a repo convention that landed in a file about the *human*.
- **Granularity and naming**: one focused topic per file, named for what is in it using the project's real name; path and description say when to read it.
- **No drift, no over-pruning**: confirm you changed structure and not the meaning of preferences or behavioral instructions, and restore any specific paths, chronic failures, or gotchas lost in curation.

### 10. Push, then report
**Unpushed memory does not reach your team or your next session.** If the push fails, report initialization as incomplete rather than describing working-tree files as live memory.

```bash
cadri memory push --human -m "init: <summary of what was initialized and key decisions>"
cadri memory status   # both books clean, nothing ahead
```

`cadri memory push` commits only the files you edited, rebases onto the latest book, and pushes. Exit status 3 means a conflict: follow the directions it prints (the `cadri-memory-repair` skill covers them), then run `cadri memory push --continue`.

Only once the push succeeds, tell the user what you built and whether coverage was complete, then ask whether they want refinement — which means another push, so repeat this step.

## Critical
- **Use parallel tool calls wherever possible** — read many files in one turn, write many memory files in one turn.
- **Write findings to memory as you go**; don't hold everything until the end.
