import { useMemo } from "react";
import { AuthorMark } from "./AuthorMark";
import type { MemoryCommit } from "./types";

const timeFormat = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
});

export function CommitLog({ commits, onOpen }: { commits: MemoryCommit[]; onOpen: (path: string) => void }) {
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
            <AuthorMark author={commit.author} />
            <time dateTime={commit.at}>{timeFormat.format(new Date(commit.at))}</time>
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
