import { useState, type KeyboardEvent } from "react";
import { addDays, dayKey, dayStart, RANGE_LABELS, timeWindow, type Filters, type RangeKey } from "./filters";
import { harnessInfo } from "./HarnessLogo";
import type { MemoryBook, TranscriptRow } from "./types";

type Mode = "transcripts" | "memories";

type Series = { key: string; label: string; color: string };

type Day = { key: string; counts: Map<string, number>; total: number };

const WIDTH = 800;
const HEIGHT = 220;
const LEFT = 36;
const RIGHT = 8;
const TOP = 10;
const BOTTOM = 26;
const MAX_DAYS = 366;

const BOOK_SERIES: Record<MemoryBook, Series> = {
  team: { key: "team", label: "Team memories", color: "var(--pink)" },
  user: { key: "user", label: "Personal memories", color: "var(--lav)" },
};

const dayLabel = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });

function niceMax(value: number): number {
  if (value <= 4) {
    return 4;
  }
  const power = 10 ** Math.floor(Math.log10(value));
  const step = [1, 2, 2.5, 5, 10].find((candidate) => candidate * power >= value / 4) ?? 10;
  return Math.ceil(value / (step * power)) * step * power;
}

function bucket(items: Array<{ at: string; series: string }>, keys: string[]): Day[] {
  const days = new Map<string, Day>(keys.map((key) => [key, { key, counts: new Map(), total: 0 }]));
  for (const item of items) {
    const time = Date.parse(item.at);
    if (Number.isNaN(time)) {
      continue;
    }
    const day = days.get(dayKey(time));
    if (!day) {
      continue;
    }
    day.counts.set(item.series, (day.counts.get(item.series) ?? 0) + 1);
    day.total += 1;
  }
  return keys.map((key) => days.get(key)!);
}

function dayKeys(window: { start: number | null; end: number }, times: number[]): string[] {
  const last = dayKey(window.end - 1);
  const earliest = times.length > 0 ? Math.min(...times) : null;
  let first = window.start !== null ? dayKey(window.start) : earliest !== null ? dayKey(earliest) : addDays(last, -29);
  if (dayStart(first) < dayStart(addDays(last, -(MAX_DAYS - 1)))) {
    first = addDays(last, -(MAX_DAYS - 1));
  }
  if (first > last) {
    first = last;
  }
  const keys: string[] = [];
  for (let key = first; key <= last; key = addDays(key, 1)) {
    keys.push(key);
  }
  return keys;
}

function buildChart(
  mode: Mode,
  rows: TranscriptRow[],
  commits: Array<{ at: string; book: MemoryBook }>,
  window: { start: number | null; end: number },
) {
  const items =
    mode === "transcripts"
      ? rows.map((row) => ({ at: row.started_at, series: row.harness ?? "" }))
      : commits.map((commit) => ({ at: commit.at, series: commit.book }));
  // Both modes share one span so toggling keeps the same days on the axis.
  const times = [...rows.map((row) => row.started_at), ...commits.map((commit) => commit.at)]
    .map((at) => Date.parse(at))
    .filter((time) => !Number.isNaN(time));
  const days = bucket(items, dayKeys(window, times));
  const seriesKeys = [...new Set(items.map((item) => item.series))].sort();
  const series: Series[] =
    mode === "transcripts"
      ? seriesKeys.map((key) => {
          const info = harnessInfo(key || undefined);
          return { key, label: info.label, color: info.color };
        })
      : (["team", "user"] as MemoryBook[]).filter((book) => seriesKeys.includes(book)).map((book) => BOOK_SERIES[book]);
  const total = days.reduce((sum, day) => sum + day.total, 0);
  return { days, series, total, max: niceMax(Math.max(0, ...days.map((day) => day.total))) };
}

function CalendarIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <rect x="3" y="5" width="18" height="16" rx="2" />
      <path d="M3 10h18M8 3v4M16 3v4" />
    </svg>
  );
}

const QUICK_RANGES: RangeKey[] = ["1d", "3d", "7d", "30d", "all"];

function RangePicker({
  filters,
  now,
  onChange,
}: {
  filters: Filters;
  now: number;
  onChange: (changes: Partial<Filters>) => void;
}) {
  const today = dayKey(now);
  return (
    <div className="activity-range">
      <div className="memory-switch range-switch" role="group" aria-label="Time range">
        {QUICK_RANGES.map((key) => (
          <button key={key} type="button" aria-pressed={filters.range === key} onClick={() => onChange({ range: key })}>
            {RANGE_LABELS[key]}
          </button>
        ))}
        <button
          type="button"
          className="range-dates"
          aria-pressed={filters.range === "custom"}
          onClick={() => onChange({ range: "custom", from: filters.from ?? addDays(today, -6), to: filters.to ?? today })}
        >
          <CalendarIcon />
          {RANGE_LABELS.custom}
        </button>
      </div>
      {filters.range === "custom" ? (
        <div className="range-inputs">
          <label>
            <span className="filter-label">From</span>
            <input
              type="date"
              value={filters.from ?? ""}
              max={filters.to ?? today}
              onChange={(event) => onChange({ from: event.target.value || null })}
            />
          </label>
          <label>
            <span className="filter-label">To</span>
            <input
              type="date"
              value={filters.to ?? ""}
              min={filters.from ?? undefined}
              max={today}
              onChange={(event) => onChange({ to: event.target.value || null })}
            />
          </label>
        </div>
      ) : null}
    </div>
  );
}

