import type { MemoryCommit } from "../memories/types";

export type MemoryBook = "team" | "user";

export type LinkedCommit = MemoryCommit & { book: MemoryBook };

export type TranscriptRow = {
  session_id: string;
  cwd: string | null;
  hook_event: string | null;
  created_at: string;
  updated_at: string;
  bytes: number;
  preview: string;
  user_id?: string;
  harness?: string;
  harness_version?: string;
  model?: string;
  model_version?: string;
  compaction_summary?: string;
  /** First thing the person typed. */
  prompt?: string;
  started_at: string;
  ended_at: string;
  user_turns: number;
  assistant_turns: number;
  tool_call_count: number;
  event_count: number;
  tool_counts: Record<string, number>;
  has_writes: boolean;
  memory_commits: LinkedCommit[];
};

export type ChatMessage = {
  role: "user" | "assistant";
  /** For the assistant, the reply after the turn's last tool call. */
  text: string;
  /** Assistant narration written between tool calls. */
  notes?: string[];
  at?: string;
};

export type ToolCall = {
  name: string;
  at?: string;
};

export type TranscriptDetail = TranscriptRow & {
  messages: ChatMessage[];
  tool_calls: ToolCall[];
};

export function commitLink(commit: { id: string; book: MemoryBook }): string {
  return commit.book === "user" ? `/memories/commits/${commit.id}?scope=user` : `/memories/commits/${commit.id}`;
}

export function transcriptLink(sessionId: string): string {
  return `/transcripts/${encodeURIComponent(sessionId)}`;
}

export function shortId(id: string): string {
  if (id.length <= 8) {
    return id;
  }
  // OpenCode ids (`ses_<time><random>`) share their leading characters within a minute.
  return id.startsWith("ses_") ? id.slice(-8) : id.slice(0, 8);
}

const dateTime = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });

export function formatTime(iso: string): string {
  const time = new Date(iso);
  return Number.isNaN(time.getTime()) ? iso : dateTime.format(time);
}

export function formatDuration(startIso: string, endIso: string): string {
  const seconds = Math.max(0, Math.round((Date.parse(endIso) - Date.parse(startIso)) / 1000));
  if (!Number.isFinite(seconds)) {
    return "";
  }
  if (seconds < 60) {
    return `${seconds}s`;
  }
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) {
    return `${minutes}m ${seconds % 60}s`;
  }
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${minutes % 60}m`;
}

/** The last path segment of a working directory, e.g. `/Users/ada/site` → `site`. */
export function folderName(cwd: string | null): string {
  if (!cwd) {
    return "";
  }
  const parts = cwd.replace(/[\\/]+$/, "").split(/[\\/]/);
  return parts[parts.length - 1] ?? cwd;
}
