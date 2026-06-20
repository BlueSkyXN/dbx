import type { ColumnInfo, DatabaseType } from "@/types/database";

export interface ExternalTableMeta {
  columns: ColumnInfo[];
  primaryKeys: string[];
}

export function externalRecordIdColumn(primaryKeys: string[] | undefined, resultColumns: string[]): string | undefined {
  if (!primaryKeys?.length) return undefined;
  return primaryKeys.find((primaryKey) => resultColumns.includes(primaryKey));
}

export function isFeishuBitableTableEditable(params: { databaseType?: DatabaseType; context?: "results" | "table-data"; connectionId?: string; tableMeta?: ExternalTableMeta; resultColumns: string[] }): boolean {
  return params.databaseType === "feishu_bitable" && params.context === "table-data" && !!params.connectionId && !!params.tableMeta && !!externalRecordIdColumn(params.tableMeta.primaryKeys, params.resultColumns);
}

export function isFeishuSheetsGridEditable(databaseType?: DatabaseType): boolean {
  return databaseType !== "feishu_sheets";
}