export function ActivityChart({
  rows,
  commits,
  filters,
  now,
  onChange,
  onPickDay,
}: {
  rows: TranscriptRow[];
  commits: Array<{ at: string; book: MemoryBook }>;
  filters: Filters;
  now: number;
  onChange: (changes: Partial<Filters>) => void;
  onPickDay: (key: string) => void;
}) {
  const window = timeWindow(filters, now);
  const [mode, setMode] = useState<Mode>("transcripts");
  const chart = buildChart(mode, rows, commits, window);

  const plotWidth = WIDTH - LEFT - RIGHT;
  const plotHeight = HEIGHT - TOP - BOTTOM;
  const slot = plotWidth / Math.max(1, chart.days.length);
  const barWidth = Math.max(1, Math.min(28, slot * 0.62));
  const labelEvery = Math.max(1, Math.ceil(chart.days.length / 8));
  const noun = mode === "transcripts" ? "transcript" : "memory";
  const plural = mode === "transcripts" ? "transcripts" : "memories";

  function describe(day: Day): string {
    const label = dayLabel.format(new Date(dayStart(day.key)));
    const parts = chart.series
      .filter((series) => day.counts.get(series.key))
      .map((series) => `${series.label} ${day.counts.get(series.key)}`);
    const count = `${day.total} ${day.total === 1 ? noun : plural}`;
    return parts.length > 1 ? `${label}: ${count} (${parts.join(", ")})` : `${label}: ${count}`;
  }

  function onKey(event: KeyboardEvent<SVGGElement>, key: string): void {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      onPickDay(key);
    }
  }

  return (
    <section className="activity-chart" aria-label="Activity per day">
      <div className="activity-head">
        <div>
          <h2>Activity</h2>
          <p className="activity-sub">
            {mode === "transcripts"
              ? `${chart.total} ${chart.total === 1 ? noun : plural} per day, by agent harness`
              : `${chart.total} memory ${chart.total === 1 ? "commit" : "commits"} per day, team and personal books`}
          </p>
        </div>
        <div className="memory-switch" role="group" aria-label="Chart">
          <button type="button" aria-pressed={mode === "transcripts"} onClick={() => setMode("transcripts")}>
            Transcripts
          </button>
          <button type="button" aria-pressed={mode === "memories"} onClick={() => setMode("memories")}>
            Memories
          </button>
        </div>
      </div>
      <RangePicker filters={filters} now={now} onChange={onChange} />
      <svg className="activity-svg" viewBox={`0 0 ${WIDTH} ${HEIGHT}`} role="img" aria-label={`${plural} per day`}>
        {[0, 0.25, 0.5, 0.75, 1].map((fraction) => {
          const y = TOP + plotHeight * (1 - fraction);
          return (
            <g key={fraction}>
              <line className="activity-grid" x1={LEFT} x2={WIDTH - RIGHT} y1={y} y2={y} />
              <text className="activity-axis" x={LEFT - 6} y={y + 4} textAnchor="end">
                {Math.round(chart.max * fraction)}
              </text>
            </g>
          );
        })}
        {chart.days.map((day, index) => {
          const x = LEFT + slot * index + (slot - barWidth) / 2;
          let y = TOP + plotHeight;
          return (
            <g
              key={day.key}
              className="activity-day"
              role="button"
              tabIndex={0}
              aria-label={describe(day)}
              onClick={() => onPickDay(day.key)}
              onKeyDown={(event) => onKey(event, day.key)}
            >
              <title>{describe(day)}</title>
              <rect className="activity-hit" x={LEFT + slot * index} y={TOP} width={slot} height={plotHeight} />
              {day.total === 0 ? (
                <rect className="activity-empty" x={x} y={TOP + plotHeight - 2} width={barWidth} height={2} />
              ) : (
                chart.series.map((series) => {
                  const count = day.counts.get(series.key) ?? 0;
                  if (count === 0) {
                    return null;
                  }
                  const height = (count / chart.max) * plotHeight;
                  y -= height;
                  return <rect key={series.key} x={x} y={y} width={barWidth} height={height} fill={series.color} />;
                })
              )}
              {index % labelEvery === 0 ? (
                <text className="activity-axis" x={LEFT + slot * index + slot / 2} y={HEIGHT - 8} textAnchor="middle">
                  {dayLabel.format(new Date(dayStart(day.key)))}
                </text>
              ) : null}
            </g>
          );
        })}
      </svg>
      {chart.series.length > 0 ? (
        <ul className="activity-legend">
          {chart.series.map((series) => (
            <li key={series.key}>
              <span className="activity-swatch" style={{ background: series.color }} />
              {series.label}
            </li>
          ))}
        </ul>
      ) : null}
    </section>
  );
}
