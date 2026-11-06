import { useCallback, useEffect, useRef, useState } from "react";
import { Link, useParams } from "react-router";
import { Page } from "../../page";
import { useSession } from "../../session";
import { ChatThread } from "./ChatThread";
import { HarnessLogo } from "./HarnessLogo";
import { TranscriptSidebar } from "./TranscriptSidebar";
import { shortId, type TranscriptDetail as Detail } from "./types";

export default function TranscriptDetail() {
  const session = useSession();
  const { sessionId = "" } = useParams();
  const [detail, setDetail] = useState<Detail | null>(null);
  const [status, setStatus] = useState("Loading…");
  const request = useRef(0);

  const load = useCallback(async () => {
    const id = ++request.current;
    try {
      const response = await fetch(`/api/transcripts/${encodeURIComponent(sessionId)}`, { headers: session.headers() });
      if (id !== request.current) {
        return;
      }
      if (response.status === 401) {
        session.unauthorized();
        return;
      }
      if (response.status === 404) {
        setDetail(null);
        setStatus(`Session ${sessionId} is not in Cadri yet.`);
        return;
      }
      if (response.status === 402) {
        setDetail(null);
        setStatus("This team's trial has ended.");
        return;
      }
      if (!response.ok) {
        throw new Error(`${response.status} ${response.statusText}`);
      }
      setDetail((await response.json()) as Detail);
      setStatus("");
    } catch (error) {
      if (id !== request.current) {
        return;
      }
      setStatus(error instanceof Error ? `Could not load the transcript: ${error.message}` : "Could not load the transcript.");
    }
  }, [session, sessionId]);

  useEffect(() => {
    setDetail(null);
    setStatus("Loading…");
    void load();
  }, [load]);

  const userLabel = detail?.user_id ? (detail.user_id === session.me?.user_id ? "Me" : detail.user_id) : null;

  return (
    <Page
      title="Transcript"
      beside={
        detail ? (
          <span className="transcript-title-id">
            <HarnessLogo harness={detail.harness} size={18} />
            <code title={detail.session_id}>{shortId(detail.session_id)}</code>
          </span>
        ) : null
      }
      actions={
        <Link className="btn ghost btn-back" to="/transcripts">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <path d="M19 12H5M11 18l-6-6 6-6" />
          </svg>
          All transcripts
        </Link>
      }
    >
      {status ? (
        <p className="status" role="status">
          {status}
        </p>
      ) : null}
      {detail ? (
        <div className="transcript-detail">
          <section className="transcript-chat" aria-label="Conversation">
            <ChatThread messages={detail.messages} />
          </section>
          <TranscriptSidebar detail={detail} userLabel={userLabel} />
        </div>
      ) : null}
    </Page>
  );
}
