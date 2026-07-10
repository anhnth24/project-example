import type { FsNode } from "./types";

export function normalizeSearch(value: string): string {
  return value
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/đ/g, "d")
    .replace(/Đ/g, "D")
    .toLocaleLowerCase("vi")
    .trim();
}

export function findByRel(node: FsNode | null, relPath: string): FsNode | null {
  if (!node) return null;
  if (node.relPath === relPath) return node;
  for (const child of node.children) {
    const found = findByRel(child, relPath);
    if (found) return found;
  }
  return null;
}

export function flattenFiles(node: FsNode | null): FsNode[] {
  if (!node) return [];
  const files: FsNode[] = [];
  const visit = (current: FsNode) => {
    if (!current.isDir) files.push(current);
    current.children.forEach(visit);
  };
  node.children.forEach(visit);
  return files;
}

export function parentRel(relPath: string): string {
  const slash = relPath.lastIndexOf("/");
  return slash < 0 ? "" : relPath.slice(0, slash);
}

export function folderLabel(relPath: string): string {
  const parent = parentRel(relPath);
  if (!parent) return "DATA";
  return parent.slice(parent.lastIndexOf("/") + 1);
}

export function isWithinRel(candidate: string, parent: string): boolean {
  return candidate === parent || candidate.startsWith(`${parent}/`);
}

export function nodeMatches(node: FsNode, rawQuery: string): boolean {
  const query = normalizeSearch(rawQuery);
  if (!query) return true;
  if (normalizeSearch(node.name).includes(query)) return true;
  return node.children.some((child) => nodeMatches(child, query));
}
