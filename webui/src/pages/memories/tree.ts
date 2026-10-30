export type FolderNode = {
  kind: "folder";
  name: string;
  path: string;
  children: TreeNode[];
};

export type FileNode = {
  kind: "file";
  name: string;
  path: string;
};

export type TreeNode = FolderNode | FileNode;

export function buildTree(filePaths: string[], directories: string[] = []): TreeNode[] {
  const root: FolderNode = { kind: "folder", name: "", path: "", children: [] };
  for (const path of directories) {
    ensureFolder(root, path);
  }
  for (const path of filePaths) {
    const parts = path.split("/").filter((part) => part.length > 0);
    let cursor = root;
    for (let index = 0; index < parts.length; index += 1) {
      const name = parts[index] ?? "";
      const childPath = parts.slice(0, index + 1).join("/");
      const last = index === parts.length - 1;
      if (last) {
        if (!cursor.children.some((child) => child.name === name)) {
          cursor.children.push({ kind: "file", name, path: childPath });
        }
        continue;
      }
      cursor = ensureChildFolder(cursor, name, childPath);
    }
  }
  sortNodes(root.children);
  return root.children;
}

function ensureFolder(root: FolderNode, path: string): void {
  const parts = path.split("/").filter((part) => part.length > 0);
  let cursor = root;
  for (let index = 0; index < parts.length; index += 1) {
    const name = parts[index] ?? "";
    const childPath = parts.slice(0, index + 1).join("/");
    cursor = ensureChildFolder(cursor, name, childPath);
  }
}

function ensureChildFolder(cursor: FolderNode, name: string, path: string): FolderNode {
  const existing = cursor.children.find((child) => child.name === name);
  if (existing?.kind === "folder") {
    return existing;
  }
  if (existing) {
    return cursor;
  }
  const folder: FolderNode = { kind: "folder", name, path, children: [] };
  cursor.children.push(folder);
  return folder;
}

export function folderPaths(nodes: TreeNode[]): string[] {
  const paths: string[] = [];
  for (const node of nodes) {
    if (node.kind === "folder") {
      paths.push(node.path, ...folderPaths(node.children));
    }
  }
  return paths;
}

function sortNodes(nodes: TreeNode[]): void {
  nodes.sort((left, right) => {
    if (left.kind !== right.kind) {
      return left.kind === "folder" ? -1 : 1;
    }
    return left.name.localeCompare(right.name);
  });
  for (const node of nodes) {
    if (node.kind === "folder") {
      sortNodes(node.children);
    }
  }
}
