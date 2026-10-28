import { useEffect, useMemo, useState, type ReactNode } from "react";
import { buildTree, type TreeNode } from "./tree";
import type { MemoryFile } from "./types";

export function FileTree({
  files,
  activePath,
  narrow,
  onToggleNarrow,
  onOpen,
}: {
  files: MemoryFile[];
  activePath: string | null;
  narrow: boolean;
  onToggleNarrow: () => void;
  onOpen: (path: string) => void;
}) {
  const tree = useMemo(() => buildTree(files.map((file) => file.path)), [files]);
  const [open, setOpen] = useState<ReadonlySet<string>>(() => new Set());
  const [query, setQuery] = useState("");
  const needle = query.trim().toLowerCase();
  const searching = needle.length >= 2;
  const matches = useMemo(() => {
    if (!searching) {
      return [];
    }
    return files
      .map((file) => file.path)
      .filter((path) => path.toLowerCase().includes(needle))
      .slice(0, 12);
  }, [files, needle, searching]);

  useEffect(() => {
    if (!activePath) {
      return;
    }
    const parts = activePath.split("/");
    setOpen((current) => {
      const next = new Set(current);
      for (let index = 1; index < parts.length; index += 1) {
        next.add(parts.slice(0, index).join("/"));
      }
      return next;
    });
  }, [activePath]);

  function toggle(path: string): void {
    setOpen((current) => {
      const next = new Set(current);
      if (next.has(path)) {
        next.delete(path);
      } else {
        next.add(path);
      }
      return next;
    });
  }

  return (
    <div className={narrow ? "memory-tree narrow" : "memory-tree"}>
      <div className="memory-tree-head">
        <label className="tree-search">
          <svg className="tree-search-icon" viewBox="0 0 16 16" width="13" height="13" aria-hidden="true">
            <circle cx="7" cy="7" r="4.2" fill="none" stroke="currentColor" strokeWidth="1.5" />
            <path d="M10.2 10.2 13 13" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
          </svg>
          <input
            aria-label="Search files"
            placeholder="Search files"
            value={query}
            role="combobox"
            aria-expanded={searching}
            aria-controls="file-search-results"
            autoComplete="off"
            spellCheck={false}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                setQuery("");
              } else if (event.key === "Enter" && searching && matches[0]) {
                onOpen(matches[0]);
                setQuery("");
              }
            }}
          />
          {searching ? (
            <ul className="tree-search-results" id="file-search-results" role="listbox">
              {matches.length === 0 ? (
                <li className="tree-search-empty">No matching files</li>
              ) : (
                matches.map((path) => (
                  <li key={path}>
                    <button type="button" role="option" onClick={() => { onOpen(path); setQuery(""); }}>
                      <FileGlyph />
                      <span className="tree-search-path">
                        <Highlight text={path} query={query.trim()} />
                      </span>
                    </button>
                  </li>
                ))
              )}
            </ul>
          ) : null}
        </label>
        <button
          className="tree-collapse"
          type="button"
          aria-pressed={narrow}
          aria-label={narrow ? "Expand file list" : "Collapse file list"}
          onClick={onToggleNarrow}
        >
          <svg className={narrow ? "tree-chevron" : "tree-chevron flip"} viewBox="0 0 16 16" width="12" height="12" aria-hidden="true">
            <path d="M6 4.2 10 8 6 11.8" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
        </button>
      </div>
      <div className="memory-tree-scroll" role="tree" aria-label="Memory files">
        <TreeRows nodes={tree} depth={1} open={open} activePath={activePath} onToggle={toggle} onOpen={onOpen} />
      </div>
    </div>
  );
}

function TreeRows({
  nodes,
  depth,
  open,
  activePath,
  onToggle,
  onOpen,
}: {
  nodes: TreeNode[];
  depth: number;
  open: ReadonlySet<string>;
  activePath: string | null;
  onToggle: (path: string) => void;
  onOpen: (path: string) => void;
}) {
  return nodes.map((node) => {
    const pad = { paddingLeft: 8 + (depth - 1) * 16 };
    if (node.kind === "file") {
      const selected = node.path === activePath;
      return (
        <button
          key={node.path}
          className={selected ? "tree-row active" : "tree-row"}
          type="button"
          role="treeitem"
          aria-level={depth}
          aria-current={selected ? "page" : undefined}
          title={node.path}
          style={pad}
          onClick={() => onOpen(node.path)}
        >
          <FileGlyph />
          <span className="tree-label">{node.name}</span>
        </button>
      );
    }
    const expanded = open.has(node.path);
    return (
      <div key={node.path} role="group">
        <button
          className="tree-row"
          type="button"
          role="treeitem"
          aria-expanded={expanded}
          aria-level={depth}
          title={node.path}
          style={pad}
          onClick={() => onToggle(node.path)}
        >
          <Chevron expanded={expanded} />
          <FolderGlyph />
          <span className="tree-label">{node.name}</span>
        </button>
        {expanded ? (
          <TreeRows
            nodes={node.children}
            depth={depth + 1}
            open={open}
            activePath={activePath}
            onToggle={onToggle}
            onOpen={onOpen}
          />
        ) : null}
      </div>
    );
  });
}

function Highlight({ text, query }: { text: string; query: string }) {
  const target = query.toLowerCase();
  const lower = text.toLowerCase();
  const parts: ReactNode[] = [];
  let cursor = 0;
  let found = lower.indexOf(target);
  let key = 0;
  while (target && found !== -1) {
    if (found > cursor) {
      parts.push(text.slice(cursor, found));
    }
    parts.push(<mark key={key}>{text.slice(found, found + target.length)}</mark>);
    key += 1;
    cursor = found + target.length;
    found = lower.indexOf(target, cursor);
  }
  if (cursor < text.length) {
    parts.push(text.slice(cursor));
  }
  return parts;
}

function Chevron({ expanded }: { expanded: boolean }) {
  return (
    <svg className={expanded ? "tree-chevron open" : "tree-chevron"} viewBox="0 0 16 16" width="12" height="12" aria-hidden="true">
      <path d="M6 4.2 10 8 6 11.8" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

function FolderGlyph() {
  return (
    <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
      <path
        d="M2.2 4.2h4l1.2 1.4h6.4v6.6H2.2z"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function FileGlyph() {
  return (
    <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
      <path
        d="M4.2 2.2h4.6L12 5.4v8.4H4.2z"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinejoin="round"
      />
      <path d="M8.6 2.4V5.6H11.8" fill="none" stroke="currentColor" strokeWidth="1.4" />
    </svg>
  );
}
