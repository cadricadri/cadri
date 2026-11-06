export type MemoryAuthor =
  | { kind: "human"; user_id: string; label: string }
  | { kind: "robot"; id: string };

export type BlameEntry = {
  author: MemoryAuthor;
  commit_id: string;
};

export type MemoryFile = {
  path: string;
  /** Blob SHA of the saved text. Empty for a file this page just created or edited. */
  blob: string;
  /** `null` means the text is not loaded yet. */
  content: string | null;
  blame: BlameEntry[];
};

export type LoadedFile = MemoryFile & { content: string };

export type TreeEntry = { path: string; blob: string; size: number };

export type OpenFile = { path: string; blob: string; content: string };

export type MemoryTree = {
  tip: string | null;
  seq: number;
  files: TreeEntry[];
  directories: string[];
  open?: OpenFile;
};

export type MemoryCommit = {
  id: string;
  at: string;
  author: MemoryAuthor;
  message: string;
  files: string[];
};

export type MemoryStore = {
  tip: string | null;
  files: MemoryFile[];
  directories: string[];
  commits: MemoryCommit[];
};
