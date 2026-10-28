import { useCallback, useEffect, useRef, useState } from "react";
import { useSearchParams } from "react-router";
import { Page } from "../page";
import { useSession } from "../session";

type Transcript = {
  session_id: string;
  cwd: string | null;
  hook_event: string | null;
  content: string;
  created_at: string;
  updated_at: string;
  bytes: number;
  preview: string;
};

function scrollToSession(row: HTMLTableRowElement | null): void {
  row?.scrollIntoView({ block: "nearest" });
}

function statusWithSession(
  status: string,
  phase: "loading" | "done",
  sessionQuery: string | null,
  rows: Transcript[],
): string {
  if (phase !== "done" || !sessionQuery) {
    return status;
  }
  const found = rows.some((row) => row.session_id === sessionQuery);
  return found
    ? `${status} Showing session ${sessionQuery}.`
    : `${status} Session ${sessionQuery} is not in this list yet.`;
}

function formatTranscript(content: string): string {
  return content
    .split("\n")
    .filter((line) => line.trim().length > 0)
    .map((line) => {
      try {
        return JSON.stringify(JSON.parse(line), null, 2);
      } catch {
        return line;
      }
    })
    .join("\n\n");
}

export default function Transcripts() {
  const session = useSession();
  const [params] = useSearchParams();
  const sessionQuery = params.get("session");
  const [status, setStatus] = useState("Loading…");
  const [phase, setPhase] = useState<"loading" | "done">("loading");
  const [rows, setRows] = useState<Transcript[]>([]);
  const request = useRef(0);

  const load = useCallback(async () => {
    const id = ++request.current;
    setStatus("Loading…");
    setPhase("loading");
    try {
      const response = await fetch("/api/transcripts", { headers: session.headers() });
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
      const transcripts = (await response.json()) as Transcript[];
      setRows(transcripts);
      setStatus(
        transcripts.length === 0
          ? "No transcripts yet."
          : `${transcripts.length} transcript${transcripts.length === 1 ? "" : "s"}.`,
      );
      setPhase("done");
    } catch (error) {
      if (id !== request.current) {
        return;
      }
      setRows([]);
      setStatus(
        error instanceof Error ? `Could not load transcripts: ${error.message}` : "Could not load transcripts.",
      );
      setPhase("done");
    }
  }, [session]);

  useEffect(() => {
    void load();
  }, [load]);

  const shown = statusWithSession(status, phase, sessionQuery, rows);

  return (
    <Page
      title="Transcripts"
      actions={
        <button className="btn ghost" type="button" onClick={() => void load()}>
          Refresh
        </button>
      }
    >
      <p className="status" role="status">
        {shown}
      </p>
      <div className="table-scroll">
        <table>
          <thead>
            <tr>
              <th>Session</th>
              <th>Updated</th>
              <th>Directory</th>
              <th>Hook</th>
              <th>Bytes</th>
              <th>Transcript</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((transcript) => (
              <tr
                key={transcript.session_id}
                className={transcript.session_id === sessionQuery ? "transcript-hit" : undefined}
                ref={transcript.session_id === sessionQuery ? scrollToSession : undefined}
              >
                <td>{transcript.session_id}</td>
                <td>{transcript.updated_at}</td>
                <td>{transcript.cwd ?? ""}</td>
                <td>{transcript.hook_event ?? ""}</td>
                <td>{transcript.bytes}</td>
                <td>
                  <pre className="transcript">{formatTranscript(transcript.content)}</pre>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </Page>
  );
}
