import type { MemoryStore } from "./types";

export type MemoryChange =
  | { op: "write"; path: string; content: string }
  | { op: "delete"; path: string }
  | { op: "rename"; from: string; to: string };

export function changesBetween(
  previous: MemoryStore,
  next: MemoryStore,
): { message: string; changes: MemoryChange[] } | null {
  const message =
    next.commits[0] && next.commits[0].id !== previous.commits[0]?.id ? next.commits[0].message.trim() : "";
  const before = virtualFiles(previous);
  const after = virtualFiles(next);
  const removed = [...before.keys()].filter((path) => !after.has(path));
  const added = [...after.keys()].filter((path) => !before.has(path));
  const used = new Set<string>();
  const changes: MemoryChange[] = [];
  for (const from of removed) {
    const content = before.get(from) ?? "";
    const match = added.find((to) => !used.has(to) && after.get(to) === content);
    if (match) {
      used.add(match);
      changes.push({ op: "rename", from, to: match });
    } else {
      changes.push({ op: "delete", path: from });
    }
  }
  for (const path of added) {
    if (!used.has(path)) {
      changes.push(write(path, after.get(path) ?? ""));
    }
  }
  for (const path of before.keys()) {
    if (after.has(path) && before.get(path) !== after.get(path)) {
      changes.push(write(path, after.get(path) ?? ""));
    }
  }
  if (changes.length === 0) {
    return null;
  }
  return { message, changes };
}

function virtualFiles(store: MemoryStore): Map<string, string> {
  const files = new Map<string, string>();
  for (const file of store.files) {
    files.set(file.path, file.content ?? `\u0000blob:${file.blob}`);
  }
  for (const directory of store.directories ?? []) {
    files.set(`${directory}/.keep`, "");
  }
  return files;
}

function write(path: string, content: string): MemoryChange {
  if (content.startsWith("\u0000blob:")) {
    throw new Error(`Cannot write ${path}: its text was never loaded.`);
  }
  return { op: "write", path, content };
}
