import { useCallback, useEffect, useRef, useState } from "react";
import { Link, useParams, useSearchParams } from "react-router";
import { Page } from "../../page";
import { useSession } from "../../session";
import { shortId, transcriptLink } from "../transcripts/types";
import { AuthorMark } from "./AuthorMark";
import { diffLines, diffStats, type DiffLine } from "./diff";
import type { CommitDetail, FileChange } from "./types";

const when = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });

export default function CommitDiff() {
  const session = useSession();
  const { id = "" } = useParams();
  const [params] = useSearchParams();
  const scope = params.get("scope") === "user" ? "user" : "team";
  const [detail, setDetail] = useState<CommitDetail | null>(null);
  const [status, setStatus] = useState("Loading…");
  const request = useRef(0);

  const load = useCallback(async () => {
    const call = ++request.current;
    const query = scope === "user" ? "?scope=user" : "";
    try {
      const response = await fetch(`/api/memories/commits/${encodeURIComponent(id)}${query}`, { headers: session.headers() });
      if (call !== request.current) {
        return;
      }
      if (response.status === 401) {
        session.unauthorized();
        return;
      }
      if (response.status === 404 || response.status === 400) {
        setDetail(null);
        setStatus(`No commit ${shortId(id)} in the ${scope === "user" ? "personal" : "team"} book.`);
        return;
      }
      if (!response.ok) {
        throw new Error(`${response.status} ${response.statusText}`);
      }
      setDetail((await response.json()) as CommitDetail);
      setStatus("");
    } catch (error) {
      if (call !== request.current) {
        return;
      }
      setStatus(error instanceof Error ? `Could not load the commit: ${error.message}` : "Could not load the commit.");
    }
  }, [id, scope, session]);

  useEffect(() => {
    setDetail(null);
    setStatus("Loading…");
    void load();
  }, [load]);

  const historyLink = scope === "user" ? "/?scope=user&view=committed" : "/?view=committed";

  return (
    <Page
      title="Memory commit"
      beside={detail ? <code className="commit-sha">{shortId(detail.id)}</code> : null}
      actions={
        <Link className="btn ghost" to={historyLink}>
          Commit history
        </Link>
      }
    >
      {status ? (
        <p className="status" role="status">
          {status}
        </p>
      ) : null}
      {detail ? (
        <>
          <section className="commit-head">
            <p className="commit-message">{detail.message || "Update memory"}</p>
            <div className="commit-meta">
              <AuthorMark author={detail.author} />
              <span>{detail.book === "user" ? "Personal book" : "Team book"}</span>
              <time dateTime={detail.at}>{when.format(new Date(detail.at))}</time>
              <code title={detail.id}>{detail.id}</code>
            </div>
            {detail.session_id ? (
              <p className="commit-source">
                Created in session{" "}
                <Link className="text-link" to={transcriptLink(detail.session_id)}>
                  <code>{shortId(detail.session_id)}</code>
                </Link>
              </p>
            ) : (
              <p className="commit-source side-mute">This commit does not name the session that wrote it.</p>
            )}
          </section>
          {detail.changes.length === 0 ? <p className="memory-empty">This commit changed no files.</p> : null}
          {detail.changes.map((change) => (
            <FileDiff key={change.path} change={change} scope={scope} />
          ))}
        </>
      ) : null}
    </Page>
  );
}

function FileDiff({ change, scope }: { change: FileChange; scope: "team" | "user" }) {
  const lines = diffLines(change.before, change.after);
  const { added, removed } = diffStats(lines);
  const state = change.before === null ? "added" : change.after === null ? "deleted" : "modified";
  const fileLink = `/?${new URLSearchParams(scope === "user" ? { scope: "user", file: change.path } : { file: change.path })}`;
  return (
    <section className="diff-file">
      <header className="diff-file-head">
        {change.after !== null ? (
          <Link className="diff-path" to={fileLink}>
            {change.path}
          </Link>
        ) : (
          <span className="diff-path">{change.path}</span>
        )}
        <span className={`diff-state diff-${state}`}>{state}</span>
        <span className="diff-stats">
          <span className="diff-plus">+{added}</span> <span className="diff-minus">−{removed}</span>
        </span>
      </header>
      <div className="diff-scroll">
        <table className="diff-table">
          <tbody>
            {lines.map((line, index) => (
              <DiffRow key={index} line={line} />
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}

function DiffRow({ line }: { line: DiffLine }) {
  if (line.kind === "skip") {
    return (
      <tr className="diff-skip">
        <td colSpan={3}>
          ⋯ {line.count} unchanged line{line.count === 1 ? "" : "s"}
        </td>
      </tr>
    );
  }
  const sign = line.kind === "add" ? "+" : line.kind === "del" ? "−" : " ";
  return (
    <tr className={`diff-line diff-${line.kind}`}>
      <td className="diff-no">{line.oldNo ?? ""}</td>
      <td className="diff-no">{line.newNo ?? ""}</td>
      <td className="diff-text">
        <span className="diff-sign">{sign}</span>
        {line.text}
      </td>
    </tr>
  );
}
