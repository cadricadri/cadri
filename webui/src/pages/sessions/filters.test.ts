import { describe, expect, it } from "vitest";
import type { SessionRow } from "./types";
import { applyFilters, readFilters, timeWindow, withFilters } from "./filters";

function filters(overrides: Partial<ReturnType<typeof readFilters>> = {}) {
  return {
    range: "all" as const,
    from: null,
    to: null,
    harness: null,
    model: null,
    user: null,
    ...overrides,
  };
}

function row(overrides: Partial<SessionRow> & { started_at: string }): SessionRow {
  return {
    session_id: "s",
    hook_event: null,
    created_at: overrides.started_at,
    updated_at: overrides.started_at,
    bytes: 1,
    preview: "",
    ended_at: overrides.started_at,
    user_turns: 1,
    assistant_turns: 0,
    tool_call_count: 0,
    event_count: 1,
    tool_counts: {},
    has_writes: false,
    memory_commits: [],
    ...overrides,
  };
}

const now = new Date(2026, 10, 17, 15, 0, 0).getTime();

describe("session filters", () => {
  it("treats the old 24h range as today", () => {
    const read = readFilters(new URLSearchParams("range=24h"));
    expect(read.range).toBe("1d");
    const window = timeWindow(read, now);
    expect(window.start).toBe(new Date(2026, 10, 17).getTime());
    expect(window.end).toBe(now);
  });

  it("drops a date that is not a calendar day", () => {
    const read = readFilters(new URLSearchParams("range=custom&from=yesterday&to=2026-11-02"));
    expect(read.from).toBeNull();
    expect(read.to).toBe("2026-11-02");
  });

  it("includes both ends of a custom range and ignores the other facet", () => {
    const active = filters({
      range: "custom",
      from: "2026-11-02",
      to: "2026-11-01",
      harness: "cursor",
    });
    const window = timeWindow(active, now);
    expect(window.start).toBe(new Date(2026, 10, 1).getTime());
    expect(window.end).toBe(new Date(2026, 10, 3).getTime());
    const kept = applyFilters(
      [
        row({ session_id: "in", started_at: "2026-11-01T12:00:00", harness: "cursor" }),
        row({ session_id: "late", started_at: "2026-11-03T00:00:00", harness: "cursor" }),
        row({ session_id: "other", started_at: "2026-11-01T12:00:00", harness: "claude" }),
      ],
      active,
      now,
    );
    expect(kept.map((item) => item.session_id)).toEqual(["in"]);
  });

  it("removes a cleared range and the chip leftover from the url", () => {
    const next = withFilters(new URLSearchParams("range=7d&chip=old&harness=cursor"), {
      range: "all",
      harness: null,
    });
    expect(next.toString()).toBe("");
  });
});
