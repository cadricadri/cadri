import { useEffect, useMemo, useRef, useState, type DragEvent, type ReactNode } from "react";
import { planCreate, planMove, planRename, type PathIssue } from "./folders";
import { buildTree, folderPaths, type TreeNode } from "./tree";
import type { MemoryFile, MemoryStore } from "./types";

export function FileTree({
  store,
  activePath,
  narrow,
  changesDisabled,
  onToggleNarrow,
  onOpen,
  onUpload,
  onCreateFolder,
  onCreateFile,
  onRenameFolder,
  onMove,
  onIssue,
}: {
  store: MemoryStore;
  activePath: string | null;
  narrow: boolean;
  changesDisabled: boolean;
  onToggleNarrow: () => void;
  onOpen: (path: string) => void;
  onUpload: (files: File[]) => void;
  onCreateFolder: (path: string) => void;
  onCreateFile: (directory: string) => void;
  onRenameFolder: (from: string, to: string) => void;
  onMove: (from: string, parent: string) => void;
  onIssue: (title: string, issue: PathIssue) => void;
}) {
  const files = store.files;
  const tree = useMemo(
    () => buildTree(files.map((file) => file.path), store.directories),
    [files, store.directories],
  );
  const folders = useMemo(() => new Set(folderPaths(tree)), [tree]);
  const [open, setOpen] = useState<ReadonlySet<string>>(() => new Set());
  const [query, setQuery] = useState("");
  const [dragging, setDragging] = useState(false);
  const [draftParent, setDraftParent] = useState<string | null>(null);
  const [draftName, setDraftName] = useState("");
  const [renaming, setRenaming] = useState<string | null>(null);
  const [renameName, setRenameName] = useState("");
  const [menu, setMenu] = useState<string | null>(null);
  const [dragSource, setDragSource] = useState<string | null>(null);
  const [dropParent, setDropParent] = useState<string | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);
  const dragPath = useRef<string | null>(null);
  const dropParentRef = useRef<string | null>(null);
  const expandPath = useRef<string | null>(null);
  const expandTimer = useRef<number | null>(null);
  const suppressClick = useRef(false);
  const needle = query.trim().toLowerCase();
  const searching = needle.length >= 2;
  const matches = useMemo(() => {
    if (!searching) {
      return [];
    }
    return [...folders, ...files.map((file) => file.path)]
      .filter((path) => path.toLowerCase().includes(needle))
      .slice(0, 12);
  }, [files, folders, needle, searching]);

  useEffect(() => {
    if (!activePath) {
      return;
    }
    reveal(activePath);
  }, [activePath]);

  function reveal(path: string): void {
    const parts = path.split("/");
    setOpen((current) => {
      const next = new Set(current);
      for (let index = 1; index <= parts.length; index += 1) {
        const folder = parts.slice(0, index).join("/");
        if (folders.has(folder) || index < parts.length) {
          next.add(folder);
        }
      }
      return next;
    });
  }

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

  function startCreate(parent: string): void {
    if (changesDisabled) {
      return;
    }
    setMenu(null);
    setRenaming(null);
    setDraftName("");
    setDraftParent(parent);
    if (parent) {
      setOpen((current) => new Set(current).add(parent));
    }
  }

  function toggleMenu(parent: string): void {
    if (changesDisabled || !parent) {
      return;
    }
    setRenaming(null);
    setDraftParent(null);
    setMenu((current) => (current === parent ? null : parent));
  }

  function endDrag(): void {
    dragPath.current = null;
    dropParentRef.current = null;
    expandPath.current = null;
    if (expandTimer.current !== null) {
      window.clearTimeout(expandTimer.current);
      expandTimer.current = null;
    }
    setDragSource(null);
    setDropParent(null);
  }

  function queueExpand(path: string): void {
    if (expandPath.current === path) {
      return;
    }
    expandPath.current = path;
    if (expandTimer.current !== null) {
      window.clearTimeout(expandTimer.current);
    }
    expandTimer.current = window.setTimeout(() => {
      setOpen((current) => new Set(current).add(path));
      expandTimer.current = null;
    }, 450);
  }

  function beginDrag(path: string, event: DragEvent): void {
    if (changesDisabled) {
      event.preventDefault();
      return;
    }
    event.dataTransfer.setData("application/x-cadri-path", path);
    event.dataTransfer.effectAllowed = "move";
    dragPath.current = path;
    suppressClick.current = true;
    setDragSource(path);
    setMenu(null);
  }

  function clickAfterDrag(action: () => void): void {
    if (suppressClick.current) {
      suppressClick.current = false;
      return;
    }
    action();
  }

  function dragOverParent(event: DragEvent, parent: string): void {
    if (!dragPath.current) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    const planned = planMove(store, dragPath.current, parent);
    if ("path" in planned) {
      event.dataTransfer.dropEffect = "move";
      dropParentRef.current = parent;
      setDropParent(parent);
      if (parent) {
        queueExpand(parent);
      } else {
        expandPath.current = null;
      }
      return;
    }
    event.dataTransfer.dropEffect = "none";
    dropParentRef.current = null;
    setDropParent(null);
    expandPath.current = null;
    if (expandTimer.current !== null) {
      window.clearTimeout(expandTimer.current);
      expandTimer.current = null;
    }
  }

  function dropOnParent(event: DragEvent, parent: string): void {
    if (!dragPath.current) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    const from = dragPath.current;
    endDrag();
    const planned = planMove(store, from, parent);
    if ("path" in planned) {
      reveal(planned.path);
      onMove(from, parent);
      return;
    }
    if (!("noop" in planned)) {
      onIssue("Could not move", planned);
    }
  }

  function submitCreate(): void {
    if (draftParent === null) {
      return;
    }
    const planned = planCreate(store, draftParent, draftName);
    if ("path" in planned) {
      onCreateFolder(planned.path);
      setDraftParent(null);
      setDraftName("");
      reveal(planned.path);
      return;
    }
    onIssue("Could not create the folder", planned);
  }

  function startRename(path: string, name: string): void {
    if (changesDisabled) {
      return;
    }
    setMenu(null);
    setDraftParent(null);
    setRenaming(path);
    setRenameName(name);
  }

  function submitRename(): void {
    if (!renaming) {
      return;
    }
    const from = renaming;
    const planned = planRename(store, from, renameName);
    setRenaming(null);
    if ("path" in planned) {
      if (planned.path !== from) {
        onRenameFolder(from, planned.path);
        reveal(planned.path);
      }
      return;
    }
    onIssue("Could not rename the folder", planned);
  }

  function openMatch(path: string): void {
    if (folders.has(path)) {
      reveal(path);
    } else {
      onOpen(path);
    }
    setQuery("");
  }

  return (
    <div
      className={["memory-tree", narrow ? "narrow" : "", dragging ? "dropping" : ""].filter(Boolean).join(" ")}
      onDragOver={(event) => {
        if (dragPath.current) {
          event.preventDefault();
          return;
        }
        event.preventDefault();
        if (!changesDisabled) {
          setDragging(true);
        }
      }}
      onDragLeave={(event) => {
        if (event.currentTarget.contains(event.relatedTarget as Node | null)) {
          return;
        }
        setDragging(false);
        dropParentRef.current = null;
        setDropParent(null);
      }}
      onDrop={(event) => {
        event.preventDefault();
        setDragging(false);
        if (dragPath.current) {
          endDrag();
          return;
        }
        if (!changesDisabled) {
          onUpload(Array.from(event.dataTransfer.files));
        }
      }}
    >
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
                openMatch(matches[0]);
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
                    <button type="button" role="option" onClick={() => openMatch(path)}>
                      {folders.has(path) ? <FolderGlyph /> : <FileGlyph />}
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
        <input
          ref={fileRef}
          className="tree-upload-input"
          type="file"
          multiple
          tabIndex={-1}
          aria-hidden="true"
          onChange={(event) => {
            onUpload(Array.from(event.target.files ?? []));
            event.target.value = "";
          }}
        />
        <button
          className="tree-folder"
          type="button"
          aria-label="New folder"
          title={changesDisabled ? "Finish editing before adding a folder" : "New folder"}
          disabled={changesDisabled}
          onClick={() => startCreate("")}
        >
          <NewFolderGlyph />
        </button>
        <button
          className="tree-upload"
          type="button"
          aria-label="Upload files"
          title={changesDisabled ? "Finish editing before uploading" : "Upload files"}
          disabled={changesDisabled}
          onClick={() => fileRef.current?.click()}
        >
          <UploadGlyph />
        </button>
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
      <div
        className={["memory-tree-scroll", dropParent === "" ? "drop-root" : ""].filter(Boolean).join(" ")}
        role="tree"
        aria-label="Memory files"
        onDragOver={(event) => dragOverParent(event, "")}
        onDrop={(event) => dropOnParent(event, "")}
      >
        {dragging ? <p className="tree-drop">Drop to upload</p> : null}
        {draftParent === "" ? (
          <NameDraft
            label="New folder name"
            value={draftName}
            depth={1}
            onChange={setDraftName}
            onSubmit={submitCreate}
            onCancel={() => setDraftParent(null)}
          />
        ) : null}
        {tree.length === 0 && draftParent === null && !dragging ? <p className="tree-empty">No files yet</p> : null}
        <TreeRows
          nodes={tree}
          depth={1}
          open={open}
          activePath={activePath}
          draftParent={draftParent}
          draftName={draftName}
          renaming={renaming}
          renameName={renameName}
          changesDisabled={changesDisabled}
          onDraftName={setDraftName}
          onSubmitCreate={submitCreate}
          onCancelCreate={() => setDraftParent(null)}
          onRenameName={setRenameName}
          onSubmitRename={submitRename}
          onCancelRename={() => setRenaming(null)}
          onToggle={(path) => clickAfterDrag(() => toggle(path))}
          onOpen={(path) => clickAfterDrag(() => onOpen(path))}
          menu={menu}
          dragSource={dragSource}
          dropParent={dropParent}
          onToggleMenu={toggleMenu}
          onDismissMenu={() => setMenu(null)}
          onStartCreate={startCreate}
          onCreateFile={(directory) => {
            setMenu(null);
            onCreateFile(directory);
          }}
          onStartRename={startRename}
          onDragStart={beginDrag}
          onDragEnd={endDrag}
          onDragOverParent={dragOverParent}
          onDropParent={dropOnParent}
        />
      </div>
    </div>
  );
}

