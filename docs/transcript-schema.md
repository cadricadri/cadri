# Cadri transcript + memory commit schema (v1)

Locked for agent-side initialization. Client never sends `user_id` / `org_id`.

## Transcripts

Two layers:

- **A (trajectory):** full session blob (native JSONL or trajectory-v1) for memory gen / drill-in.
- **B (conversation):** user-visible turns for list UI.

### Client POST (`POST /api/transcripts`)

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

- **List** returns Layer B + metadata (no full `trajectory`/`content` body by default; includes `preview`, `transcript_text` or derived preview).
- **Full body** available on the same list payload for v1 simplicity if small; large `trajectory` is stored but list may omit it via a future `?detail=` — for v1, list includes all fields (existing behavior) so the UI keeps working.

### Visibility

Org-scoped (shared with team), same as today.

## Memory commits

Memory text lives in the git book. Provenance:

- **Id:** `commit_hash` + file path (no separate `memory_uuid` table in v1).
- Client may include in commit message / API metadata: `thread_id`, `other_memories_used`, harness/model, `compaction_prompt_version`.
- Server sets author from token + `Cadri-Author-Kind`.
