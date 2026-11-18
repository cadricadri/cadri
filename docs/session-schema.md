# Cadri session + memory commit schema (v1)

Locked for agent-side initialization. Client never sends `user_id` / `org_id`.

## Sessions

The client still posts the harness transcript. The server parses it on ingest and stores the chat, plan cards, and tool names. The raw transcript is not kept. It remains on the machine that uploaded it, so a later backfill can read it again.

### Client POST (`POST /api/sessions`)

| Field | Required | Notes |
|---|---|---|
| `thread_id` | yes* | Harness session id. Alias: `session_id` (accepted for back-compat). |
| `timestamp` | no | ISO-8601; server uses clock if omitted. |
| `conversation` | no | `[{ "role": "user\|assistant\|system", "text": "..." }, ...]` |
| `transcript_text` | no | Flattened conversation; server may derive from `conversation`. |
| `harness` | no | e.g. `claude_code`, `cursor`, `codex`, `opencode` |
| `harness_version` | no | |
| `model` | no | |
| `model_version` | no | |
| `hook_event` | no | `backfill`, `SessionEnd`, … |
| `content` / `trajectory` | no* | Layer A blob. Prefer `trajectory`; `content` kept for back-compat (Claude JSONL). |
| `compaction_summary` | no | Harness summary if compaction ran. |

\*At least one of `content`, `trajectory`, `conversation`, or `transcript_text` must be non-empty.

### Server-owned

`org_id`, `user_id` (token `sub`), `created_at`, `updated_at`, `bytes`, `preview`.

### List vs detail

- **`GET /api/sessions` (list)** returns the summary: `preview`, `prompt`, harness/model fields, `bytes`, timestamps, turn counts. It does not return the chat.
- **`POST /api/sessions`** parses the uploaded transcript and returns the summary row. It does not echo the raw transcript.
- On every upload the server parses the transcript (`engine/core/src/transcript/`) and stores that view plus `prompt`, `started_at`, `ended_at`, `user_turns`, `assistant_turns`, `tool_call_count`, `tool_counts`, `has_writes`. `bytes` is the size of the stored view. List rows carry the summary plus `event_count` and `memory_commits` (commits whose `Cadri-Session-Id` names the session).
- **`GET /api/sessions/{session_id}`** returns the row with `messages` (`[{role: user|assistant, text, notes?, at?}]`, tool calls hidden; `notes` is assistant text written before a tool call in the same turn, `text` the reply after the last one), `tool_calls` (`[{name, at?}]`), `plans` (`[{title, overview?, body, source, after_message}]`, where `after_message` is the chat message the plan follows), and `memory_commits`. A plan is a markdown design extracted from the session blob (Cursor `CreatePlan`, a write under `~/.cursor/plans`, `~/.claude/plans`, or `~/.opencode/plan`, a `*.plan.md` file, Codex `apply_patch` of a plan file, or an OpenCode plan tool). It does not return raw `content`.

### Visibility

Org-scoped (shared with team), same as today.

## Memory commits

Memory text lives in the git book. Provenance:

- **Id:** `commit_hash` + file path (no separate `memory_uuid` table in v1).
- Client may include in commit message / API metadata: `thread_id`, `other_memories_used`, harness/model, `compaction_prompt_version`.
- Server sets author from token + `Cadri-Author-Kind`.
- `POST /api/memories/commits` accepts `session_id`; the server writes it as a `Cadri-Session-Id:` trailer. Commits without the trailer but with a `Sources: <thread_id>, …` or `Sources: <harness>:<thread_id>, …` line are linked by the first id listed.
- **`GET /api/memories/commits/{id}?scope=team|personal`** returns the commit (`session_id` included) and `changes: [{path, before, after}]` with full file text (`null` when absent). `scope=user` is the previous name of the personal book and still works.
