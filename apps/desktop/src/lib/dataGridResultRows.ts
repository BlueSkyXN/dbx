import type { CellValue } from "@/lib/cellValue";

export function applyDirtyRowsToResultRows(rows: CellValue[][], dirtyRows: Map<number, Map<number, CellValue>>) {
  for (const [sourceIndex, changes] of dirtyRows) {
    const row = rows[sourceIndex];
    if (!row) continue;
    for (const [colIdx, value] of changes) {
      row[colIdx] = value;
    }
  }
}