function parentDir(path: string): string {
  return path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "";
}

function TreeRows({
  nodes,
  depth,
  open,
  activePath,
  draftParent,
  draftName,
  renaming,
  renameName,
  changesDisabled,
  onDraftName,
  onSubmitCreate,
  onCancelCreate,
  onRenameName,
  onSubmitRename,
  onCancelRename,
  onToggle,
  onOpen,
  menu,
  dragSource,
  dropParent,
  onToggleMenu,
  onDismissMenu,
  onStartCreate,
  onCreateFile,
  onStartRename,
  onDragStart,
  onDragEnd,
  onDragOverParent,
  onDropParent,
}: {
  nodes: TreeNode[];
  depth: number;
  open: ReadonlySet<string>;
  activePath: string | null;
  draftParent: string | null;
  draftName: string;
  renaming: string | null;
  renameName: string;
  changesDisabled: boolean;
  onDraftName: (value: string) => void;
  onSubmitCreate: () => void;
  onCancelCreate: () => void;
  onRenameName: (value: string) => void;
  onSubmitRename: () => void;
  onCancelRename: () => void;
  onToggle: (path: string) => void;
  onOpen: (path: string) => void;
  menu: string | null;
  dragSource: string | null;
  dropParent: string | null;
  onToggleMenu: (parent: string) => void;
  onDismissMenu: () => void;
  onStartCreate: (parent: string) => void;
  onCreateFile: (directory: string) => void;
  onStartRename: (path: string, name: string) => void;
  onDragStart: (path: string, event: DragEvent) => void;
  onDragEnd: () => void;
  onDragOverParent: (event: DragEvent, parent: string) => void;
  onDropParent: (event: DragEvent, parent: string) => void;
}) {
  return nodes.map((node) => {
    const pad = { paddingLeft: 8 + (depth - 1) * 16 };
    if (node.kind === "file") {
      const selected = node.path === activePath;
      const parent = parentDir(node.path);
      return (
        <button
          key={node.path}
          className={["tree-row", selected ? "active" : "", dragSource === node.path ? "drag-source" : ""].filter(Boolean).join(" ")}
          type="button"
          role="treeitem"
          aria-level={depth}
          aria-current={selected ? "page" : undefined}
          title={node.path}
          style={pad}
          draggable={!changesDisabled}
          onClick={() => onOpen(node.path)}
          onDragStart={(event) => onDragStart(node.path, event)}
          onDragEnd={onDragEnd}
          onDragOver={(event) => onDragOverParent(event, parent)}
          onDrop={(event) => onDropParent(event, parent)}
        >
          <FileGlyph />
          <span className="tree-label">{node.name}</span>
        </button>
      );
    }
    const expanded = open.has(node.path) || draftParent === node.path;
    return (
      <div key={node.path} className={menu === node.path ? "tree-add-open" : undefined} role="group">
        <div
          className={["tree-row", dragSource === node.path ? "drag-source" : "", dropParent === node.path ? "drop-target" : ""].filter(Boolean).join(" ")}
          role="treeitem"
          aria-expanded={expanded}
          aria-level={depth}
          title={node.path}
          style={pad}
          onDragOver={(event) => onDragOverParent(event, node.path)}
          onDrop={(event) => onDropParent(event, node.path)}
        >
          {renaming === node.path ? (
            <NameDraft
              label={`Rename ${node.name}`}
              value={renameName}
              depth={0}
              onChange={onRenameName}
              onSubmit={onSubmitRename}
              onCancel={onCancelRename}
            />
          ) : (
            <>
              <button
                type="button"
                className="tree-row-main"
                draggable={!changesDisabled}
                onClick={() => onToggle(node.path)}
                onDragStart={(event) => onDragStart(node.path, event)}
                onDragEnd={onDragEnd}
              >
                <Chevron expanded={expanded} />
                <FolderGlyph />
                <span className="tree-label">{node.name}</span>
              </button>
              <FolderMenu
                name={node.name}
                open={menu === node.path}
                disabled={changesDisabled}
                onToggle={() => onToggleMenu(node.path)}
                onDismiss={onDismissMenu}
                onRename={() => onStartRename(node.path, node.name)}
                onNewFolder={() => onStartCreate(node.path)}
                onNewFile={() => onCreateFile(node.path)}
              />
            </>
          )}
        </div>
        {draftParent === node.path ? (
          <NameDraft
            label={`New folder in ${node.name}`}
            value={draftName}
            depth={depth + 1}
            onChange={onDraftName}
            onSubmit={onSubmitCreate}
            onCancel={onCancelCreate}
          />
        ) : null}
        {expanded ? (
          <TreeRows
            nodes={node.children}
            depth={depth + 1}
            open={open}
            activePath={activePath}
            draftParent={draftParent}
            draftName={draftName}
            renaming={renaming}
            renameName={renameName}
            changesDisabled={changesDisabled}
            onDraftName={onDraftName}
            onSubmitCreate={onSubmitCreate}
            onCancelCreate={onCancelCreate}
            onRenameName={onRenameName}
            onSubmitRename={onSubmitRename}
            onCancelRename={onCancelRename}
            onToggle={onToggle}
            onOpen={onOpen}
            menu={menu}
            dragSource={dragSource}
            dropParent={dropParent}
            onToggleMenu={onToggleMenu}
            onDismissMenu={onDismissMenu}
            onStartCreate={onStartCreate}
            onCreateFile={onCreateFile}
            onStartRename={onStartRename}
            onDragStart={onDragStart}
            onDragEnd={onDragEnd}
            onDragOverParent={onDragOverParent}
            onDropParent={onDropParent}
          />
        ) : null}
      </div>
    );
  });
}

