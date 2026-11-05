import type { Me } from "../../ee/workos";
import type { BlameEntry, MemoryCommit, MemoryStore, MemoryTree } from "./types";

export type MemoryScope = "team" | "user";

const STORAGE_PREFIX = "cadri.memories.v1.";
const FRESH_MS = 10_000;
const PREFETCH_ALL_LIMIT = 300;
const PREFETCH_PARALLEL = 6;

type Stored = { tip: string | null; files: { path: string; blob: string }[]; directories: string[] };

const trees = new Map<MemoryScope, MemoryStore>();
const fetchedAt = new Map<MemoryScope, number>();
const inflight = new Map<MemoryScope, Promise<MemoryStore>>();
const texts = new Map<string, string>();
const blobsInflight = new Map<string, Promise<string>>();
let identity = "local.local";

export function setMemoryIdentity(me: Me | null): void {
  identity = me ? `${me.org_id}.${me.user_id}` : "local.local";
}

function storageKey(scope: MemoryScope): string {
  return `${STORAGE_PREFIX}${identity}.${scope}`;
}

function readStored(scope: MemoryScope): MemoryStore | null {
  try {
    const raw = localStorage.getItem(storageKey(scope));
    if (!raw) {
      return null;
    }
    const stored = JSON.parse(raw) as Stored;
    if (!Array.isArray(stored.files) || !Array.isArray(stored.directories)) {
      return null;
    }
    return {
      tip: stored.tip ?? null,
      files: stored.files.map((file) => ({
        path: file.path,
        blob: file.blob,
        content: texts.get(file.blob) ?? null,
        blame: [],
      })),
      directories: stored.directories,
      commits: [],
    };
  } catch {
    return null;
  }
}

function writeStored(scope: MemoryScope, store: MemoryStore): void {
  if (store.files.some((file) => file.blob === "")) {
    return;
  }
  const stored: Stored = {
    tip: store.tip,
    files: store.files.map((file) => ({ path: file.path, blob: file.blob })),
    directories: store.directories,
  };
  try {
    localStorage.setItem(storageKey(scope), JSON.stringify(stored));
  } catch {
    /* Storage may be full or disabled; the page still works without it. */
  }
}

/** Drops everything remembered about books: call before leaving the session. */
export function clearMemoryCaches(): void {
  trees.clear();
  fetchedAt.clear();
  texts.clear();
  try {
    const doomed: string[] = [];
    for (let index = 0; index < localStorage.length; index += 1) {
      const key = localStorage.key(index);
      if (key?.startsWith(STORAGE_PREFIX)) {
        doomed.push(key);
      }
    }
    doomed.forEach((key) => localStorage.removeItem(key));
  } catch {
    /* ignore */
  }
}

export function peekMemoryBook(scope: MemoryScope): MemoryStore | null {
  return trees.get(scope) ?? readStored(scope);
}

export function rememberMemoryBook(scope: MemoryScope, store: MemoryStore): void {
  trees.set(scope, store);
  fetchedAt.set(scope, Date.now());
  writeStored(scope, store);
}

export function cachedText(blob: string): string | null {
  return texts.get(blob) ?? null;
}

export function rememberText(blob: string, text: string): void {
  if (blob) {
    texts.set(blob, text);
  }
}

export function primeMemoryBook(scope: MemoryScope, headers: Record<string, string>): void {
  void loadMemoryBook(scope, headers, null).catch(() => {
    /* The page surfaces the error when it joins this request. */
  });
}

/**
 * One in-flight GET per book. A response fetched moments ago is reused. Otherwise the last known
 * tree (memory or localStorage) is revalidated with If-None-Match.
 */
export function loadMemoryBook(
  scope: MemoryScope,
  headers: Record<string, string>,
  open: string | null,
): Promise<MemoryStore> {
  const pending = inflight.get(scope);
  if (pending) {
    return pending;
  }
  const cached = trees.get(scope);
  if (cached && Date.now() - (fetchedAt.get(scope) ?? 0) < FRESH_MS) {
    return Promise.resolve(cached);
  }
  const promise = fetchTree(scope, headers, open, cached ?? readStored(scope))
    .then((store) => {
      rememberMemoryBook(scope, store);
      return store;
    })
    .finally(() => {
      inflight.delete(scope);
    });
  inflight.set(scope, promise);
  return promise;
}

