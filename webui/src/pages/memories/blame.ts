import type { BlameEntry, MemoryAuthor, MemoryStore } from "./types";

export type ShownBlame =
  | { kind: "saved"; entry: BlameEntry; showMark: boolean }
  | { kind: "unsaved"; showMark: boolean };

export type BlameGroup = {
  unsaved: boolean;
  commitId: string | null;
  author: MemoryAuthor | null;
  lines: { number: number; text: string }[];
};

export function blameGroups(saved: BlameEntry[], savedContent: string, buffer: string): BlameGroup[] {
  const rows = shownBlame(saved, savedContent, buffer);
  const lines = buffer.split("\n");
  const groups: BlameGroup[] = [];
  rows.forEach((row, index) => {
    const line = { number: index + 1, text: lines[index] ?? "" };
    const previous = groups[groups.length - 1];
    const same =
      previous !== undefined &&
      (row.kind === "unsaved"
        ? previous.unsaved
        : !previous.unsaved && previous.commitId === row.entry.commit_id);
    if (same && previous) {
      previous.lines.push(line);
      return;
    }
    groups.push({
      unsaved: row.kind === "unsaved",
      commitId: row.kind === "saved" ? row.entry.commit_id : null,
      author: row.kind === "saved" ? row.entry.author : null,
      lines: [line],
    });
  });
  return groups;
}

export function shownBlame(saved: BlameEntry[], savedContent: string, buffer: string): ShownBlame[] {
  const oldLines = savedContent.split("\n");
  const newLines = buffer.split("\n");
  const match = matchingOldIndexes(oldLines, newLines);
  const rows: ShownBlame[] = newLines.map((line, index) => {
    const oldIndex = match[index];
    if (oldIndex >= 0 && oldLines[oldIndex] === line && saved[oldIndex]) {
      return { kind: "saved", entry: saved[oldIndex], showMark: false };
    }
    return { kind: "unsaved", showMark: false };
  });
  let previous = "";
  for (const row of rows) {
    const key = row.kind === "unsaved" ? "unsaved" : row.entry.commit_id;
    row.showMark = key !== previous;
    previous = key;
  }
  return rows;
}

export function reattribute(
  previous: BlameEntry[],
  previousContent: string,
  nextContent: string,
  author: MemoryAuthor,
  commitId: string,
): BlameEntry[] {
  const oldLines = previousContent.split("\n");
  const newLines = nextContent.split("\n");
  const match = matchingOldIndexes(oldLines, newLines);
  const fresh: BlameEntry = { author, commit_id: commitId };
  return newLines.map((line, index) => {
    const oldIndex = match[index];
    if (oldIndex >= 0 && oldLines[oldIndex] === line && previous[oldIndex]) {
      return previous[oldIndex];
    }
    return fresh;
  });
}

export function applySave(
  store: MemoryStore,
  path: string,
  content: string,
  message: string,
  author: MemoryAuthor,
  nextPath = path,
): MemoryStore {
  const file = store.files.find((item) => item.path === path);
  const trimmed = message.trim();
  const sameContent = file?.content === content;
  const samePath = nextPath === path;
  if (!file || trimmed.length === 0 || (sameContent && samePath)) {
    return store;
  }
  if (!samePath && (!isMemoryPath(nextPath) || store.files.some((item) => item.path === nextPath))) {
    return store;
  }
  const commitId = `c_${crypto.randomUUID().slice(0, 8)}`;
  const blame = sameContent ? file.blame : reattribute(file.blame, file.content, content, author, commitId);
  return {
    files: store.files.map((item) => (item.path === path ? { ...item, path: nextPath, content, blame } : item)),
    commits: [
      {
        id: commitId,
        at: new Date().toISOString(),
        author,
        message: trimmed,
        files: [nextPath],
      },
      ...store.commits.map((commit) => ({
        ...commit,
        files: commit.files.map((item) => (item === path ? nextPath : item)),
      })),
    ],
  };
}

function isMemoryPath(path: string): boolean {
  if (path.length === 0 || path.startsWith("/") || path.includes("\\")) {
    return false;
  }
  return path.split("/").every((part) => part.length > 0 && part !== "." && part !== "..");
}

function matchingOldIndexes(oldLines: string[], newLines: string[]): number[] {
  const oldCount = oldLines.length;
  const newCount = newLines.length;
  const lengths: number[][] = Array.from({ length: oldCount + 1 }, () => Array<number>(newCount + 1).fill(0));
  for (let oldIndex = 1; oldIndex <= oldCount; oldIndex += 1) {
    for (let newIndex = 1; newIndex <= newCount; newIndex += 1) {
      if (oldLines[oldIndex - 1] === newLines[newIndex - 1]) {
        lengths[oldIndex][newIndex] = lengths[oldIndex - 1][newIndex - 1] + 1;
      } else {
        lengths[oldIndex][newIndex] = Math.max(lengths[oldIndex - 1][newIndex], lengths[oldIndex][newIndex - 1]);
      }
    }
  }
  const match = Array<number>(newCount).fill(-1);
  let oldIndex = oldCount;
  let newIndex = newCount;
  while (oldIndex > 0 && newIndex > 0) {
    if (oldLines[oldIndex - 1] === newLines[newIndex - 1]) {
      match[newIndex - 1] = oldIndex - 1;
      oldIndex -= 1;
      newIndex -= 1;
    } else if (lengths[oldIndex - 1][newIndex] >= lengths[oldIndex][newIndex - 1]) {
      oldIndex -= 1;
    } else {
      newIndex -= 1;
    }
  }
  return match;
}
