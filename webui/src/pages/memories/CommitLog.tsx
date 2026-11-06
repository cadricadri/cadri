import { useMemo } from "react";
import { Link } from "react-router";
import { AuthorMark } from "./AuthorMark";
import type { MemoryCommit } from "./types";

const timeFormat = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
});

export function CommitLog({
  commits,
  scope,
  onOpen,
}: {
  commits: MemoryCommit[];
  scope: "team" | "user";
  onOpen: (path: string) => void;
}) {
  const ordered = useMemo(
    () => [...commits].sort((left, right) => (left.at < right.at ? 1 : left.at > right.at ? -1 : 0)),
    [commits],
  );

  if (ordered.length === 0) {
    return <p className="memory-empty">No commits yet.</p>;
  }

  return (
    <ol className="memory-log">
      {ordered.map((commit) => (
        <li className="memory-commit" key={commit.id}>
          <div className="memory-commit-meta">
            <AuthorMark author={commit.author} sessionId={commit.session_id} />
            <span className="memory-commit-when">
              <time dateTime={commit.at}>{timeFormat.format(new Date(commit.at))}</time>
              <Link className="memory-commit-diff" to={`/memories/commits/${commit.id}${scope === "user" ? "?scope=user" : ""}`}>
                View diff
              </Link>
            </span>
          </div>
          <p>{commit.message}</p>
          <ul className="memory-files">
            {commit.files.map((path) => (
              <li key={path}>
                <button type="button" onClick={() => onOpen(path)}>
                  {path}
                </button>
              </li>
            ))}
          </ul>
        </li>
      ))}
    </ol>
  );
}
