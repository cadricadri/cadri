import { useEffect, useRef, useState, type Ref } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { HumanMark, RobotMark } from "../../icons";
import { blameGroups, type BlameGroup } from "./blame";
import { AuthorMark } from "./AuthorMark";
import type { MemoryAuthor, MemoryCommit, MemoryFile } from "./types";

export type FileTab = "preview" | "code";

export function FileView({
  file,
  commits,
  paths,
  tab,
  onTab,
  onSave,
}: {
  file: MemoryFile;
  commits: MemoryCommit[];
  paths: string[];
  tab: FileTab;
  onTab: (tab: FileTab) => void;
  onSave: (content: string, message: string, nextPath: string) => void;
}) {
  const filename = file.path.split("/").pop() ?? file.path;
  const directory = file.path.includes("/") ? file.path.slice(0, file.path.lastIndexOf("/")) : "";
  const [buffer, setBuffer] = useState(file.content);
  const [name, setName] = useState(filename);
  const [editing, setEditing] = useState(false);
  const [blameOpen, setBlameOpen] = useState(false);
  const textRef = useRef<HTMLTextAreaElement>(null);
  const numbersRef = useRef<HTMLDivElement>(null);
  const railRef = useRef<HTMLDivElement>(null);
  const focusEdit = useRef(false);
  const parts = file.path.split("/");
  const lines = buffer.split("\n");
  const nextPath = memoryPath(directory, name);
  const nameChanged = nextPath !== null && nextPath !== file.path;
  const contentChanged = buffer !== file.content;
  const nameTaken = nextPath !== null && nextPath !== file.path && paths.includes(nextPath);
  const canCommit = nextPath !== null && !nameTaken && (contentChanged || nameChanged);
  const nameProblem =
    name.trim() === filename
      ? null
      : nextPath === null
        ? "Use a file name without slashes."
        : nameTaken
          ? "A file with this name already exists."
          : null;

  function syncScroll(): void {
    const text = textRef.current;
    if (!text) {
      return;
    }
    if (numbersRef.current) {
      numbersRef.current.scrollTop = text.scrollTop;
    }
    if (railRef.current) {
      railRef.current.scrollTop = text.scrollTop;
    }
  }

  useEffect(() => {
    if (editing && tab === "code" && focusEdit.current) {
      focusEdit.current = false;
      textRef.current?.focus();
    }
  }, [editing, tab]);

  function startEdit(): void {
    focusEdit.current = true;
    setEditing(true);
    if (tab !== "code") {
      onTab("code");
    }
  }

  function cancel(): void {
    setBuffer(file.content);
    setName(filename);
    setEditing(false);
  }

  function commit(): void {
    if (!nextPath || !canCommit) {
      return;
    }
    const nextName = nextPath.split("/").pop() ?? nextPath;
    const message =
      nameChanged && !contentChanged ? `Rename ${filename} to ${nextName}` : `Update ${nextName}`;
    onSave(buffer, message, nextPath);
  }

  return (
    <div className="memory-file">
      <div className="memory-file-head">
        <div className="memory-slug">
          <p className="memory-crumb">
            {parts.map((part, index) => {
              const last = index === parts.length - 1;
              return (
                <span key={`${part}-${index}`}>
                  {index > 0 ? (
                    <span className="memory-sep" aria-hidden="true">
                      /
                    </span>
                  ) : null}
                  {last && editing && tab === "code" ? (
                    <input
                      className="memory-name"
                      aria-label="File name"
                      aria-invalid={nameProblem !== null}
                      value={name}
                      size={Math.max(name.length, 8)}
                      spellCheck={false}
                      onChange={(event) => setName(event.target.value)}
                    />
                  ) : (
                    <span aria-current={last ? "page" : undefined}>{part}</span>
                  )}
                </span>
              );
            })}
            {nameProblem ? <span className="memory-name-note">{nameProblem}</span> : null}
          </p>
          <div className="memory-file-actions">
            {tab === "code" && editing ? (
                <>
                  <button className="btn ghost" type="button" onClick={cancel}>
                    Cancel changes
                  </button>
                  <button className="btn" type="button" disabled={!canCommit} onClick={commit}>
                    Commit Changes
                  </button>
                </>
              ) : (
                <button className="btn" type="button" onClick={startEdit}>
                  Edit
                </button>
              )}
          </div>
        </div>
        <div className="memory-tabs" role="tablist" aria-label="File view">
          {(["preview", "code"] as const).map((item) => (
            <button
              key={item}
              type="button"
              role="tab"
              id={`file-tab-${item}`}
              aria-selected={tab === item}
              aria-controls={`file-panel-${item}`}
              onClick={() => onTab(item)}
            >
              {item === "preview" ? "Preview" : "Code"}
            </button>
          ))}
        </div>
      </div>
      {tab === "preview" ? (
        <section className="memory-preview" id="file-panel-preview" role="tabpanel" aria-labelledby="file-tab-preview">
          <ReactMarkdown remarkPlugins={[remarkGfm]}>{buffer}</ReactMarkdown>
        </section>
      ) : (
        <section className="memory-edit" id="file-panel-code" role="tabpanel" aria-labelledby="file-tab-code">
          {blameOpen ? <BlameLegend file={file} /> : null}
          <div className="memory-editor">
            <button
              className="code-blame-bar"
              type="button"
              aria-pressed={blameOpen}
              aria-label={blameOpen ? "Hide blame" : "Show blame"}
              onClick={() => setBlameOpen((open) => !open)}
            >
              <span>Blame</span>
            </button>
            {blameOpen && !editing ? (
              <div className="memory-blame">
                <BlameView file={file} buffer={buffer} commits={commits} />
              </div>
            ) : (
              <>
                {blameOpen ? <BlameRail railRef={railRef} file={file} buffer={buffer} commits={commits} /> : null}
                {editing ? (
                  <CodeLines numbersRef={numbersRef} lines={lines} />
                ) : (
                  <div className="code-scroll">
                    <CodeLines lines={lines} />
                    <pre className="code-text">{buffer.length > 0 ? buffer : " "}</pre>
                  </div>
                )}
                {editing ? (
                  <textarea
                    ref={textRef}
                    aria-label={`Edit ${file.path}`}
                    value={buffer}
                    wrap="off"
                    spellCheck={false}
                    autoCapitalize="off"
                    autoCorrect="off"
                    onChange={(event) => setBuffer(event.target.value)}
                    onScroll={syncScroll}
                  />
                ) : null}
              </>
            )}
          </div>
        </section>
      )}
    </div>
  );
}

