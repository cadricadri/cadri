---
name: cadri-memory-seed
description: Seed Cadri personal/team memory books from recently backfilled transcripts after cadri init. Use when install docs ask to backfill memories, or the user asks to learn from past agent sessions into Cadri.
---

# Cadri memory seed (stub)

After `cadri init` has uploaded transcripts, seed memory books on the client:

1. Confirm Cadri is reachable (`CADRI_URL`, default `https://app.cadri.io`).
2. List recent transcripts via the Cadri UI or `GET /api/transcripts` with the user's credentials.
3. For a small batch (start with 3–5 finished sessions), run a **background subagent** that:
   - Reads the transcript (prefer conversation / compaction summary when present).
   - Applies Letta-style priorities: mistakes → preferences → durable facts → contradictions → reusable procedures.
   - Commits to the **personal** book (`scope=user`) unless the lesson is clearly team-wide.
   - Sets `Cadri-Author-Kind: robot`.
   - Puts provenance in the commit message (`Sources: <thread_id>`), not in the memory body.
4. Cap concurrency at 2–3 subagents. Skip empty or trivial sessions.

Exact reflection prompts will be refined later; keep this skill as the orchestration stub.
