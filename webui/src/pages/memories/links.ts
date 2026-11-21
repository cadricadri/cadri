export type MemoryLinkTarget = {
  path: string;
  hash: string;
};

const scheme = /^[a-z][a-z0-9+.-]*:/i;

/** Resolve a markdown link against the open memory file. External and in-page links stay as written. */
export function resolveMemoryLink(currentPath: string, href: string | undefined): MemoryLinkTarget | null {
  if (!href) {
    return null;
  }
  const trimmed = href.trim();
  if (trimmed === "" || trimmed.startsWith("#") || trimmed.startsWith("//") || scheme.test(trimmed)) {
    return null;
  }

  const hashAt = trimmed.indexOf("#");
  const beforeHash = hashAt === -1 ? trimmed : trimmed.slice(0, hashAt);
  const hash = hashAt === -1 ? "" : trimmed.slice(hashAt + 1);
  const queryAt = beforeHash.indexOf("?");
  const raw = queryAt === -1 ? beforeHash : beforeHash.slice(0, queryAt);
  if (raw === "") {
    return null;
  }

  const parts = raw.startsWith("/") ? [] : directoryParts(currentPath);
  for (const segment of raw.split("/")) {
    if (segment === "" || segment === ".") {
      continue;
    }
    const name = decodeSegment(segment);
    if (name === "" || name === ".") {
      continue;
    }
    if (name === "..") {
      if (parts.length === 0) {
        return null;
      }
      parts.pop();
      continue;
    }
    parts.push(name);
  }
  if (parts.length === 0) {
    return null;
  }
  return { path: parts.join("/"), hash };
}

/** App URL for a memory file link, keeping the current book (scope) and dropping the commit log. */
export function memoryFileLocation(currentPath: string, href: string | undefined, search: URLSearchParams): string | null {
  const target = resolveMemoryLink(currentPath, href);
  if (!target) {
    return null;
  }
  const next = new URLSearchParams(search);
  next.delete("view");
  next.set("file", target.path);
  const hash = target.hash ? `#${target.hash}` : "";
  return `/?${next.toString()}${hash}`;
}

function directoryParts(currentPath: string): string[] {
  const parts = currentPath.split("/");
  parts.pop();
  return parts.filter((part) => part.length > 0);
}

function decodeSegment(segment: string): string {
  try {
    return decodeURIComponent(segment);
  } catch {
    return segment;
  }
}
