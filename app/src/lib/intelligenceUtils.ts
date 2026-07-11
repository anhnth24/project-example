export function intelligenceSlug(value: string): string {
  return value
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/đ/gi, "d")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "")
    .slice(0, 60);
}

export function reconcileIntelligenceScope(
  current: string[],
  available: string[],
): string[] {
  const valid = current.filter((relPath) => available.includes(relPath));
  if (valid.length || !available.length) return valid;
  return [...available];
}

export function toggleScopeItem(current: string[], relPath: string): string[] {
  return current.includes(relPath)
    ? current.filter((item) => item !== relPath)
    : [...current, relPath];
}

export function updateTableCell(
  rows: string[][],
  rowIndex: number,
  columnIndex: number,
  value: string,
): string[][] {
  return rows.map((row, currentRow) =>
    currentRow === rowIndex
      ? row.map((cell, currentColumn) =>
          currentColumn === columnIndex ? value : cell,
        )
      : row,
  );
}

export function sameScope(left: string[], right: string[]): boolean {
  return left.length === right.length && left.every((item) => right.includes(item));
}

export function canGenerateHandoff(sourceRels: string[], productName: string): boolean {
  return sourceRels.length > 0 && productName.trim().length > 0;
}
