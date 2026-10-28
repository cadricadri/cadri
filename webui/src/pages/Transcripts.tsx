import { useCallback, useEffect, useRef, useState } from "react";
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
  const [status, setStatus] = useState("Loading…");
  const [rows, setRows] = useState<Transcript[]>([]);
  const request = useRef(0);

  const load = useCallback(async () => {
    const id = ++request.current;
    setStatus("Loading…");
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
    } catch (error) {
      if (id !== request.current) {
        return;
      }
      setRows([]);
      setStatus(
        error instanceof Error ? `Could not load transcripts: ${error.message}` : "Could not load transcripts.",
      );
    }
  }, [session]);

  useEffect(() => {
    void load();
  }, [load]);

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
        {status}
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
              <tr key={transcript.session_id}>
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
