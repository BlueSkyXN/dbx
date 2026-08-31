import type { CustomSaveResult } from "@/composables/useDataGridEditor";
import type { CellValue } from "@/lib/dataGrid/cellValue";
import type { GridNewRowMeta } from "@/lib/dataGrid/gridNewRowPlacement";
import type { ApplyChangesResult, DeleteMode, ExternalColumn, ExternalOperation, ExternalTableSchema, PageSnapshot } from "@/types/externalTable";

export interface ExternalGridSaveChanges {
  dirtyRows: Map<number, Map<number, CellValue>>;
  newRows: CellValue[][];
  newRowMeta: GridNewRowMeta[];
  deletedRows: Set<number>;
}

type PlannedChange = { kind: "update"; sourceRowIndex: number; columnIndex: number } | { kind: "insert"; newRowToken: number } | { kind: "delete"; sourceRowIndex: number; dirtyColumnIndexes: number[] };

export interface ExternalSavePlan {
  operations: ExternalOperation[];
  changesByOperationId: Map<string, PlannedChange>;
}

export function externalValueForGrid(value: unknown): CellValue {
  if (value === null || typeof value === "string" || typeof value === "number" || typeof value === "boolean") return value;
  return JSON.stringify(value);
}

export function gridValueForExternal(value: CellValue, column: ExternalColumn): unknown {
  if (column.valueType !== "json" || value === null || typeof value !== "string") return value;
  const trimmed = value.trim();
  if (!trimmed) return null;
  try {
    return JSON.parse(trimmed);
  } catch {
    throw new Error(`Column '${column.displayName}' requires valid JSON.`);
  }
}

export function buildExternalSavePlan(changes: ExternalGridSaveChanges, page: PageSnapshot, schema: ExternalTableSchema): ExternalSavePlan {
  const columns = page.columns.length ? page.columns : schema.columns;
  const operations: ExternalOperation[] = [];
  const changesByOperationId = new Map<string, PlannedChange>();
  let sequence = 0;
  const operationId = (kind: string) => `${kind}-${++sequence}`;

  for (const [sourceRowIndex, dirtyColumns] of changes.dirtyRows) {
    if (changes.deletedRows.has(sourceRowIndex)) continue;
    const row = page.rows[sourceRowIndex];
    if (!row) throw new Error(`External row ${sourceRowIndex + 1} is no longer available; reload first.`);
    for (const [columnIndex, value] of dirtyColumns) {
      const column = columns[columnIndex];
      if (!column) throw new Error(`External column ${columnIndex + 1} is no longer available; reload first.`);
      const id = operationId("update");
      operations.push({
        kind: "update",
        operationId: id,
        rowKey: row.rowKey,
        columnKey: column.columnKey,
        oldValue: row.values[columnIndex] ?? null,
        newValue: gridValueForExternal(value, column),
      });
      changesByOperationId.set(id, { kind: "update", sourceRowIndex, columnIndex });
    }
  }

  for (const sourceRowIndex of changes.deletedRows) {
    const row = page.rows[sourceRowIndex];
    if (!row) throw new Error(`External row ${sourceRowIndex + 1} is no longer available; reload first.`);
    const id = operationId("delete");
    operations.push({ kind: "delete", operationId: id, rowKey: row.rowKey });
    changesByOperationId.set(id, {
      kind: "delete",
      sourceRowIndex,
      dirtyColumnIndexes: [...(changes.dirtyRows.get(sourceRowIndex)?.keys() ?? [])],
    });
  }

  changes.newRows.forEach((row, newRowIndex) => {
    const meta = changes.newRowMeta[newRowIndex];
    if (!meta) throw new Error(`External inserted row ${newRowIndex + 1} has no stable pending-row token.`);
    const id = operationId("insert");
    const values = columns.flatMap((column, columnIndex) =>
      column.writable
        ? [
            {
              columnKey: column.columnKey,
              value: gridValueForExternal(row[columnIndex] ?? null, column),
            },
          ]
        : [],
    );
    operations.push({ kind: "insert", operationId: id, values });
    changesByOperationId.set(id, { kind: "insert", newRowToken: meta.token });
  });

  return { operations, changesByOperationId };
}

export function externalSavePreview(plan: ExternalSavePlan, deleteMode: DeleteMode): string[] {
  return plan.operations.map((operation) => {
    if (operation.kind === "update") return `UPDATE ${operation.rowKey} ${operation.columnKey}`;
    if (operation.kind === "delete") {
      return deleteMode === "delete_record" ? `DELETE RECORD ${operation.rowKey}` : `DELETE ENTIRE SOURCE ROW ${operation.rowKey}`;
    }
    return `APPEND ${operation.values.length} cell(s)`;
  });
}

export function customSaveResultFromExternal(plan: ExternalSavePlan, result: ApplyChangesResult): CustomSaveResult {
  const custom: CustomSaveResult = {
    appliedDirtyCells: [],
    appliedNewRowTokens: [],
    appliedDeletedRows: [],
    conflicts: [],
    rejected: [],
    unknown: [],
    reloadRequired: result.reloadRequired,
    saveBlocked: result.saveBlocked,
  };
  const hasNonApplied = result.operationResults.some(({ outcome }) => outcome !== "applied");
  let structuralReloadMessageAdded = false;
  const seen = new Set<string>();
  for (const operationResult of result.operationResults) {
    seen.add(operationResult.operationId);
    const change = plan.changesByOperationId.get(operationResult.operationId);
    const message = operationResult.message || `${operationResult.operationId}: ${operationResult.outcome}`;
    if (!change) {
      custom.rejected.push(message);
      continue;
    }
    if (operationResult.outcome === "applied") {
      if (change.kind === "update") {
        custom.appliedDirtyCells.push({ sourceRowIndex: change.sourceRowIndex, columnIndex: change.columnIndex });
      } else if (change.kind === "insert") {
        if (hasNonApplied) {
          custom.saveBlocked = true;
          if (!structuralReloadMessageAdded) custom.unknown.push("A structural change was applied alongside unresolved operations; reload before saving again.");
          structuralReloadMessageAdded = true;
        } else {
          custom.appliedNewRowTokens.push(change.newRowToken);
        }
      } else {
        if (hasNonApplied) {
          custom.saveBlocked = true;
          if (!structuralReloadMessageAdded) custom.unknown.push("A structural change was applied alongside unresolved operations; reload before saving again.");
          structuralReloadMessageAdded = true;
        } else {
          custom.appliedDeletedRows.push(change.sourceRowIndex);
          custom.appliedDirtyCells.push(...change.dirtyColumnIndexes.map((columnIndex) => ({ sourceRowIndex: change.sourceRowIndex, columnIndex })));
        }
      }
    } else if (operationResult.outcome === "conflict") {
      custom.conflicts.push(message);
      custom.reloadRequired = true;
      custom.saveBlocked = true;
    } else if (operationResult.outcome === "unknown") {
      custom.unknown.push(message);
      custom.reloadRequired = true;
      custom.saveBlocked = true;
    } else {
      custom.rejected.push(message);
    }
  }
  for (const operation of plan.operations) {
    if (!seen.has(operation.operationId)) {
      custom.unknown.push(`${operation.operationId}: no result returned`);
      custom.reloadRequired = true;
      custom.saveBlocked = true;
    }
  }
  return custom;
}