function CodeLines({ lines, numbersRef }: { lines: string[]; numbersRef?: Ref<HTMLDivElement> }) {
  return (
    <div className="code-gutter" ref={numbersRef} aria-hidden="true">
      {lines.map((_, index) => (
        <div key={index}>{index + 1}</div>
      ))}
    </div>
  );
}

const heatStops = ["1", "2", "3", "4", "5"] as const;

function BlameLegend({ file }: { file: MemoryFile }) {
  const people = contributors(file);
  return (
    <div className="blame-legend">
      <div className="blame-scale" aria-hidden="true">
        <span>Older</span>
        {heatStops.map((stop) => (
          <i key={stop} className="blame-swatch" data-heat={stop} />
        ))}
        <span>Newer</span>
      </div>
      <div className="blame-contributors">
        <span className="blame-faces">
          {people.slice(0, 4).map((author) => (
            <span key={authorKey(author)} title={author.kind === "human" ? author.label : author.session_id}>
              {author.kind === "human" ? <HumanMark /> : <RobotMark />}
            </span>
          ))}
        </span>
        <span>Contributors</span>
        <strong>{people.length}</strong>
      </div>
    </div>
  );
}

function contributors(file: MemoryFile): MemoryAuthor[] {
  const counts = new Map<string, { author: MemoryAuthor; count: number }>();
  for (const entry of file.blame) {
    const key = authorKey(entry.author);
    const row = counts.get(key);
    if (row) {
      row.count += 1;
    } else {
      counts.set(key, { author: entry.author, count: 1 });
    }
  }
  return [...counts.values()].sort((a, b) => b.count - a.count).map((row) => row.author);
}

function authorKey(author: MemoryAuthor): string {
  return author.kind === "human" ? `human:${author.user_id}` : `robot:${author.session_id}`;
}