function FolderMenu({
  name,
  open,
  disabled,
  onToggle,
  onDismiss,
  onRename,
  onNewFolder,
  onNewFile,
}: {
  name: string;
  open: boolean;
  disabled: boolean;
  onToggle: () => void;
  onDismiss: () => void;
  onRename: () => void;
  onNewFolder: () => void;
  onNewFile: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) {
      return;
    }
    function onPointer(event: PointerEvent): void {
      if (!ref.current?.contains(event.target as Node)) {
        onDismiss();
      }
    }
    function onKey(event: KeyboardEvent): void {
      if (event.key === "Escape") {
        onDismiss();
      }
    }
    document.addEventListener("pointerdown", onPointer);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", onPointer);
      document.removeEventListener("keydown", onKey);
    };
  }, [open, onDismiss]);

  return (
    <div className="tree-add" ref={ref}>
      <button
        type="button"
        className="tree-mini"
        aria-label={`Actions for ${name}`}
        aria-haspopup="menu"
        aria-expanded={open}
        title="Folder actions"
        disabled={disabled}
        onClick={onToggle}
      >
        <PencilGlyph />
      </button>
      {open ? (
        <div className="tree-add-menu" role="menu" aria-label={`Actions for ${name}`}>
          <button type="button" role="menuitem" onClick={onRename}>
            Rename folder
          </button>
          <button type="button" role="menuitem" onClick={onNewFolder}>
            New sub-folder
          </button>
          <button type="button" role="menuitem" onClick={onNewFile}>
            New file
          </button>
        </div>
      ) : null}
    </div>
  );
}

