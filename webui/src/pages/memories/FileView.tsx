import { useEffect, useLayoutEffect, useRef, useState, type ReactNode, type Ref } from "react";
import { Link } from "react-router";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { HumanMark, RobotMark } from "../../icons";
import { displayAuthorName } from "../../funNames";
import { blameGroups, type BlameGroup } from "./blame";
import { AuthorMark } from "./AuthorMark";
import type { LoadedFile, MemoryAuthor, MemoryCommit, MemoryFile } from "./types";

export type FileTab = "preview" | "code";

export function FileView({
  file,
  createIn,
  scope,
  commits,
  paths,
  directories = [],
  tab,
  onTab,
  onSave,
  onEditingChange,
  onDiscard,
  onNeedHistory,
  onNeedBlame,
}: {
  file: LoadedFile;
  createIn?: string;
  scope: "team" | "user";
  commits: MemoryCommit[];
  paths: string[];
  directories?: string[];
  tab: FileTab;
  onTab: (tab: FileTab) => void;
  onSave: (content: string, message: string, nextPath: string) => void;
  onEditingChange: (editing: boolean) => void;
  onDiscard?: () => void;
  onNeedHistory?: () => void;
  onNeedBlame?: () => void;
}) {
  const creating = createIn !== undefined;
  const filename = creating ? "" : (file.path.split("/").pop() ?? file.path);
  const directory = creating ? createIn : file.path.includes("/") ? file.path.slice(0, file.path.lastIndexOf("/")) : "";
  const [buffer, setBuffer] = useState(file.content);
  const [name, setName] = useState(filename);
  const [editing, setEditing] = useState(creating);
  const [blameOpen, setBlameOpen] = useState(false);
  const [pointerOnAge, setPointerOnAge] = useState(false);
  const [ageFocused, setAgeFocused] = useState(false);
  const blameHover = pointerOnAge || ageFocused;
  const textRef = useRef<HTMLTextAreaElement>(null);
  const nameRef = useRef<HTMLInputElement>(null);
  const numbersRef = useRef<HTMLDivElement>(null);
  const railRef = useRef<HTMLDivElement>(null);
  const focusEdit = useRef(false);
  const place = directory.length > 0 ? directory.split("/") : [];
  const lines = buffer.split("\n");
  const nextPath = memoryPath(directory, name);
  const nameChanged = nextPath !== null && nextPath !== file.path;
  const contentChanged = buffer !== file.content;
  const fileClash = nextPath !== null && nextPath !== file.path && paths.includes(nextPath);
  const folderClash =
    nextPath !== null &&
    directories.some((item) => item === nextPath || item.startsWith(`${nextPath}/`));
  const nameTaken = fileClash || folderClash;
  const canCommit = nextPath !== null && !nameTaken && (creating || contentChanged || nameChanged);
  const nameProblem =
    name.trim() === filename
      ? null
      : nextPath === null
        ? "Use a file name without slashes."
        : fileClash
          ? "A file with this name already exists."
          : folderClash
            ? "A folder with this name already exists."
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
    if (creating && tab === "code") {
      nameRef.current?.focus();
      return;
    }
    if (editing && tab === "code" && focusEdit.current) {
      focusEdit.current = false;
      textRef.current?.focus();
    }
  }, [creating, editing, tab]);

  const blameLoaded = file.blame.length > 0;
  const needBlame = useRef(onNeedBlame);
  const needHistory = useRef(onNeedHistory);
  needBlame.current = onNeedBlame;
  needHistory.current = onNeedHistory;
  useEffect(() => {
    if (creating) {
      return;
    }
    needHistory.current?.();
    if (!blameLoaded) {
      needBlame.current?.();
    }
  }, [creating, blameLoaded, file.path]);
  useEffect(() => {
    if (blameOpen && !creating && !blameLoaded) {
      onNeedBlame?.();
      onNeedHistory?.();
    }
  }, [blameOpen, creating, blameLoaded, onNeedBlame, onNeedHistory]);

  useEffect(() => {
    onEditingChange(editing);
  }, [editing, onEditingChange]);

  useEffect(() => {
    return () => onEditingChange(false);
  }, [onEditingChange]);

  function startEdit(): void {
    focusEdit.current = true;
    setEditing(true);
    if (tab !== "code") {
      onTab("code");
    }
  }

  function cancel(): void {
    if (creating) {
      onDiscard?.();
      return;
    }
    setBuffer(file.content);
    setName(filename);
    setEditing(false);
  }

  function commit(): void {
    if (!nextPath || !canCommit) {
      return;
    }
    const nextName = nextPath.split("/").pop() ?? nextPath;
    const message = creating
      ? `Create ${nextPath}`
      : nameChanged && !contentChanged
        ? `Rename ${filename} to ${nextName}`
        : `Update ${nextName}`;
    onSave(buffer, message, nextPath);
  }

  const edited = creating ? null : latestBlameCommit(file, commits);
  const blameExpanded = blameOpen || blameHover;
  const shownTab: FileTab = blameHover && !creating ? "code" : tab;
  const showSheet = blameOpen && !editing;

  return (
    <div className="memory-file">
      <div className="memory-file-head">
        <div className="memory-slug">
          <p className="memory-crumb">
            {place.map((part, index) => (
              <span key={`${part}-${index}`}>
                {index > 0 ? (
                  <span className="memory-sep" aria-hidden="true">
                    /
                  </span>
                ) : null}
                <span>{part}</span>
              </span>
            ))}
            {place.length > 0 ? (
              <span className="memory-sep" aria-hidden="true">
                /
              </span>
            ) : null}
            {creating || (editing && tab === "code") ? (
              <input
                ref={nameRef}
                className="memory-name"
                aria-label="File name"
                aria-invalid={nameProblem !== null}
                placeholder={creating ? "filename.md" : undefined}
                value={name}
                size={Math.max(name.length, 12)}
                spellCheck={false}
                onChange={(event) => setName(event.target.value)}
              />
            ) : (
              <span aria-current="page">{filename}</span>
            )}
            {edited ? (
              <Link
                className="memory-edited"
                to={commitPath(edited.id, scope)}
                title={edited.message}
                onMouseEnter={() => {
                  onNeedBlame?.();
                  onNeedHistory?.();
                  setPointerOnAge(true);
                }}
                onMouseLeave={() => setPointerOnAge(false)}
                onFocus={() => setAgeFocused(true)}
                onBlur={() => setAgeFocused(false)}
              >
                (last edited <time dateTime={edited.at}>{ago(edited.at)}</time>)
              </Link>
            ) : null}
            {nameProblem ? <span className="memory-name-note">{nameProblem}</span> : null}
          </p>
          <div className="memory-file-actions">
            {creating || (tab === "code" && editing) ? (
                <>
                  <button className="btn ghost" type="button" onClick={cancel}>
                    {creating ? "Cancel" : "Cancel changes"}
                  </button>
                  <button className="btn" type="button" disabled={!canCommit} onClick={commit}>
                    {creating ? "Create file" : "Commit Changes"}
                  </button>
                </>
              ) : (
                <button className="btn" type="button" onClick={startEdit}>
                  Edit
                </button>
              )}
          </div>
        </div>
        <div className="memory-tabbar">
          <div className="memory-tabs" role="tablist" aria-label="File view">
            {(["preview", "code"] as const).map((item) => (
              <button
                key={item}
                type="button"
                role="tab"
                id={`file-tab-${item}`}
                aria-selected={shownTab === item}
                aria-controls={`file-panel-${item}`}
                onClick={() => onTab(item)}
              >
                {item === "preview" ? "Preview" : "Code"}
              </button>
            ))}
          </div>
          {blameLoaded ? <Contributors file={file} /> : null}
        </div>
      </div>
      {shownTab === "preview" ? (
        <section className="memory-preview" id="file-panel-preview" role="tabpanel" aria-labelledby="file-tab-preview">
          <ReactMarkdown remarkPlugins={[remarkGfm]}>{buffer}</ReactMarkdown>
        </section>
      ) : (
        <section className="memory-edit" id="file-panel-code" role="tabpanel" aria-labelledby="file-tab-code">
          {blameExpanded ? <BlameLegend /> : null}
          <div className="memory-editor">
            <button
              className={blameExpanded ? "code-blame-bar is-open" : "code-blame-bar"}
              type="button"
              aria-pressed={blameOpen}
              aria-label={blameOpen ? "Hide blame" : "Show blame"}
              onClick={() =>
                setBlameOpen((open) => {
                  if (!open) {
                    onNeedHistory?.();
                  }
                  return !open;
                })
              }
            >
              <span>Blame</span>
            </button>
            {blameOpen && !blameLoaded && !creating ? (
              <p className="status" role="status">
                Loading blame…
              </p>
            ) : blameOpen && !editing ? (
              <div className="memory-blame">
                <BlameView file={file} buffer={buffer} commits={commits} />
              </div>
            ) : (
              <>
                <ExpandRail open={blameExpanded && !showSheet}>
                  {!blameLoaded && !creating ? (
                    <div className="blame-rail">
                      <p className="blame-rail-loading">Loading…</p>
                    </div>
                  ) : (
                    <BlameRail railRef={railRef} file={file} buffer={buffer} commits={commits} />
                  )}
                </ExpandRail>
                {editing ? (
                  <CodeLines numbersRef={numbersRef} lines={lines} />
                ) : (
                  <div
                    className="code-scroll"
                    onScroll={(event) => {
                      if (railRef.current) {
                        railRef.current.scrollTop = event.currentTarget.scrollTop;
                      }
                    }}
                  >
                    <CodeLines lines={lines} />
                    <pre className="code-text">{buffer.length > 0 ? buffer : " "}</pre>
                  </div>
                )}
                {editing ? (
                  <textarea
                    ref={textRef}
                    aria-label={creating ? "New file" : `Edit ${file.path}`}
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

function latestBlameCommit(file: LoadedFile, commits: MemoryCommit[]): MemoryCommit | null {
  if (file.blame.length === 0) {
    return null;
  }
  const byId = new Map(commits.map((commit) => [commit.id, commit]));
  let best: MemoryCommit | null = null;
  let bestAt = Number.NEGATIVE_INFINITY;
  const seen = new Set<string>();
  for (const entry of file.blame) {
    if (seen.has(entry.commit_id)) {
      continue;
    }
    seen.add(entry.commit_id);
    const commit = byId.get(entry.commit_id);
    if (!commit) {
      return null;
    }
    const at = Date.parse(commit.at);
    if (Number.isNaN(at) || at <= bestAt) {
      continue;
    }
    best = commit;
    bestAt = at;
  }
  return best;
}

function commitPath(id: string, scope: "team" | "user"): string {
  const path = `/memories/commits/${encodeURIComponent(id)}`;
  return scope === "user" ? `${path}?scope=user` : path;
}

const blameRailMs = 180;

function ExpandRail({ open, children }: { open: boolean; children: ReactNode }) {
  const [mounted, setMounted] = useState(open);
  const [wide, setWide] = useState(false);

  useLayoutEffect(() => {
    if (!open) {
      setWide(false);
      if (blameMotionMs() === 0) {
        setMounted(false);
        return;
      }
      const timer = window.setTimeout(() => setMounted(false), blameRailMs);
      return () => window.clearTimeout(timer);
    }
    setMounted(true);
  }, [open]);

  useEffect(() => {
    if (!open || !mounted) {
      return;
    }
    if (blameMotionMs() === 0) {
      setWide(true);
      return;
    }
    const frame = requestAnimationFrame(() => setWide(true));
    return () => cancelAnimationFrame(frame);
  }, [open, mounted]);

  if (!mounted) {
    return null;
  }
  return <div className={wide ? "blame-rail-slot open" : "blame-rail-slot"}>{children}</div>;
}

function blameMotionMs(): number {
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches ? 0 : blameRailMs;
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

function Contributors({ file }: { file: MemoryFile }) {
  const people = contributors(file);
  return (
    <div className="blame-contributors">
      <span className="blame-faces">
        {people.slice(0, 4).map((author) => (
          <span
            key={authorKey(author)}
            title={author.kind === "human" ? displayAuthorName(author.user_id, author.label) : author.id}
          >
            {author.kind === "human" ? <HumanMark /> : <RobotMark />}
          </span>
        ))}
      </span>
      <span>Contributors</span>
      <strong>{people.length}</strong>
    </div>
  );
}

function BlameLegend() {
  return (
    <div className="blame-legend">
      <div className="blame-scale" aria-hidden="true">
        <span>Older</span>
        {heatStops.map((stop) => (
          <i key={stop} className="blame-swatch" data-heat={stop} />
        ))}
        <span>Newer</span>
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
  return author.kind === "human" ? `human:${author.user_id}` : `robot:${author.id}`;
}

function BlameRail({
  railRef,
  file,
  buffer,
  commits,
}: {
  railRef: Ref<HTMLDivElement>;
  file: LoadedFile;
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

function BlameView({ file, buffer, commits }: { file: LoadedFile; buffer: string; commits: MemoryCommit[] }) {
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
                <AuthorMark author={group.author} sessionId={commit?.session_id} />
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