function BlameRail({
  railRef,
  file,
  buffer,
  commits,
}: {
  railRef: Ref<HTMLDivElement>;
  file: MemoryFile;
  buffer: string;
  commits: MemoryCommit[];
}) {
  const groups = blameGroups(file.blame, file.content, buffer);
  const byId = new Map(commits.map((commit) => [commit.id, commit]));
  const stamps = heatStamps(groups, byId);

  return (
    <div className="blame-rail" ref={railRef} aria-hidden="true">
      {groups.map((group, index) => {
        const commit = group.commitId ? byId.get(group.commitId) : undefined;
        return (
          <div
            className="blame-rail-group"
            key={`${group.commitId ?? "unsaved"}-${group.lines[0]?.number ?? index}`}
            data-heat={group.unsaved ? "unsaved" : String(heat(commit?.at, stamps))}
            style={{ height: group.lines.length * 22 }}
          >
            {group.unsaved || !group.author ? (
              <span className="author-mark blame-unsaved">unsaved</span>
            ) : (
              <AuthorMark author={group.author} />
            )}
            <p>{commit?.message ?? "Unsaved edit"}</p>
          </div>
        );
      })}
    </div>
  );
}

function BlameView({ file, buffer, commits }: { file: MemoryFile; buffer: string; commits: MemoryCommit[] }) {
  const groups = blameGroups(file.blame, file.content, buffer);
  const byId = new Map(commits.map((commit) => [commit.id, commit]));
  const stamps = heatStamps(groups, byId);

  return (
    <div className="blame-sheet">
      {groups.map((group, index) => {
        const commit = group.commitId ? byId.get(group.commitId) : undefined;
        return (
          <article
            className="blame-group"
            key={`${group.commitId ?? "unsaved"}-${group.lines[0]?.number ?? index}`}
            data-heat={group.unsaved ? "unsaved" : String(heat(commit?.at, stamps))}
          >
            <header className="blame-meta">
              {group.unsaved || !group.author ? (
                <span className="author-mark blame-unsaved">unsaved</span>
              ) : (
                <AuthorMark author={group.author} />
              )}
              <p>{commit?.message ?? "Unsaved edit"}</p>
              {commit ? <time dateTime={commit.at}>{ago(commit.at)}</time> : null}
            </header>
            <pre className="blame-code">
              {group.lines.map((line) => (
                <span className="blame-code-line" key={line.number}>
                  <span>{line.number}</span>
                  <span>{line.text.length > 0 ? line.text : " "}</span>
                </span>
              ))}
            </pre>
          </article>
        );
      })}
    </div>
  );
}

function heatStamps(groups: BlameGroup[], byId: Map<string, MemoryCommit>): number[] {
  return groups
    .map((group) => (group.commitId ? Date.parse(byId.get(group.commitId)?.at ?? "") : Number.NaN))
    .filter((stamp) => !Number.isNaN(stamp));
}

function memoryPath(directory: string, name: string): string | null {
  const trimmed = name.trim();
  if (trimmed.length === 0 || trimmed === "." || trimmed === ".." || /[\\/]/.test(trimmed)) {
    return null;
  }
  return directory.length > 0 ? `${directory}/${trimmed}` : trimmed;
}

function heat(at: string | undefined, stamps: number[]): number {
  if (!at || stamps.length === 0) {
    return 3;
  }
  const time = Date.parse(at);
  const oldest = Math.min(...stamps);
  const newest = Math.max(...stamps);
  if (!Number.isFinite(time) || newest === oldest) {
    return 3;
  }
  return Math.min(5, Math.max(1, Math.round(((time - oldest) / (newest - oldest)) * 4) + 1));
}

function ago(at: string): string {
  const delta = Date.now() - Date.parse(at);
  const minute = 60 * 1000;
  const hour = 60 * minute;
  const day = 24 * hour;
  if (!Number.isFinite(delta) || delta < minute) {
    return "just now";
  }
  if (delta < hour) {
    const minutes = Math.round(delta / minute);
    return `${minutes} minute${minutes === 1 ? "" : "s"} ago`;
  }
  if (delta < day) {
    const hours = Math.round(delta / hour);
    return `${hours} hour${hours === 1 ? "" : "s"} ago`;
  }
  const days = Math.round(delta / day);
  if (days < 14) {
    return `${days} day${days === 1 ? "" : "s"} ago`;
  }
  const weeks = Math.round(days / 7);
  if (weeks < 8) {
    return `${weeks} week${weeks === 1 ? "" : "s"} ago`;
  }
  const months = Math.round(days / 30);
  return `${months} month${months === 1 ? "" : "s"} ago`;
}