function NameDraft({
  label,
  value,
  depth,
  onChange,
  onSubmit,
  onCancel,
}: {
  label: string;
  value: string;
  depth: number;
  onChange: (value: string) => void;
  onSubmit: () => void;
  onCancel: () => void;
}) {
  return (
    <form
      className="tree-draft"
      style={depth > 0 ? { paddingLeft: 8 + (depth - 1) * 16 } : undefined}
      onSubmit={(event) => {
        event.preventDefault();
        onSubmit();
      }}
    >
      <FolderGlyph />
      <input
        aria-label={label}
        value={value}
        autoFocus
        spellCheck={false}
        onChange={(event) => onChange(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            onCancel();
          }
        }}
      />
    </form>
  );
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

function UploadGlyph() {
  return (
    <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
      <path d="M8 2.4v6.2" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
      <path d="M5.3 5.1 8 2.4l2.7 2.7" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
      <path d="M3.2 9.2v3.2h9.6V9.2" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
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

function NewFolderGlyph() {
  return (
    <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
      <path d="M1.8 4.4h3.6l1.1 1.3h7.7v6.4H1.8z" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinejoin="round" />
      <path d="M8 7.2v3.6M6.2 9h3.6" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
    </svg>
  );
}

function PencilGlyph() {
  return (
    <svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true">
      <path d="M9.2 3.2 12.8 6.8 6 13.6H2.4V10z" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinejoin="round" />
    </svg>
  );
}
