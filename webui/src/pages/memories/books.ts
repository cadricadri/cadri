import type { BlameEntry, MemoryCommit, MemoryStore } from "./types";

type MemoryScope = "team" | "user";

const trees = new Map<MemoryScope, MemoryStore>();
const inflight = new Map<MemoryScope, Promise<MemoryStore>>();

export function peekMemoryBook(scope: MemoryScope): MemoryStore | null {
  return trees.get(scope) ?? null;
}

export function rememberMemoryBook(scope: MemoryScope, store: MemoryStore): void {
  trees.set(scope, store);
}

export function primeMemoryBook(scope: MemoryScope, headers: Record<string, string>): void {
  void loadMemoryBook(scope, headers).catch(() => {
    /* The page surfaces the error when it joins this request. */
  });
}

/** One in-flight GET per book. A finished warm response is reused instead of fetched again. */
export function loadMemoryBook(scope: MemoryScope, headers: Record<string, string>): Promise<MemoryStore> {
  const pending = inflight.get(scope);
  if (pending) {
    return pending;
  }
  const cached = trees.get(scope);
  if (cached) {
    return Promise.resolve(cached);
  }
  const promise = fetchTree(scope, headers)
    .then((store) => {
      trees.set(scope, store);
      return store;
    })
    .finally(() => {
      inflight.delete(scope);
    });
  inflight.set(scope, promise);
  return promise;
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

async function fetchTree(scope: MemoryScope, headers: Record<string, string>): Promise<MemoryStore> {
  const response = await fetch(scopedPath("/api/memories", scope), { headers });
  if (response.status === 401) {
    throw new LoadError(401, "unauthorized");
  }
  if (response.status === 402) {
    throw new LoadError(402, "trial");
  }
  if (!response.ok) {
    throw new LoadError(response.status, `${response.status} ${response.statusText}`);
  }
  return (await response.json()) as MemoryStore;
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
