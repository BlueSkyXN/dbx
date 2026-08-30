export type InsertMode = "unsupported" | "append";
export type DeleteMode = "unsupported" | "remove_row" | "delete_record";
export type ConflictMode = "file_snapshot" | "revision_and_readback";
export type ExternalValueType = "string" | "number" | "boolean" | "date_time" | "json" | "unknown";
export type ReadState = "complete" | "incomplete";
export type OperationOutcome = "applied" | "conflict" | "rejected" | "unknown" | "not_attempted";

export interface AdapterCapabilities {
  canRead: boolean;
  canUpdate: boolean;
  insertMode: InsertMode;
  deleteMode: DeleteMode;
  supportsCellReadonly: boolean;
  conflictMode: ConflictMode;
}

export interface ExternalTableRef {
  tableKey: string;
  displayName: string;
}

export interface ExternalColumn {
  columnKey: string;
  displayName: string;
  valueType: ExternalValueType;
  writable: boolean;
}

export interface ExternalRow {
  rowKey: string;
  values: unknown[];
  readonlyColumnKeys?: string[];
}

export interface PageSnapshot {
  table: ExternalTableRef;
  columns: ExternalColumn[];
  rows: ExternalRow[];
  nextCursor?: string | null;
  snapshotToken: string;
  readState: ReadState;
}

export interface ExternalTableSchema {
  table: ExternalTableRef;
  columns: ExternalColumn[];
  capabilities: AdapterCapabilities;
  writable: boolean;
  readonlyReason?: string | null;
}

export interface ReadPageRequest {
  table: ExternalTableRef;
  cursor?: string | null;
  limit: number;
}

export interface ExternalCellInput {
  columnKey: string;
  value: unknown;
}

export type ExternalOperation = { kind: "update"; operationId: string; rowKey: string; columnKey: string; oldValue: unknown; newValue: unknown } | { kind: "insert"; operationId: string; values: ExternalCellInput[] } | { kind: "delete"; operationId: string; rowKey: string };

export interface ApplyChangesRequest {
  table: ExternalTableRef;
  snapshotToken: string;
  operations: ExternalOperation[];
}

export interface OperationResult {
  operationId: string;
  outcome: OperationOutcome;
  createdRowKey?: string | null;
  message?: string | null;
}

export interface ApplyChangesResult {
  operationResults: OperationResult[];
  newSnapshotToken?: string | null;
  reloadRequired: boolean;
  saveBlocked: boolean;
}

export interface CsvExternalConfig {
  delimiter: string;
  hasHeader: boolean;
  encoding: string;
}

export interface XlsxExternalConfig {
  hasHeader: boolean;
  dataRange?: string;
}

export interface FeishuSheetsExternalConfig {
  spreadsheetToken: string;
  sheetId?: string;
  dataRange?: string;
  hasHeader: boolean;
}

export interface FeishuBaseExternalConfig {
  baseToken: string;
  tableId?: string;
  viewId?: string;
}

export const EXTERNAL_TABLE_DATABASE_TYPES = ["csv", "xlsx", "feishu-sheets", "feishu-base"] as const;
export type ExternalTableDatabaseType = (typeof EXTERNAL_TABLE_DATABASE_TYPES)[number];

export function isExternalTableDatabaseType(value: string | undefined): value is ExternalTableDatabaseType {
  return EXTERNAL_TABLE_DATABASE_TYPES.includes(value as ExternalTableDatabaseType);
}
