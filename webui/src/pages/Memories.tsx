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

export default function Memories() {
  const session = useSession();
  const [params, setParams] = useSearchParams();
  const [store, setStore] = useState<MemoryStore | null>(null);
  const [status, setStatus] = useState("Loading…");
  const [narrow, setNarrow] = useState(false);
  const request = useRef(0);
  const filePath = params.get("file");
  const tabParam = params.get("tab");
  const tab: FileTab = tabParam === "code" || tabParam === "blame" ? "code" : "preview";
  const committed = params.get("view") === "committed";
  const file = pickFile(store, filePath);

  const load = useCallback(async () => {
    const id = ++request.current;
    setStatus("Loading…");
    try {
      const response = await fetch("/api/memories", { headers: session.headers() });
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
      setStore((await response.json()) as MemoryStore);
      setStatus("");
    } catch (error) {
      if (id !== request.current) {
        return;
      }
      setStore(null);
      setStatus(error instanceof Error ? `Could not load memories: ${error.message}` : "Could not load memories.");
    }
  }, [session]);

  useEffect(() => {
    void load();
  }, [load]);

  function openFile(path: string): void {
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

  async function save(content: string, message: string, nextPath: string): Promise<void> {
    if (!store || !file) {
      return;
    }
    const next = applySave(store, file.path, content, message, editorAuthor(session.me), nextPath);
    if (next === store) {
      return;
    }
    const id = ++request.current;
    try {
      const response = await fetch("/api/memories", {
        method: "PUT",
        headers: { "content-type": "application/json", ...session.headers() },
        body: JSON.stringify(next),
      });
      if (id !== request.current) {
        return;
      }
      if (response.status === 401) {
        session.unauthorized();
        return;
      }
      if (!response.ok) {
        throw new Error(`${response.status} ${response.statusText}`);
      }
      setStore((await response.json()) as MemoryStore);
      setStatus("");
      if (nextPath !== file.path) {
        setParams((current) => {
          const params = new URLSearchParams(current);
          params.set("file", nextPath);
          return params;
        });
      }
    } catch (error) {
      if (id !== request.current) {
        return;
      }
      setStatus(error instanceof Error ? `Could not save the memory: ${error.message}` : "Could not save the memory.");
    }
  }

  return (
    <Page
      title="Memories"
      fill
      actions={
        <div className="memory-switch" role="group" aria-label="Memory views">
          <button type="button" aria-pressed={!committed} onClick={showFiles}>
            Files
          </button>
          <button type="button" aria-pressed={committed} onClick={showCommitted}>
            Memory Commit History
          </button>
        </div>
      }
    >
      {status ? (
        <p className="status" role="status">
          {status}
        </p>
      ) : null}
      {store && committed ? (
        <div className="memory-body">
          <CommitLog commits={store.commits} onOpen={openFile} />
        </div>
      ) : null}
      {store && !committed && file ? (
        <div className="memory-body memory-workspace">
          <FileTree
            files={store.files}
            activePath={file.path}
            narrow={narrow}
            onToggleNarrow={() => setNarrow((value) => !value)}
            onOpen={openFile}
          />
          <FileView
            key={file.path}
            file={file}
            commits={store.commits}
            paths={store.files.map((item) => item.path)}
            tab={tab}
            onTab={showTab}
            onSave={(content, message, nextPath) => void save(content, message, nextPath)}
          />
        </div>
      ) : null}
    </Page>
  );
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
