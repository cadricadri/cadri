import type { SessionRow } from "./types";

export type RangeKey = "all" | "1d" | "3d" | "7d" | "30d" | "custom";

export type Filters = {
  range: RangeKey;
  /** `YYYY-MM-DD`, local time. Only used when `range` is `custom`. */
  from: string | null;
  to: string | null;
  harness: string | null;
  model: string | null;
  user: string | null;
};

export const RANGE_LABELS: Record<RangeKey, string> = {
  "1d": "1 day",
  "3d": "3 days",
  "7d": "7 days",
  "30d": "30 days",
  all: "All time",
  custom: "Dates",
};

/** Calendar days, today included, for each quick range. */
const RANGE_DAYS: Partial<Record<RangeKey, number>> = { "1d": 1, "3d": 3, "7d": 7, "30d": 30 };

const RANGES = Object.keys(RANGE_LABELS) as RangeKey[];
const DAY_KEY = /^\d{4}-\d{2}-\d{2}$/;
export function readFilters(params: URLSearchParams): Filters {
  const raw = params.get("range");
  const range = (raw === "24h" ? "1d" : raw) as RangeKey | null;
  const day = (key: string) => {
    const value = params.get(key);
    return value && DAY_KEY.test(value) ? value : null;
  };
  return {
    range: range && RANGES.includes(range) ? range : "all",
    from: day("from"),
    to: day("to"),
    harness: params.get("harness") || null,
    model: params.get("model") || null,
    user: params.get("user") || null,
  };
}

/** Writes `changes` into the query string; `null` or a default value removes the key. */
export function withFilters(params: URLSearchParams, changes: Partial<Filters>): URLSearchParams {
  const next = new URLSearchParams(params);
  next.delete("chip");
  for (const [key, value] of Object.entries(changes)) {
    const isDefault = value === null || value === "" || (key === "range" && value === "all");
    if (isDefault) {
      next.delete(key);
    } else {
      next.set(key, String(value));
    }
  }
  if (changes.range && changes.range !== "custom") {
    next.delete("from");
    next.delete("to");
  }
  return next;
}

export function dayKey(time: number): string {
  const date = new Date(time);
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${date.getFullYear()}-${month}-${day}`;
}

/** Local midnight at the start of `key`. */
export function dayStart(key: string): number {
  const [year, month, day] = key.split("-").map(Number);
  return new Date(year, month - 1, day).getTime();
}

export function addDays(key: string, days: number): string {
  const [year, month, day] = key.split("-").map(Number);
  return dayKey(new Date(year, month - 1, day + days).getTime());
}

/** `[start, end)` in epoch ms. `null` start means no lower bound. */
export function timeWindow(filters: Filters, now: number): { start: number | null; end: number } {
  const days = RANGE_DAYS[filters.range];
  if (days) {
    return { start: dayStart(addDays(dayKey(now), 1 - days)), end: now };
  }
  switch (filters.range) {
    case "custom": {
      const from = filters.from ?? filters.to;
      const to = filters.to ?? filters.from;
      if (!from || !to) {
        return { start: null, end: now };
      }
      const [low, high] = from <= to ? [from, to] : [to, from];
      return { start: dayStart(low), end: dayStart(addDays(high, 1)) };
    }
    default:
      return { start: null, end: now };
  }
}

export function inWindow(iso: string, window: { start: number | null; end: number }): boolean {
  const time = Date.parse(iso);
  if (Number.isNaN(time)) {
    return false;
  }
  return (window.start === null || time >= window.start) && time < window.end;
}

/** Every filter except time, so the chart can show the whole span around the window. */
export function matchesFacets(row: SessionRow, filters: Filters): boolean {
  if (filters.harness && (row.harness ?? "") !== filters.harness) {
    return false;
  }
  if (filters.model && (row.model ?? "") !== filters.model) {
    return false;
  }
  if (filters.user && (row.user_id ?? "") !== filters.user) {
    return false;
  }
  return true;
}

export function applyFilters(rows: SessionRow[], filters: Filters, now: number): SessionRow[] {
  const window = timeWindow(filters, now);
  return rows.filter((row) => matchesFacets(row, filters) && inWindow(row.started_at, window));
}

export function distinct(rows: SessionRow[], pick: (row: SessionRow) => string | undefined): string[] {
  const seen = new Set<string>();
  for (const row of rows) {
    const value = pick(row);
    if (value) {
      seen.add(value);
    }
  }
  return [...seen].sort((left, right) => left.localeCompare(right));
}
