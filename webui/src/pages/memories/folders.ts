import { isMemoryPath } from "./blame";
import type { MemoryAuthor, MemoryStore } from "./types";

export type PathIssue = {
  text: string;
  name?: string;
  directory?: string;
};

export function planCreate(
  store: MemoryStore,
  parent: string,
  rawName: string,
): { path: string } | PathIssue {
  const name = rawName.trim();
  if (!isMemoryPath(name) || name.includes("/")) {
    return { text: "Use a folder name without slashes." };
  }
  const path = parent ? `${parent}/${name}` : name;
  if (!isMemoryPath(path) || occupies(store, path)) {
    return { text: "", name, directory: parent ? `/${parent}` : "/" };
  }
  return { path };
}

export function planRename(store: MemoryStore, from: string, rawName: string): { path: string } | PathIssue {
  const name = rawName.trim();
  const parent = from.includes("/") ? from.slice(0, from.lastIndexOf("/")) : "";
  if (!isMemoryPath(name) || name.includes("/")) {
    return { text: "Use a folder name without slashes." };
  }
  const path = parent ? `${parent}/${name}` : name;
  if (path === from) {
    return { path };
  }
  if (path.startsWith(`${from}/`)) {
    return { text: "A folder cannot move inside itself." };
  }
  if (!isMemoryPath(path) || occupies(store, path)) {
    return { text: "", name, directory: parent ? `/${parent}` : "/" };
  }
  return { path };
}

export function applyCreateFile(
  store: MemoryStore,
  path: string,
  content: string,
  message: string,
  author: MemoryAuthor,
): MemoryStore {
  const trimmed = message.trim();
  if (!isMemoryPath(path) || trimmed.length === 0 || occupies(store, path)) {
    return store;
  }
  const commitId = `c_${crypto.randomUUID().slice(0, 8)}`;
  return {
    files: [
      ...store.files,
      {
        path,
        content,
        blame: content.split("\n").map(() => ({ author, commit_id: commitId })),
      },
    ],
    directories: store.directories,
    commits: [
      {
        id: commitId,
        at: new Date().toISOString(),
        author,
        message: trimmed,
        files: [path],
      },
      ...store.commits,
    ],
  };
}

export function applyCreateFolder(store: MemoryStore, path: string, author: MemoryAuthor): MemoryStore {
  if (!isMemoryPath(path) || occupies(store, path)) {
    return store;
  }
  const commitId = `c_${crypto.randomUUID().slice(0, 8)}`;
  return {
    files: store.files,
    directories: [...store.directories, path],
    commits: [
      {
        id: commitId,
        at: new Date().toISOString(),
        author,
        message: `Create folder ${path}`,
        files: [],
      },
      ...store.commits,
    ],
  };
}

export function applyRenameFolder(
  store: MemoryStore,
  from: string,
  to: string,
  author: MemoryAuthor,
): MemoryStore {
  if (from === to || !isMemoryPath(to) || to.startsWith(`${from}/`) || occupies(store, to)) {
    return store;
  }
  if (!isFolder(store, from)) {
    return store;
  }
  const rewrite = (path: string) =>
    path === from || path.startsWith(`${from}/`) ? `${to}${path.slice(from.length)}` : path;
  const moved = store.files.filter((file) => file.path.startsWith(`${from}/`)).map((file) => rewrite(file.path));
  const commitId = `c_${crypto.randomUUID().slice(0, 8)}`;
  return {
    files: store.files.map((file) => ({ ...file, path: rewrite(file.path) })),
    directories: Array.from(new Set([...store.directories.map(rewrite), to])),
    commits: [
      {
        id: commitId,
        at: new Date().toISOString(),
        author,
        message: `Rename ${from} to ${to}`,
        files: moved,
      },
      ...store.commits.map((commit) => ({
        ...commit,
        files: commit.files.map(rewrite),
      })),
    ],
  };
}

export function planMove(
  store: MemoryStore,
  from: string,
  parent: string,
): { path: string } | { noop: true } | PathIssue {
  const name = from.includes("/") ? from.slice(from.lastIndexOf("/") + 1) : from;
  const path = parent ? `${parent}/${name}` : name;
  if (path === from) {
    return { noop: true };
  }
  if (parent !== "" && !isFolder(store, parent)) {
    return { text: "Drop onto a folder." };
  }
  if (isFolder(store, from) && (parent === from || parent.startsWith(`${from}/`))) {
    return { text: "A folder cannot move inside itself." };
  }
  if (!isMemoryPath(path) || occupies(store, path)) {
    return { text: "", name, directory: parent ? `/${parent}` : "/" };
  }
  if (!isFolder(store, from) && !store.files.some((file) => file.path === from)) {
    return { text: "That item is not in the tree." };
  }
  return { path };
}

export function applyMove(store: MemoryStore, from: string, to: string, author: MemoryAuthor): MemoryStore {
  if (from === to || !isMemoryPath(to) || to.startsWith(`${from}/`) || occupies(store, to)) {
    return store;
  }
  if (isFolder(store, from)) {
    const rewrite = (path: string) =>
      path === from || path.startsWith(`${from}/`) ? `${to}${path.slice(from.length)}` : path;
    const moved = store.files.filter((file) => file.path.startsWith(`${from}/`)).map((file) => rewrite(file.path));
    const commitId = `c_${crypto.randomUUID().slice(0, 8)}`;
    return {
      files: store.files.map((file) => ({ ...file, path: rewrite(file.path) })),
      directories: Array.from(new Set([...store.directories.map(rewrite), to])),
      commits: [
        {
          id: commitId,
          at: new Date().toISOString(),
          author,
          message: `Move ${from} to ${to}`,
          files: moved,
        },
        ...store.commits.map((commit) => ({
          ...commit,
          files: commit.files.map(rewrite),
        })),
      ],
    };
  }
  const file = store.files.find((item) => item.path === from);
  if (!file) {
    return store;
  }
  const rewrite = (path: string) => (path === from ? to : path);
  const commitId = `c_${crypto.randomUUID().slice(0, 8)}`;
  return {
    files: store.files.map((item) => (item.path === from ? { ...item, path: to } : item)),
    directories: store.directories,
    commits: [
      {
        id: commitId,
        at: new Date().toISOString(),
        author,
        message: `Move ${from} to ${to}`,
        files: [to],
      },
      ...store.commits.map((commit) => ({
        ...commit,
        files: commit.files.map(rewrite),
      })),
    ],
  };
}

function occupies(store: MemoryStore, path: string): boolean {
  return isFolder(store, path) || store.files.some((file) => file.path === path || path.startsWith(`${file.path}/`));
}

function isFolder(store: MemoryStore, path: string): boolean {
  const prefix = `${path}/`;
  return (
    store.directories.some((directory) => directory === path || directory.startsWith(prefix)) ||
    store.files.some((file) => file.path.startsWith(prefix))
  );
}
