import { useCallback, useEffect, useRef, useState } from "react";
import { useSearchParams } from "react-router";
import type { Me } from "../ee/workos";
import { Page } from "../page";
import { useSession } from "../session";
import { applySave } from "./memories/blame";
import { CommitLog } from "./memories/CommitLog";
import { FileTree } from "./memories/FileTree";
import { FileView, type FileTab } from "./memories/FileView";
import type { MemoryAuthor, MemoryStore } from "./memories/types";
import { applyCreateFile, applyCreateFolder, applyMove, applyRenameFolder, planMove, type PathIssue } from "./memories/folders";
import { applyUpload, prepareUploads } from "./memories/upload";

type MemoryScope = "team" | "user";

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
  const request = useRef(0);
  const seenScope = useRef<MemoryScope>("team");
  const filePath = params.get("file");
  const tabParam = params.get("tab");
  const tab: FileTab = tabParam === "code" || tabParam === "blame" ? "code" : "preview";
  const committed = params.get("view") === "committed";
  const scope: MemoryScope = params.get("scope") === "user" ? "user" : "team";
  const file = pickFile(store, filePath);

  const load = useCallback(async () => {
    const id = ++request.current;
    setStatus("Loading…");
    try {
      const response = await fetch(memoriesUrl(scope), { headers: session.headers() });
      if (id !== request.current) {
        return;
      }
      if (response.status === 401) {
        session.unauthorized();
        return;
      }
      if (response.status === 402) {
        setStore(null);
        setStatus("This team's trial has ended.");
        return;
      }
      if (!response.ok) {
        throw new Error(`${response.status} ${response.statusText}`);
      }
      setStore(readStore((await response.json()) as MemoryStore));
      setStatus("");
    } catch (error) {
      if (id !== request.current) {
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

  async function persist(next: MemoryStore, openPath?: string): Promise<string | null> {
    const id = ++request.current;
    try {
      const response = await fetch(memoriesUrl(scope), {
        method: "PUT",
        headers: { "content-type": "application/json", ...session.headers() },
        body: JSON.stringify(next),
      });
      if (id !== request.current) {
        return null;
      }
      if (response.status === 401) {
        session.unauthorized();
        return null;
      }
      if (!response.ok) {
        throw new Error(`${response.status} ${response.statusText}`);
      }
      setStore(readStore((await response.json()) as MemoryStore));
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
    const problem = await persist(next, path);
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
    const problem = await persist(next, nextPath !== file.path ? nextPath : undefined);
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
    const problem = await persist(next, prepared.drafts[0]?.path);
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
    const problem = await persist(next);
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
    const problem = await persist(next, openPath);
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
    const problem = await persist(next, openPath);
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
          {store && committed ? <CommitLog commits={store.commits} onOpen={openFile} /> : null}
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
                  file={{ path: draftDirectory, content: "", blame: [] }}
                  createIn={draftDirectory}
                  commits={store.commits}
                  paths={store.files.map((item) => item.path)}
                  directories={store.directories}
                  tab={tab}
                  onTab={showTab}
                  onEditingChange={setEditing}
                  onDiscard={() => setDraftDirectory(null)}
                  onSave={(content, message, nextPath) => void createFile(content, message, nextPath)}
                />
              ) : file ? (
                <FileView
                  key={`${scope}:${file.path}`}
                  file={file}
                  commits={store.commits}
                  paths={store.files.map((item) => item.path)}
                  directories={store.directories}
                  tab={tab}
                  onTab={showTab}
                  onEditingChange={setEditing}
                  onSave={(content, message, nextPath) => void save(content, message, nextPath)}
                />
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

function memoriesUrl(scope: MemoryScope): string {
  return scope === "user" ? "/api/memories?scope=user" : "/api/memories";
}

function pickFile(store: MemoryStore | null, requested: string | null) {
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
