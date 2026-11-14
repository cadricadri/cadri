import { useCallback, useEffect, useMemo, useRef, useState, type MouseEvent } from "react";
import { Link, Navigate, useNavigate, useSearchParams } from "react-router";
import { sessionUserLabel } from "../../funNames";
import { Page } from "../../page";
import { useSession } from "../../session";
import { loadMemoryLog } from "../memories/books";
import { ActivityChart } from "./ActivityChart";
import { applyFilters, distinct, matchesFacets, readFilters, withFilters, type Filters } from "./filters";
import { SessionFilters } from "./FilterBar";
import { HarnessBadge } from "./HarnessLogo";
import {
  commitLink,
  formatTime,
  sessionLink,
  shortId,
  type LinkedCommit,
  type MemoryBook,
  type SessionRow,
} from "./types";

/** Matches the memories page: a full seed can run well past a few minutes. */
const SEEDING_HINT_MS = 2 * 60 * 60 * 1000;

export default function SessionList() {
  const session = useSession();
  const navigate = useNavigate();
  const [params, setParams] = useSearchParams();
  const legacySession = params.get("session");
  const [status, setStatus] = useState("Loading…");
  const [phase, setPhase] = useState<"loading" | "done">("loading");
  const [rows, setRows] = useState<SessionRow[]>([]);
  const [commits, setCommits] = useState<Array<{ at: string; book: MemoryBook }>>([]);
  const [commitsReady, setCommitsReady] = useState(false);
  const request = useRef(0);
  const seeding = rows.length === 0 && phase === "done" && status.startsWith("Seeding");
  const filters = readFilters(params);
  const [now, setNow] = useState(() => Date.now());
  const oldestUpload = rows.reduce((oldest, row) => Math.min(oldest, Date.parse(row.created_at)), Number.POSITIVE_INFINITY);
  const memoryPending =
    phase === "done" &&
    rows.length > 0 &&
    commitsReady &&
    commits.length === 0 &&
    Number.isFinite(oldestUpload) &&
    now - oldestUpload < SEEDING_HINT_MS;
  const [sort, setSort] = useState<Sort>({ key: "end", dir: "desc" });

  const load = useCallback(async () => {
    const id = ++request.current;
    setPhase("loading");
    setNow(Date.now());
    try {
      const response = await fetch("/api/sessions", { headers: session.headers() });
      if (id !== request.current) {
        return;
      }
      if (response.status === 401) {
        session.unauthorized();
        return;
      }
      if (response.status === 402) {
        setRows([]);
        setStatus("This team's trial has ended.");
        setPhase("done");
        return;
      }
      if (!response.ok) {
        throw new Error(`${response.status} ${response.statusText}`);
      }
      const sessions = (await response.json()) as SessionRow[];
      setRows(sessions);
      setStatus(sessions.length === 0 ? "Seeding sessions from your agents…" : "");
      setPhase("done");
    } catch (error) {
      if (id !== request.current) {
        return;
      }
      setRows([]);
      setStatus(error instanceof Error ? `Could not load sessions: ${error.message}` : "Could not load sessions.");
      setPhase("done");
    }
  }, [session]);

  const loadCommits = useCallback(async () => {
    setCommitsReady(false);
    const books: MemoryBook[] = ["team", "user"];
    const logs = await Promise.all(
      books.map((book) =>
        loadMemoryLog(book, session.headers())
          .then((log) => log.map((commit) => ({ at: commit.at, book })))
          .catch(() => []),
      ),
    );
    setCommits(logs.flat());
    setCommitsReady(true);
  }, [session]);

  useEffect(() => {
    void load();
    void loadCommits();
  }, [load, loadCommits]);

  useEffect(() => {
    if (!params.has("chip")) {
      return;
    }
    const next = new URLSearchParams(params);
    next.delete("chip");
    setParams(next, { replace: true });
  }, [params, setParams]);

  useEffect(() => {
    if (!seeding) {
      return;
    }
    const id = window.setInterval(() => void load(), 3000);
    return () => window.clearInterval(id);
  }, [seeding, load]);

  const shown = applyFilters(rows, filters, now);
  const ordered = useMemo(() => sortRows(shown, sort), [shown, sort]);
  const chartRows = rows.filter((row) => matchesFacets(row, filters));

  function toggleSort(key: SortKey): void {
    setSort((current) =>
      current.key === key ? { key, dir: current.dir === "desc" ? "asc" : "desc" } : { key, dir: "desc" },
    );
  }

  const meId = session.me?.user_id;
  const userLabel = useCallback((userId: string) => sessionUserLabel(userId, meId), [meId]);

  function change(changes: Partial<Filters>): void {
    setParams((current) => withFilters(current, changes), { replace: true });
  }

  function pickDay(key: string): void {
    change({ range: "custom", from: key, to: key });
  }

  function openRow(event: MouseEvent<HTMLTableRowElement>, row: SessionRow): void {
    if ((event.target as HTMLElement).closest("a, button, summary, details")) {
      return;
    }
    navigate(sessionLink(row.session_id));
  }

  if (legacySession) {
    return <Navigate to={sessionLink(legacySession)} replace />;
  }

  const filtered = shown.length !== rows.length;
  const count = `${shown.length} session${shown.length === 1 ? "" : "s"}${filtered ? ` of ${rows.length}` : ""}.`;

  return (
    <Page title="Sessions">
      {seeding ? (
        <>
          <p className="status" role="status">
            {status}
          </p>
          <div className="sessions-empty">
            <img
              className="sessions-empty-art"
              src="/images/pantheon-collab.png"
              width={768}
              height={512}
              alt="Pixel art of humans and robots building a temple together"
            />
            <p className="hint">
              Waiting for <code>cadri init</code> to upload sessions. Paste the install prompt into your agent if you
              have not already.
            </p>
          </div>
        </>
      ) : (
        <>
          <ActivityChart
            rows={chartRows}
            commits={commits}
            filters={filters}
            now={now}
            onChange={change}
            onPickDay={pickDay}
          />
          <SessionFilters
            filters={filters}
            harnesses={distinct(rows, (row) => row.harness)}
            models={distinct(rows, (row) => row.model)}
            users={distinct(rows, (row) => row.user_id).sort((left, right) =>
              userLabel(left).localeCompare(userLabel(right)),
            )}
            userLabel={userLabel}
            onChange={change}
          />
          <p className="status" role="status">
            {status || (phase === "loading" && rows.length === 0 ? "Loading…" : count)}
          </p>
          {memoryPending ? (
            <p className="hint">
              Memories are still being written from these sessions. That usually takes several minutes. The Memories
              page updates when they land.
            </p>
          ) : null}
          {rows.length > 0 ? (
            <div className="table-scroll">
              <table className="session-table">
                <thead>
                  <tr>
                    <th>Session ID</th>
                    <th>Agent Harness</th>
                    <th>User</th>
                    <th>Session Prompt</th>
                    <SortHeader label="Activity" column="activity" sort={sort} onSort={toggleSort} />
                    <SortHeader label="Memories Generated" column="memories" sort={sort} onSort={toggleSort} />
                    <SortHeader label="Start" column="start" sort={sort} onSort={toggleSort} />
                    <SortHeader label="End" column="end" sort={sort} onSort={toggleSort} />
                  </tr>
                </thead>
                <tbody>
                  {ordered.map((row) => (
                    <tr key={row.session_id} className="session-row" onClick={(event) => openRow(event, row)}>
                      <td className="session-id">
                        <Link to={sessionLink(row.session_id)} title={row.session_id}>
                          {shortId(row.session_id)}
                        </Link>
                      </td>
                      <td>
                        <HarnessBadge harness={row.harness} />
                      </td>
                      <td className="session-user">{row.user_id ? userLabel(row.user_id) : "—"}</td>
                      <td className="session-prompt">
                        {row.model ? <span className="session-model">{row.model}</span> : null}
                        <span className="session-prompt-text">{row.prompt || row.preview || "No prompt"}</span>
                      </td>
                      <td>
                        <ActivityBar row={row} />
                      </td>
                      <td>
                        <MemoryLinks commits={row.memory_commits} />
                      </td>
                      <td className="session-time">{formatTime(row.started_at)}</td>
                      <td className="session-time">{formatTime(row.ended_at)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
              {shown.length === 0 ? <p className="memory-empty">No sessions match these filters.</p> : null}
            </div>
          ) : null}
        </>
      )}
    </Page>
  );
}

type SortKey = "activity" | "memories" | "start" | "end";
type SortDir = "asc" | "desc";
type Sort = { key: SortKey; dir: SortDir };

function sortRows(rows: SessionRow[], sort: Sort): SessionRow[] {
  const value = (row: SessionRow): number => {
    switch (sort.key) {
      case "activity":
        return row.event_count;
      case "memories":
        return row.memory_commits.length;
      case "start":
        return Date.parse(row.started_at) || 0;
      case "end":
        return Date.parse(row.ended_at) || 0;
    }
  };
  const direction = sort.dir === "asc" ? 1 : -1;
  return [...rows].sort((left, right) => (value(left) - value(right)) * direction);
}

function SortHeader({
  label,
  column,
  sort,
  onSort,
}: {
  label: string;
  column: SortKey;
  sort: Sort;
  onSort: (key: SortKey) => void;
}) {
  const active = sort.key === column;
  return (
    <th aria-sort={active ? (sort.dir === "asc" ? "ascending" : "descending") : "none"}>
      <button type="button" className={active ? "sort-button active" : "sort-button"} onClick={() => onSort(column)}>
        {label}
        {active ? (
          <span className="sort-arrow" aria-hidden="true">
            {sort.dir === "asc" ? "↑" : "↓"}
          </span>
        ) : null}
      </button>
    </th>
  );
}

function activitySummary(row: SessionRow): string {
  const calls = row.tool_call_count === 1 ? "1 tool call" : `${row.tool_call_count} tool calls`;
  return `${row.user_turns} user, ${row.assistant_turns} agent, ${calls}`;
}

function ActivityBar({ row }: { row: SessionRow }) {
  const total = Math.max(1, row.event_count);
  const parts = [
    { key: "user", count: row.user_turns, tip: "User messages" },
    { key: "assistant", count: row.assistant_turns, tip: "Agent replies" },
    { key: "tool", count: row.tool_call_count, tip: "Tool calls" },
  ];
  return (
    <div className="activity-cell">
      <span className="activity-count">
        <strong>{row.event_count}</strong> events
      </span>
      <span className="activity-track" aria-label={activitySummary(row)}>
        {parts.map((part) =>
          part.count > 0 ? (
            <span
              key={part.key}
              className={`activity-part activity-${part.key}`}
              style={{ width: `${(part.count / total) * 100}%` }}
              title={part.tip}
            />
          ) : null,
        )}
      </span>
      <span className="activity-tail">{activitySummary(row)}</span>
    </div>
  );
}

function memoryCount(count: number): string {
  return `${count} ${count === 1 ? "memory" : "memories"}`;
}

function MemoryLinks({ commits }: { commits: LinkedCommit[] }) {
  if (commits.length === 0) {
    return <span className="session-none">—</span>;
  }
  if (commits.length === 1) {
    const [commit] = commits;
    return (
      <Link className="memory-link" to={commitLink(commit)} title={commit.message}>
        {memoryCount(1)}
      </Link>
    );
  }
  return (
    <details className="memory-links">
      <summary>{memoryCount(commits.length)}</summary>
      <ul>
        {commits.map((commit) => (
          <li key={`${commit.book}:${commit.id}`}>
            <Link to={commitLink(commit)}>{commit.message || shortId(commit.id)}</Link>
          </li>
        ))}
      </ul>
    </details>
  );
}
