import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useSearchParams } from "react-router";
import type { Me } from "../ee/workos";
import { Page } from "../page";
import { useSession } from "../session";
import { applySave } from "./memories/blame";
import {
  cachedText,
  LoadError,
  loadBlob,
  loadMemoryBlame,
  loadMemoryBook,
  loadMemoryLog,
  peekMemoryBook,
  prefetchBlobs,
  rememberMemoryBook,
  rememberText,
  setMemoryIdentity,
  storeFromTree,
} from "./memories/books";
import { changesBetween, type MemoryChange } from "./memories/changes";
import { CommitLog } from "./memories/CommitLog";
import { FileTree } from "./memories/FileTree";
import { FileView, type FileTab } from "./memories/FileView";
import type { LoadedFile, MemoryAuthor, MemoryFile, MemoryStore, MemoryTree } from "./memories/types";
import { applyCreateFile, applyCreateFolder, applyMove, applyRenameFolder, planMove, type PathIssue } from "./memories/folders";
import { applyUpload, prepareUploads } from "./memories/upload";
import { EmptyBook, type EmptyPhase } from "./memories/EmptyBook";
import type { SessionRow } from "./sessions/types";

type MemoryScope = "team" | "user";

/** A book stays in "memories are being populated" until the oldest relevant session is this old. */
const SEEDING_WINDOW_MS = 60 * 60 * 1000;
const EMPTY_BOOK_POLL_MS = 10_000;

type Notice = {
  title: string;
  text?: string;
  name?: string;
  directory?: string;
};

