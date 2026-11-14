import { useCallback, useEffect, useRef, useState } from "react";
import { Link, useParams } from "react-router";
import { funName, sessionUserLabel } from "../../funNames";
import { Page } from "../../page";
import { useSession } from "../../session";
import { ChatThread } from "./ChatThread";
import { HarnessLogo } from "./HarnessLogo";
import { SessionSidebar } from "./SessionSidebar";
import { shortId, type SessionDetail as Detail } from "./types";

export default function SessionDetail() {
  const session = useSession();
  const { sessionId = "" } = useParams();
  const [detail, setDetail] = useState<Detail | null>(null);
  const [status, setStatus] = useState("Loading…");
  const request = useRef(0);

  const load = useCallback(async () => {
    const id = ++request.current;
    try {
      const response = await fetch(`/api/sessions/${encodeURIComponent(sessionId)}`, { headers: session.headers() });
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
      setStatus(error instanceof Error ? `Could not load the session: ${error.message}` : "Could not load the session.");
    }
  }, [session, sessionId]);

  useEffect(() => {
    setDetail(null);
    setStatus("Loading…");
    void load();
  }, [load]);

  const userLabel = detail?.user_id ? sessionUserLabel(detail.user_id, session.me?.user_id) : null;

  return (
    <Page
      title="Session"
      beside={
        detail ? (
          <span className="session-title-id">
            <HarnessLogo harness={detail.harness} size={18} />
            <code title={detail.session_id}>{shortId(detail.session_id)}</code>
          </span>
        ) : null
      }
      actions={
        <Link className="btn ghost btn-back" to="/sessions">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <path d="M19 12H5M11 18l-6-6 6-6" />
          </svg>
          All sessions
        </Link>
      }
    >
      {status ? (
        <p className="status" role="status">
          {status}
        </p>
      ) : null}
      {detail ? (
        <div className="session-detail">
          <section className="session-chat" aria-label="Conversation">
            <ChatThread
              messages={detail.messages}
              speaker={detail.user_id ? funName(detail.user_id) : "You"}
            />
          </section>
          <SessionSidebar detail={detail} userLabel={userLabel} />
        </div>
      ) : null}
    </Page>
  );
}
