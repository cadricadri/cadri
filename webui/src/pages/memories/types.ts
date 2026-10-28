export type MemoryAuthor =
  | { kind: "human"; user_id: string; label: string }
  | { kind: "robot"; session_id: string };

export type BlameEntry = {
  author: MemoryAuthor;
  commit_id: string;
};

export type MemoryFile = {
  path: string;
  content: string;
  blame: BlameEntry[];
};

export type MemoryCommit = {
  id: string;
  at: string;
  author: MemoryAuthor;
  message: string;
  files: string[];
};

export type MemoryStore = {
  files: MemoryFile[];
  commits: MemoryCommit[];
};