export function loadBlob(scope: MemoryScope, sha: string, headers: Record<string, string>): Promise<string> {
  const known = texts.get(sha);
  if (known !== undefined) {
    return Promise.resolve(known);
  }
  const key = `${scope}:${sha}`;
  const pending = blobsInflight.get(key);
  if (pending) {
    return pending;
  }
  const promise = fetch(scopedPath(`/api/memories/blob/${sha}`, scope), { headers })
    .then(async (response) => {
      if (!response.ok) {
        throw new Error(`${response.status} ${response.statusText}`);
      }
      const text = await response.text();
      texts.set(sha, text);
      return text;
    })
    .finally(() => {
      blobsInflight.delete(key);
    });
  blobsInflight.set(key, promise);
  return promise;
}

/** Fetches file text in the background so later clicks need no request. */
export function prefetchBlobs(
  scope: MemoryScope,
  store: MemoryStore,
  openPath: string | null,
  headers: Record<string, string>,
): () => void {
  let files = store.files;
  if (files.length > PREFETCH_ALL_LIMIT) {
    const directory = openPath && openPath.includes("/") ? openPath.slice(0, openPath.lastIndexOf("/") + 1) : "";
    files = files.filter((file) => file.path.startsWith(directory) && !file.path.slice(directory.length).includes("/"));
  }
  const queue = files.map((file) => file.blob).filter((blob) => blob !== "" && !texts.has(blob));
  let cancelled = false;
  const worker = async (): Promise<void> => {
    for (let sha = queue.shift(); sha !== undefined && !cancelled; sha = queue.shift()) {
      await loadBlob(scope, sha, headers).catch(() => undefined);
    }
  };
  const start = (): void => {
    for (let index = 0; index < PREFETCH_PARALLEL; index += 1) {
      void worker();
    }
  };
  const idle = window.requestIdleCallback as ((callback: () => void) => number) | undefined;
  if (idle) {
    const handle = idle.call(window, start);
    return () => {
      cancelled = true;
      window.cancelIdleCallback?.(handle);
    };
  }
  const handle = window.setTimeout(start, 50);
  return () => {
    cancelled = true;
    window.clearTimeout(handle);
  };
}

/** Builds a store from a tree response, reusing text and blame the page already has. */
export function storeFromTree(tree: MemoryTree, previous: MemoryStore | null): MemoryStore {
  if (tree.open) {
    texts.set(tree.open.blob, tree.open.content);
  }
  const before = new Map(previous?.files.map((file) => [file.path, file]));
  return {
    tip: tree.tip,
    files: tree.files.map((entry) => {
      const old = before.get(entry.path);
      return {
        path: entry.path,
        blob: entry.blob,
        content: texts.get(entry.blob) ?? null,
        blame: old && old.blob === entry.blob ? old.blame : [],
      };
    }),
    directories: tree.directories ?? [],
    commits: previous && previous.tip === tree.tip ? previous.commits : [],
  };
}

export async function loadMemoryLog(
  scope: MemoryScope,
  headers: Record<string, string>,
): Promise<MemoryCommit[]> {
  const response = await fetch(scopedPath("/api/memories/log", scope), { headers });
  if (!response.ok) {
    throw new Error(`${response.status} ${response.statusText}`);
  }
  const body = (await response.json()) as { commits?: MemoryCommit[] };
  return body.commits ?? [];
}

export async function loadMemoryBlame(
  scope: MemoryScope,
  path: string,
  headers: Record<string, string>,
): Promise<BlameEntry[]> {
  const params = new URLSearchParams();
  if (scope === "user") {
    params.set("scope", "user");
  }
  params.set("path", path);
  const response = await fetch(`/api/memories/blame?${params}`, { headers });
  if (!response.ok) {
    throw new Error(`${response.status} ${response.statusText}`);
  }
  const body = (await response.json()) as { blame?: BlameEntry[] };
  return body.blame ?? [];
}

async function fetchTree(
  scope: MemoryScope,
  headers: Record<string, string>,
  open: string | null,
  known: MemoryStore | null,
): Promise<MemoryStore> {
  const params = new URLSearchParams();
  if (scope === "user") {
    params.set("scope", "user");
  }
  if (open) {
    params.set("open", open);
  }
  const query = params.size > 0 ? `?${params}` : "";
  const sent = known ? { ...headers, "if-none-match": `"${known.tip ?? "empty"}"` } : headers;
  const response = await fetch(`/api/memories/tree${query}`, { headers: sent });
  if (response.status === 304 && known) {
    return known;
  }
  if (response.status === 401) {
    throw new LoadError(401, "unauthorized");
  }
  if (response.status === 402) {
    throw new LoadError(402, "trial");
  }
  if (!response.ok) {
    throw new LoadError(response.status, `${response.status} ${response.statusText}`);
  }
  return storeFromTree((await response.json()) as MemoryTree, known);
}

export class LoadError extends Error {
  status: number;

  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

function scopedPath(path: string, scope: MemoryScope): string {
  return scope === "user" ? `${path}?scope=user` : path;
}
