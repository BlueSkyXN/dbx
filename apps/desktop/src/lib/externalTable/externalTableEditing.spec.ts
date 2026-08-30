import { describe, expect, it } from "vitest";

import { buildExternalSavePlan, customSaveResultFromExternal, externalSavePreview, gridValueForExternal } from "./externalTableEditing";
import type { ExternalTableSchema, PageSnapshot } from "@/types/externalTable";

const schema: ExternalTableSchema = {
  table: { tableKey: "table", displayName: "Table" },
  columns: [
    { columnKey: "field:name", displayName: "Name", valueType: "string", writable: true },
    { columnKey: "field:tags", displayName: "Tags", valueType: "json", writable: true },
  ],
  capabilities: {
    canRead: true,
    canUpdate: true,
    insertMode: "append",
    deleteMode: "delete_record",
    supportsCellReadonly: true,
    conflictMode: "revision_and_readback",
  },
  writable: true,
};

const page: PageSnapshot = {
  table: schema.table,
  columns: schema.columns,
  rows: [
    { rowKey: "record:1", values: ["Ada", ["Open"]] },
    { rowKey: "record:2", values: ["Grace", ["Done"]] },
  ],
  nextCursor: null,
  snapshotToken: "snapshot",
  readState: "complete",
};

describe("externalTableEditing", () => {
  it("uses stable row/column keys and preserves raw old values", () => {
    const plan = buildExternalSavePlan(
      {
        dirtyRows: new Map([
          [0, new Map([[1, '["Open","Blocked"]']])],
          [1, new Map([[0, "Ignored because the row is deleted"]])],
        ]),
        newRows: [["Lin", '["Open"]']],
        newRowMeta: [{ token: 41, placement: null }],
        deletedRows: new Set([1]),
      },
      page,
      schema,
    );

    expect(plan.operations).toEqual([
      {
        kind: "update",
        operationId: "update-1",
        rowKey: "record:1",
        columnKey: "field:tags",
        oldValue: ["Open"],
        newValue: ["Open", "Blocked"],
      },
      { kind: "delete", operationId: "delete-2", rowKey: "record:2" },
      {
        kind: "insert",
        operationId: "insert-3",
        values: [
          { columnKey: "field:name", value: "Lin" },
          { columnKey: "field:tags", value: ["Open"] },
        ],
      },
    ]);
  });

  it("uses page-owned column metadata when describe metadata is stale", () => {
    const staleSchema: ExternalTableSchema = {
      ...schema,
      columns: schema.columns.map((column, index) => (index === 1 ? { ...column, valueType: "string", writable: true } : column)),
    };
    const currentPage: PageSnapshot = {
      ...page,
      columns: page.columns.map((column, index) => (index === 0 ? { ...column, writable: false } : { ...column, valueType: "json" })),
    };

    const plan = buildExternalSavePlan(
      {
        dirtyRows: new Map([[0, new Map([[1, '["Open","Blocked"]']])]]),
        newRows: [["Ignored", '["Open"]']],
        newRowMeta: [{ token: 99, placement: null }],
        deletedRows: new Set(),
      },
      currentPage,
      staleSchema,
    );

    expect(plan.operations[0]).toMatchObject({ newValue: ["Open", "Blocked"] });
    expect(plan.operations[1]).toMatchObject({
      kind: "insert",
      values: [{ columnKey: "field:tags", value: ["Open"] }],
    });
  });

  it("maps only applied operations back to DataGrid pending identities", () => {
    const plan = buildExternalSavePlan(
      {
        dirtyRows: new Map([
          [0, new Map([[0, "Ada Lovelace"]])],
          [1, new Map([[0, "Grace Hopper"]])],
        ]),
        newRows: [["Lin", "[]"]],
        newRowMeta: [{ token: 7, placement: null }],
        deletedRows: new Set(),
      },
      page,
      schema,
    );

    const result = customSaveResultFromExternal(plan, {
      operationResults: [
        { operationId: "update-1", outcome: "applied" },
        { operationId: "update-2", outcome: "conflict", message: "remote row changed" },
        { operationId: "insert-3", outcome: "unknown", message: "create outcome unknown" },
      ],
      newSnapshotToken: null,
      reloadRequired: true,
      saveBlocked: true,
    });

    expect(result.appliedDirtyCells).toEqual([{ sourceRowIndex: 0, columnIndex: 0 }]);
    expect(result.appliedNewRowTokens).toEqual([]);
    expect(result.conflicts).toEqual(["remote row changed"]);
    expect(result.unknown).toEqual(["create outcome unknown"]);
    expect(result.saveBlocked).toBe(true);
  });

  it("rejects invalid JSON edits before dispatch", () => {
    expect(() => gridValueForExternal("not-json", schema.columns[1])).toThrow("valid JSON");
  });

  it("makes physical row deletion explicit in the save preview", () => {
    const plan = buildExternalSavePlan(
      {
        dirtyRows: new Map(),
        newRows: [],
        newRowMeta: [],
        deletedRows: new Set([0]),
      },
      page,
      schema,
    );

    expect(externalSavePreview(plan, "remove_row")).toEqual(["DELETE ENTIRE SOURCE ROW record:1"]);
    expect(externalSavePreview(plan, "delete_record")).toEqual(["DELETE RECORD record:1"]);
  });

  it("blocks another save when a dispatched operation has no returned outcome", () => {
    const plan = buildExternalSavePlan(
      {
        dirtyRows: new Map([[0, new Map([[0, "Ada Lovelace"]])]]),
        newRows: [],
        newRowMeta: [],
        deletedRows: new Set(),
      },
      page,
      schema,
    );

    const result = customSaveResultFromExternal(plan, {
      operationResults: [],
      reloadRequired: false,
      saveBlocked: false,
    });

    expect(result.unknown).toEqual(["update-1: no result returned"]);
    expect(result.reloadRequired).toBe(true);
    expect(result.saveBlocked).toBe(true);
  });

  it("blocks retrying pending changes after any conflict until reload", () => {
    const plan = buildExternalSavePlan(
      {
        dirtyRows: new Map(),
        newRows: [],
        newRowMeta: [],
        deletedRows: new Set([0]),
      },
      page,
      schema,
    );

    const result = customSaveResultFromExternal(plan, {
      operationResults: [{ operationId: "delete-1", outcome: "conflict", message: "source changed" }],
      newSnapshotToken: null,
      reloadRequired: false,
      saveBlocked: false,
    });

    expect(result.conflicts).toEqual(["source changed"]);
    expect(result.reloadRequired).toBe(true);
    expect(result.saveBlocked).toBe(true);
    expect(result.appliedDeletedRows).toEqual([]);
  });
});
