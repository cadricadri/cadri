import { isMemoryPath } from "./blame";
import type { MemoryAuthor, MemoryStore } from "./types";

const MAX_UPLOAD_BYTES = 1024 * 1024;

export type UploadDraft = {
  path: string;
  content: string;
};

export type PreparedUpload = {
  drafts: UploadDraft[];
  message: string;
};

export type UploadProblem = { error: string } | { collision: { name: string; directory: string } };

export async function prepareUploads(
  existing: readonly string[],
  files: readonly File[],
): Promise<PreparedUpload | UploadProblem> {
  const drafts: UploadDraft[] = [];
  const seen = new Set<string>();
  for (const file of files) {
    if (file.size > MAX_UPLOAD_BYTES) {
      return { error: `${file.name} is larger than 1 MB.` };
    }
    const bytes = new Uint8Array(await file.arrayBuffer());
    if (bytes.includes(0)) {
      return { error: `${file.name} is not a text file.` };
    }
    let raw: string;
    try {
      raw = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    } catch {
      return { error: `${file.name} is not a text file.` };
    }
    const path = uploadPath(file.name);
    if (!path) {
      return { error: `${file.name} is not a valid file name.` };
    }
    if (seen.has(path)) {
      return { error: `${path} was selected more than once.` };
    }
    if (existing.includes(path)) {
      return { collision: { name: path, directory: "/" } };
    }
    seen.add(path);
    drafts.push({ path, content: raw.replace(/\r\n/g, "\n").replace(/\r/g, "\n") });
  }
  if (drafts.length === 0) {
    return { error: "Choose a file to upload." };
  }
  const message = drafts.length === 1 ? `Upload ${drafts[0].path}` : `Upload ${drafts.length} files`;
  return { drafts, message };
}

export function applyUpload(
  store: MemoryStore,
  drafts: UploadDraft[],
  message: string,
  author: MemoryAuthor,
): MemoryStore {
  const trimmed = message.trim();
  if (drafts.length === 0 || trimmed.length === 0) {
    return store;
  }
  const paths = new Set(store.files.map((file) => file.path));
  if (
    drafts.some(
      (draft) =>
        !isMemoryPath(draft.path) ||
        paths.has(draft.path) ||
        store.directories.some((directory) => directory === draft.path || directory.startsWith(`${draft.path}/`)),
    )
  ) {
    return store;
  }
  const commitId = `c_${crypto.randomUUID().slice(0, 8)}`;
  return {
    files: [
      ...store.files,
      ...drafts.map((draft) => ({
        path: draft.path,
        content: draft.content,
        blame: draft.content.split("\n").map(() => ({ author, commit_id: commitId })),
      })),
    ],
    directories: store.directories,
    commits: [
      {
        id: commitId,
        at: new Date().toISOString(),
        author,
        message: trimmed,
        files: drafts.map((draft) => draft.path),
      },
      ...store.commits,
    ],
  };
}

function uploadPath(name: string): string | null {
  const base = name.split(/[/\\]/).pop()?.trim() ?? "";
  return isMemoryPath(base) ? base : null;
}