export default function Memories() {
  const session = useSession();
  const [params, setParams] = useSearchParams();
  const [store, setStore] = useState<MemoryStore | null>(null);
  const [status, setStatus] = useState("Loading…");
  const [narrow, setNarrow] = useState(false);
  const [editing, setEditing] = useState(false);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [draftDirectory, setDraftDirectory] = useState<string | null>(null);
  const [sessions, setSessions] = useState<SessionRow[] | null>(null);
  const request = useRef(0);
  const blamedPaths = useRef(new Set<string>());
  const seenScope = useRef<MemoryScope>("team");
  const filePath = params.get("file");
  const tabParam = params.get("tab");
  const tab: FileTab = tabParam === "code" || tabParam === "blame" ? "code" : "preview";
  const committed = params.get("view") === "committed";
  const scope: MemoryScope = params.get("scope") === "user" ? "user" : "team";
  const picked = pickFile(store, filePath);
  const file: LoadedFile | null = picked && picked.content !== null ? { ...picked, content: picked.content } : null;
  const pending = picked && picked.content === null ? picked : null;
  const [fileError, setFileError] = useState<string | null>(null);
  const filePathRef = useRef<string | null>(null);
  filePathRef.current = picked?.path ?? filePath;

  const load = useCallback(async () => {
    const id = ++request.current;
    setMemoryIdentity(session.me);
    const known = peekMemoryBook(scope);
    if (known) {
      setStore(readStore(known));
      setStatus("");
    } else {
      setStatus("Loading…");
    }
    try {
      const store = await loadMemoryBook(scope, session.headers(), filePathRef.current);
      if (id !== request.current) {
        return;
      }
      if (store.tip !== known?.tip) {
        blamedPaths.current.clear();
        historyScope.current = null;
      }
      setStore(readStore(store));
      setStatus("");
    } catch (error) {
      if (id !== request.current) {
        return;
      }
      if (known) {
        return;
      }
      if (error instanceof LoadError && error.status === 401) {
        session.unauthorized();
        return;
      }
      if (error instanceof LoadError && error.status === 402) {
        setStore(null);
        setStatus("This team's trial has ended.");
        return;
      }
      setStore(null);
      setStatus(error instanceof Error ? `Could not load memories: ${error.message}` : "Could not load memories.");
    }
  }, [session, scope]);

  useEffect(() => {
    if (seenScope.current !== scope) {
      seenScope.current = scope;
      setStore(null);
      setEditing(false);
      setDraftDirectory(null);
    }
    void load();
  }, [load, scope]);

  const awaitingFirstMemory = store !== null && store.tip === null;
  useEffect(() => {
    if (!awaitingFirstMemory) {
      return;
    }
    let cancel = false;

    const loadSessions = async () => {
      try {
        const response = await fetch("/api/sessions", { headers: session.headers() });
        if (cancel) {
          return;
        }
        if (response.status === 401) {
          session.unauthorized();
          return;
        }
        if (!response.ok) {
          return;
        }
        const rows = (await response.json()) as SessionRow[];
        if (!cancel && Array.isArray(rows)) {
          setSessions(rows);
        }
      } catch {
        /* The next poll retries. */
      }
    };

    const watchBook = async () => {
      try {
        const response = await fetch(treeUrl(scope), {
          headers: { ...session.headers(), "if-none-match": '"empty"' },
        });
        if (cancel || response.status === 304 || !response.ok) {
          return;
        }
        const tree = (await response.json()) as MemoryTree;
        if (cancel || !tree.tip) {
          return;
        }
        const next = readStore(storeFromTree(tree, null));
        rememberMemoryBook(scope, next);
        setStore(next);
      } catch {
        /* The next poll retries. */
      }
    };

    void loadSessions();
    const id = window.setInterval(() => {
      void loadSessions();
      void watchBook();
    }, EMPTY_BOOK_POLL_MS);
    return () => {
      cancel = true;
      window.clearInterval(id);
    };
  }, [awaitingFirstMemory, scope, session]);

  const ensureBlame = useCallback(
    (path: string) => {
      const key = `${scope}:${path}`;
      if (blamedPaths.current.has(key)) {
        return;
      }
      blamedPaths.current.add(key);
      void loadMemoryBlame(scope, path, session.headers())
        .then((blame) => {
          setStore((current) => {
            if (!current) {
              return current;
            }
            return {
              ...current,
              files: current.files.map((item) => (item.path === path ? { ...item, blame } : item)),
            };
          });
        })
        .catch(() => {
          blamedPaths.current.delete(key);
        });
    },
    [scope, session],
  );

  useLayoutEffect(() => {
    if (!pending) {
      return;
    }
    const text = cachedText(pending.blob);
    if (text !== null) {
      setStore((current) => withText(current, pending.path, pending.blob, text));
    }
  }, [pending]);

  useEffect(() => {
    if (!pending || cachedText(pending.blob) !== null) {
      return;
    }
    const { path, blob } = pending;
    let cancel = false;
    setFileError(null);
    loadBlob(scope, blob, session.headers())
      .then((text) => {
        if (!cancel) {
          setStore((current) => withText(current, path, blob, text));
        }
      })
      .catch((error: unknown) => {
        if (!cancel) {
          setFileError(error instanceof Error ? `Could not load ${path}: ${error.message}` : `Could not load ${path}.`);
        }
      });
    return () => {
      cancel = true;
    };
  }, [pending, scope, session]);

  const prefetchKey = store ? `${scope}:${store.tip ?? ""}` : null;
  useEffect(() => {
    if (!store || prefetchKey === null) {
      return;
    }
    return prefetchBlobs(scope, store, filePathRef.current, session.headers());
    // Runs once per tip: later store edits do not change which blobs exist.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [prefetchKey]);

  const historyScope = useRef<MemoryScope | null>(null);
  const ensureHistory = useCallback(() => {
    if (historyScope.current === scope) {
      return;
    }
    historyScope.current = scope;
    void loadMemoryLog(scope, session.headers())
      .then((commits) => {
        setStore((current) => (current ? { ...current, commits } : current));
      })
      .catch(() => {
        historyScope.current = null;
      });
  }, [scope, session]);

  useEffect(() => {
    if (committed) {
      ensureHistory();
    }
  }, [committed, ensureHistory]);

  useEffect(() => {
    if (!store || committed) {
      return;
    }
    if (filePath && store.files.some((item) => item.path === filePath)) {
      return;
    }
    const fallback = store.files.find((item) => item.path === "README.md") ?? store.files[0];
    if (!fallback && !filePath) {
      return;
    }
    setParams(
      (current) => {
        const next = new URLSearchParams(current);
        if (fallback) {
          next.set("file", fallback.path);
        } else {
          next.delete("file");
        }
        return next;
      },
      { replace: true },
    );
  }, [store, filePath, committed, setParams]);

  function openFile(path: string): void {
    setDraftDirectory(null);
    setParams((current) => {
      const next = new URLSearchParams(current);
      next.delete("view");
      next.set("file", path);
      return next;
    });
  }

  function showFiles(): void {
    setParams((current) => {
      const next = new URLSearchParams(current);
      next.delete("view");
      return next;
    });
  }

  function showCommitted(): void {
    setParams((current) => {
      const next = new URLSearchParams(current);
      next.set("view", "committed");
      return next;
    });
  }

  function showScope(nextScope: MemoryScope): void {
    setParams((current) => {
      const next = new URLSearchParams(current);
      if (nextScope === "team") {
        next.delete("scope");
      } else {
        next.set("scope", "user");
      }
      return next;
    });
  }

  function showTab(nextTab: FileTab): void {
    setParams((current) => {
      const next = new URLSearchParams(current);
      next.delete("view");
      if (!next.get("file") && file) {
        next.set("file", file.path);
      }
      if (nextTab === "preview") {
        next.delete("tab");
      } else {
        next.set("tab", nextTab);
      }
      return next;
    });
  }

  async function persist(previous: MemoryStore, next_: MemoryStore, openPath?: string): Promise<string | null> {
    const diff = changesBetween(previous, next_);
    if (!diff) {
      return null;
    }
    const id = ++request.current;
    try {
      const response = await fetch(commitsUrl(scope), {
        method: "POST",
        headers: { "content-type": "application/json", ...session.headers() },
        body: JSON.stringify(diff),
      });
      if (id !== request.current) {
        return null;
      }
      if (response.status === 401) {
        session.unauthorized();
        return null;
      }
      if (!response.ok) {
        const payload = (await response.json().catch(() => null)) as { error?: string } | null;
        throw new Error(payload?.error ?? `${response.status} ${response.statusText}`);
      }
      const written = new Map(
        diff.changes.flatMap((change: MemoryChange) => (change.op === "write" ? [[change.path, change.content] as const] : [])),
      );
      const next = mergeSaved((await response.json()) as MemoryTree, next_, written);
      blamedPaths.current.clear();
      historyScope.current = null;
      rememberMemoryBook(scope, next);
      setStore(next);
      setStatus("");
      if (openPath) {
        setParams((current) => {
          const params = new URLSearchParams(current);
          params.delete("view");
          params.set("file", openPath);
          return params;
        });
      }
      return null;
    } catch (error) {
      if (id !== request.current) {
        return null;
      }
      return error instanceof Error ? `Could not save the memory: ${error.message}` : "Could not save the memory.";
    }
  }

  function beginFile(directory: string): void {
    setDraftDirectory(directory);
    setParams((current) => {
      const next = new URLSearchParams(current);
      next.delete("view");
      next.set("tab", "code");
      return next;
    });
  }

  async function createFile(content: string, message: string, path: string): Promise<void> {
    if (!store) {
      return;
    }
    const next = applyCreateFile(store, path, content, message, editorAuthor(session.me));
    if (next === store) {
      setNotice({ title: "Could not create the file", text: "That file could not be saved." });
      return;
    }
    const problem = await persist(store, next, path);
    if (problem) {
      setNotice({ title: "Could not create the file", text: problem });
      return;
    }
    setDraftDirectory(null);
  }

  async function save(content: string, message: string, nextPath: string): Promise<void> {
    if (!store || !file) {
      return;
    }
    const next = applySave(store, file.path, content, message, editorAuthor(session.me), nextPath);
    if (next === store) {
      return;
    }
    const problem = await persist(store, next, nextPath !== file.path ? nextPath : undefined);
    if (problem) {
      setStatus(problem);
    }
  }

  async function upload(files: File[]): Promise<void> {
    if (!store || editing || files.length === 0) {
      return;
    }
    const started = request.current;
    const prepared = await prepareUploads(
      store.files.map((item) => item.path),
      files,
    );
    if (started !== request.current || !store) {
      return;
    }
    if ("error" in prepared) {
      setNotice({ title: "Upload failed", text: prepared.error });
      return;
    }
    if ("collision" in prepared) {
      setNotice({
        title: "Upload failed",
        name: prepared.collision.name,
        directory: prepared.collision.directory,
      });
      return;
    }
    const next = applyUpload(store, prepared.drafts, prepared.message, editorAuthor(session.me));
    if (next === store) {
      return;
    }
    const problem = await persist(store, next, prepared.drafts[0]?.path);
    if (problem) {
      setNotice({ title: "Upload failed", text: problem });
    }
  }

  async function createFolder(path: string): Promise<void> {
    if (!store) {
      return;
    }
    const next = applyCreateFolder(store, path, editorAuthor(session.me));
    if (next === store) {
      return;
    }
    const problem = await persist(store, next);
    if (problem) {
      setNotice({ title: "Could not create the folder", text: problem });
    }
  }

  async function renameFolder(from: string, to: string): Promise<void> {
    if (!store) {
      return;
    }
    const next = applyRenameFolder(store, from, to, editorAuthor(session.me));
    if (next === store) {
      return;
    }
    const openPath =
      file && (file.path === from || file.path.startsWith(`${from}/`))
        ? `${to}${file.path.slice(from.length)}`
        : undefined;
    const problem = await persist(store, next, openPath);
    if (problem) {
      setNotice({ title: "Could not rename the folder", text: problem });
    }
  }

  async function moveItem(from: string, parent: string): Promise<void> {
    if (!store) {
      return;
    }
    const planned = planMove(store, from, parent);
    if ("noop" in planned) {
      return;
    }
    if (!("path" in planned)) {
      showIssue("Could not move", planned);
      return;
    }
    const next = applyMove(store, from, planned.path, editorAuthor(session.me));
    if (next === store) {
      return;
    }
    const openPath =
      file && (file.path === from || file.path.startsWith(`${from}/`))
        ? `${planned.path}${file.path.slice(from.length)}`
        : undefined;
    const problem = await persist(store, next, openPath);
    if (problem) {
      setNotice({ title: "Could not move", text: problem });
    }
  }

  function showIssue(title: string, issue: PathIssue): void {
    setNotice({ title, text: issue.text, name: issue.name, directory: issue.directory });
  }

  const emptyCopy =
    scope === "user"
      ? "Your memories are empty. Upload a file to start."
      : "This repo is empty. Upload a file to start.";
  const phase = awaitingFirstMemory ? bookPhase(scope, sessions, session.me) : null;

  return (
    <Page
      title="Memories"
      fill
      actions={
        <div className="memory-switch" role="group" aria-label="Memory scope">
          <button type="button" aria-pressed={scope === "team"} onClick={() => showScope("team")}>
            Team
          </button>
          <button type="button" aria-pressed={scope === "user"} onClick={() => showScope("user")}>
            Me
          </button>
        </div>
      }
    >
      {status ? (
        <p className="status" role="status">
          {status}
        </p>
      ) : null}
      {store || status === "Loading…" ? (
        <div className="memory-body">
          <div className="memory-pane-head">
            <div className="memory-switch" role="group" aria-label="Memory views">
              <button type="button" aria-pressed={!committed} onClick={showFiles}>
                Files
              </button>
              <button type="button" aria-pressed={committed} onClick={showCommitted}>
                Memory Commit History
              </button>
            </div>
          </div>
          {store && committed ? <CommitLog commits={store.commits} scope={scope} onOpen={openFile} /> : null}
          {store && !committed ? (
            <div className="memory-workspace">
              <FileTree
                store={store}
                activePath={file?.path ?? null}
                narrow={narrow}
                changesDisabled={editing || draftDirectory !== null}
                onToggleNarrow={() => setNarrow((value) => !value)}
                onOpen={openFile}
                onUpload={(files) => void upload(files)}
                onCreateFolder={(path) => void createFolder(path)}
                onCreateFile={beginFile}
                onRenameFolder={(from, to) => void renameFolder(from, to)}
                onMove={(from, parent) => void moveItem(from, parent)}
                onIssue={showIssue}
              />
              {draftDirectory !== null ? (
                <FileView
                  key={`draft:${scope}:${draftDirectory}`}
                  file={{ path: draftDirectory, blob: "", content: "", blame: [] }}
                  createIn={draftDirectory}
                  scope={scope}
                  commits={store.commits}
                  paths={store.files.map((item) => item.path)}
                  directories={store.directories}
                  tab={tab}
                  onTab={showTab}
                  onEditingChange={setEditing}
                  onNeedHistory={ensureHistory}
                  onDiscard={() => setDraftDirectory(null)}
                  onSave={(content, message, nextPath) => void createFile(content, message, nextPath)}
                />
              ) : pending ? (
                <p className="status" role="status">
                  {fileError ?? "Loading…"}
                </p>
              ) : file ? (
                <FileView
                  key={`${scope}:${file.path}`}
                  file={file}
                  scope={scope}
                  commits={store.commits}
                  paths={store.files.map((item) => item.path)}
                  directories={store.directories}
                  tab={tab}
                  onTab={showTab}
                  onEditingChange={setEditing}
                  onNeedHistory={ensureHistory}
                  onNeedBlame={() => ensureBlame(file.path)}
                  onSave={(content, message, nextPath) => void save(content, message, nextPath)}
                />
              ) : awaitingFirstMemory ? (
                phase ? <EmptyBook scope={scope} phase={phase} /> : null
              ) : (
                <p className="memory-empty">{emptyCopy}</p>
              )}
            </div>
          ) : null}
        </div>
      ) : null}
      {notice ? <NoticeAlert notice={notice} onDismiss={() => setNotice(null)} /> : null}
    </Page>
  );
}

function commitsUrl(scope: MemoryScope): string {
  return scope === "user" ? "/api/memories/commits?view=tree&scope=user" : "/api/memories/commits?view=tree";
}

function treeUrl(scope: MemoryScope): string {
  return scope === "user" ? "/api/memories/tree?scope=user" : "/api/memories/tree";
}

/** What an empty book should tell the reader, from how far session upload has gotten. */
function bookPhase(scope: MemoryScope, sessions: SessionRow[] | null, me: Me | null): EmptyPhase | null {
  if (!sessions) {
    return null;
  }
  const relevant = scope === "user" && me ? sessions.filter((row) => row.user_id === me.user_id) : sessions;
  if (relevant.length === 0) {
    return "install";
  }
  const first = Math.min(...relevant.map((row) => Date.parse(row.created_at)));
  if (!Number.isFinite(first) || Date.now() - first >= SEEDING_WINDOW_MS) {
    return "empty";
  }
  return "seeding";
}

function withText(store: MemoryStore | null, path: string, blob: string, text: string): MemoryStore | null {
  if (!store) {
    return store;
  }
  return {
    ...store,
    files: store.files.map((item) => (item.path === path && item.blob === blob ? { ...item, content: text } : item)),
  };
}

/** Applies the server's tree after a save, keeping the text and blame the page already has. */
function mergeSaved(tree: MemoryTree, local: MemoryStore, written: Map<string, string>): MemoryStore {
  for (const entry of tree.files) {
    const sent = written.get(entry.path);
    if (sent !== undefined) {
      rememberText(entry.blob, sent);
    }
  }
  for (const file of local.files) {
    if (file.blob !== "" && file.content !== null) {
      rememberText(file.blob, file.content);
    }
  }
  const merged = storeFromTree(tree, null);
  const before = new Map(local.files.map((file) => [file.path, file]));
  return {
    ...merged,
    files: merged.files.map((file) => {
      const old = before.get(file.path);
      return old && old.blob === file.blob ? { ...file, blame: old.blame } : file;
    }),
    commits: local.commits,
  };
}

function pickFile(store: MemoryStore | null, requested: string | null): MemoryFile | null {
  if (!store) {
    return null;
  }
  const named = requested ? store.files.find((item) => item.path === requested) : undefined;
  if (named) {
    return named;
  }
  return store.files.find((item) => item.path === "README.md") ?? store.files[0] ?? null;
}

function editorAuthor(me: Me | null): MemoryAuthor {
  if (!me) {
    return { kind: "human", user_id: "local", label: "You" };
  }
  return { kind: "human", user_id: me.user_id, label: me.email ?? me.user_id };
}

function readStore(store: MemoryStore): MemoryStore {
  return { ...store, directories: store.directories ?? [] };
}

function NoticeAlert({ notice, onDismiss }: { notice: Notice; onDismiss: () => void }) {
  useEffect(() => {
    function onKey(event: KeyboardEvent): void {
      if (event.key === "Escape") {
        onDismiss();
      }
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onDismiss]);

  return (
    <div className="upload-alert" onClick={onDismiss}>
      <div
        className="panel"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="upload-alert-title"
        aria-describedby="upload-alert-detail"
        onClick={(event) => event.stopPropagation()}
      >
        <h2 id="upload-alert-title">{notice.title}</h2>
        <p className="hint" id="upload-alert-detail">
          {notice.name && notice.directory ? (
            <>
              <code>{notice.name}</code> already exists in <code>{notice.directory}</code> directory
            </>
          ) : (
            notice.text
          )}
        </p>
        <button className="btn" type="button" autoFocus onClick={onDismiss}>
          Dismiss
        </button>
      </div>
    </div>
  );
}
