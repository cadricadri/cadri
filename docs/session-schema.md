# Cadri session + memory commit schema (v1)

Locked for agent-side initialization. Client never sends `user_id` / `org_id`.

## Sessions

Two layers:

- **A (trajectory):** full session blob (native JSONL or trajectory-v1) for memory gen / drill-in.
- **B (conversation):** user-visible turns for list UI.

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
| `cwd` | no | |
| `hook_event` | no | `backfill`, `SessionEnd`, … |
| `content` / `trajectory` | no* | Layer A blob. Prefer `trajectory`; `content` kept for back-compat (Claude JSONL). |
| `compaction_summary` | no | Harness summary if compaction ran. |

\*At least one of `content`, `trajectory`, `conversation`, or `transcript_text` must be non-empty.

### Server-owned

`org_id`, `user_id` (token `sub`), `created_at`, `updated_at`, `bytes`, `preview`.

### List vs detail

- **`GET /api/sessions` (list)** returns Layer B + metadata: `preview`, `transcript_text`, `conversation`, harness/model fields, `bytes`, timestamps. **Omits** Layer A `content` / trajectory.
- **`POST /api/sessions`** response still includes stored Layer A `content` so the uploader can confirm what was saved.
- The server parses Layer A on every upload (`engine/core/src/transcript/`) and stores a summary: `prompt`, `started_at`, `ended_at`, `user_turns`, `assistant_turns`, `tool_call_count`, `tool_counts`, `has_writes`. List rows carry it plus `event_count` and `memory_commits` (commits whose `Cadri-Session-Id` names the session).
- **`GET /api/sessions/{session_id}`** returns the row with `messages` (`[{role: user|assistant, text, notes?, at?}]`, tool calls hidden; `notes` is assistant text written before a tool call in the same turn, `text` the reply after the last one), `tool_calls` (`[{name, at?}]`) and `memory_commits`. It does not return raw `content`.

### Visibility

Org-scoped (shared with team), same as today.

## Memory commits

Memory text lives in the git book. Provenance:

- **Id:** `commit_hash` + file path (no separate `memory_uuid` table in v1).
- Client may include in commit message / API metadata: `thread_id`, `other_memories_used`, harness/model, `compaction_prompt_version`.
- Server sets author from token + `Cadri-Author-Kind`.
- `POST /api/memories/commits` accepts `session_id`; the server writes it as a `Cadri-Session-Id:` trailer. Commits without the trailer but with a `Sources: <thread_id>, …` or `Sources: <harness>:<thread_id>, …` line are linked by the first id listed.
- **`GET /api/memories/commits/{id}?scope=team|user`** returns the commit (`session_id` included) and `changes: [{path, before, after}]` with full file text (`null` when absent).
