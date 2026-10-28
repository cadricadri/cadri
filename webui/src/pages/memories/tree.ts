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

export function buildTree(paths: string[]): TreeNode[] {
  const root: FolderNode = { kind: "folder", name: "", path: "", children: [] };
  for (const path of paths) {
    const parts = path.split("/").filter((part) => part.length > 0);
    let cursor = root;
    for (let index = 0; index < parts.length; index += 1) {
      const name = parts[index] ?? "";
      const childPath = parts.slice(0, index + 1).join("/");
      const last = index === parts.length - 1;
      if (last) {
        cursor.children.push({ kind: "file", name, path: childPath });
        continue;
      }
      let folder = cursor.children.find(
        (child): child is FolderNode => child.kind === "folder" && child.name === name,
      );
      if (!folder) {
        folder = { kind: "folder", name, path: childPath, children: [] };
        cursor.children.push(folder);
      }
      cursor = folder;
    }
  }
  sortNodes(root.children);
  return root.children;
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
