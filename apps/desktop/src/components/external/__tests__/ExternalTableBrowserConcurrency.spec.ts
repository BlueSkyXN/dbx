// @vitest-environment happy-dom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createApp, defineComponent, h, nextTick, ref, type App } from "vue";

import type { ExternalTableRef, ExternalTableSchema, PageSnapshot } from "@/types/externalTable";

const mocks = vi.hoisted(() => ({
  ensureConnected: vi.fn(),
  list: vi.fn(),
  describe: vi.fn(),
  read: vi.fn(),
  apply: vi.fn(),
  selectTable: undefined as undefined | ((tableKey: string) => void),
  gridProps: undefined as any,
  runGridSave: undefined as undefined | ((dirtyRows: Map<number, Map<number, unknown>>) => Promise<unknown>),
  hasPendingChanges: false,
  setConnectionId: undefined as undefined | ((connectionId: string) => void),
  refreshBrowser: undefined as undefined | (() => Promise<boolean>),
}));

vi.mock("vue-i18n", () => ({
  useI18n: () => ({ t: (key: string) => key }),
}));

vi.mock("@/lib/backend/tauriRuntime", () => ({
  isTauriRuntime: () => true,
}));

vi.mock("@/lib/backend/api", () => ({
  externalTableList: mocks.list,
  externalTableDescribe: mocks.describe,
  externalTableReadPage: mocks.read,
  externalTableApplyChanges: mocks.apply,
}));

vi.mock("@/stores/connectionStore", () => ({
  useConnectionStore: () => ({
    ensureConnected: mocks.ensureConnected,
    getConfig: (connectionId: string) => ({ id: connectionId, db_type: "csv", read_only: false }),
  }),
}));

vi.mock("@lucide/vue", async () => {
  const { defineComponent, h } = await import("vue");
  const icon = defineComponent({ setup: () => () => h("span") });
  return { ChevronLeft: icon, ChevronRight: icon, Loader2: icon, RefreshCw: icon, ShieldAlert: icon };
});

vi.mock("@/components/ui/badge", async () => {
  const { defineComponent, h } = await import("vue");
  return {
    Badge: defineComponent({
      setup(_props, { slots }) {
        return () => h("span", slots.default?.());
      },
    }),
  };
});

vi.mock("@/components/ui/button", async () => {
  const { defineComponent, h } = await import("vue");
  return {
    Button: defineComponent({
      inheritAttrs: false,
      props: { disabled: Boolean },
      emits: ["click"],
      setup(props, { attrs, emit, slots }) {
        return () => h("button", { ...attrs, disabled: props.disabled, onClick: () => emit("click") }, slots.default?.());
      },
    }),
  };
});

vi.mock("@/components/ui/select", async () => {
  const { defineComponent, h } = await import("vue");
  const passthrough = defineComponent({
    setup(_props, { slots }) {
      return () => h("div", slots.default?.());
    },
  });
  return {
    Select: defineComponent({
      emits: ["update:modelValue"],
      setup(_props, { emit, slots }) {
        mocks.selectTable = (tableKey: string) => emit("update:modelValue", tableKey);
        return () => h("div", slots.default?.());
      },
    }),
    SelectContent: passthrough,
    SelectItem: passthrough,
    SelectTrigger: passthrough,
    SelectValue: passthrough,
  };
});

vi.mock("@/components/grid/DataGrid.vue", async () => {
  const { defineComponent, h, ref } = await import("vue");
  return {
    default: defineComponent({
      name: "DataGridStub",
      inheritAttrs: false,
      props: {
        result: { type: Object, required: true },
        customSaveHandler: { type: Object, default: undefined },
      },
      setup(props, { expose }) {
        const isSaving = ref(false);
        mocks.gridProps = props;
        mocks.runGridSave = async (dirtyRows) => {
          const handler = props.customSaveHandler as any;
          if (!handler) throw new Error("custom save handler is not ready");
          const changes = {
            dirtyRows,
            newRows: [],
            newRowMeta: [],
            deletedRows: new Set<number>(),
            columns: (props.result as any).columns,
            rows: (props.result as any).rows,
          };
          isSaving.value = true;
          try {
            const result = await handler.save(changes);
            const applied = new Map<number, Map<number, unknown>>();
            for (const cell of result?.appliedDirtyCells ?? []) {
              const value = dirtyRows.get(cell.sourceRowIndex)?.get(cell.columnIndex);
              if (value === undefined) continue;
              const row = applied.get(cell.sourceRowIndex) ?? new Map<number, unknown>();
              row.set(cell.columnIndex, value);
              applied.set(cell.sourceRowIndex, row);
            }
            handler.applySavedChanges?.({ dirtyRows: applied, columns: changes.columns });
            for (const [rowIndex, columns] of applied) {
              for (const [columnIndex, value] of columns) (props.result as any).rows[rowIndex][columnIndex] = value;
            }
            return result;
          } finally {
            isSaving.value = false;
          }
        };
        expose({
          isSaving,
          hasPendingChanges: () => mocks.hasPendingChanges,
          isCustomSaveBlocked: () => false,
          discardPendingChanges: () => {
            mocks.hasPendingChanges = false;
          },
        });
        return () => h("div", { id: "grid", "data-rows": JSON.stringify((props.result as any).rows) });
      },
    }),
  };
});

import ExternalTableBrowser from "@/components/external/ExternalTableBrowser.vue";

const column = { columnKey: "col:1", displayName: "Name", valueType: "string", writable: true } as const;
const tableA: ExternalTableRef = { tableKey: "table-a", displayName: "Table A" };
const tableB: ExternalTableRef = { tableKey: "table-b", displayName: "Table B" };
const tableC: ExternalTableRef = { tableKey: "table-c", displayName: "Table C" };

function schema(table: ExternalTableRef): ExternalTableSchema {
  return {
    table,
    columns: [column],
    capabilities: {
      canRead: true,
      canUpdate: true,
      insertMode: "append",
      deleteMode: "remove_row",
      supportsCellReadonly: true,
      conflictMode: "file_snapshot",
    },
    writable: true,
  };
}

function page(table: ExternalTableRef, value: string, snapshotToken: string): PageSnapshot {
  return {
    table,
    columns: [column],
    rows: [{ rowKey: `row:${table.tableKey}`, values: [value] }],
    nextCursor: null,
    snapshotToken,
    readState: "complete",
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

let app: App<Element> | undefined;
let host: HTMLDivElement | undefined;

async function flushUi() {
  await Promise.resolve();
  await Promise.resolve();
  await nextTick();
  await Promise.resolve();
  await nextTick();
}

async function mountBrowser(connectionId = "conn-a") {
  host = document.createElement("div");
  document.body.appendChild(host);
  app = createApp(
    defineComponent({
      setup() {
        const currentConnectionId = ref(connectionId);
        const browser = ref<any>();
        mocks.setConnectionId = (nextConnectionId) => {
          currentConnectionId.value = nextConnectionId;
        };
        mocks.refreshBrowser = () => browser.value.refresh();
        return () =>
          h(ExternalTableBrowser, {
            ref: browser,
            connectionId: currentConnectionId.value,
            pendingStateKey: `external:${currentConnectionId.value}`,
          });
      },
    }),
  );
  app.mount(host);
  await flushUi();
}

beforeEach(() => {
  mocks.ensureConnected.mockReset().mockResolvedValue(undefined);
  mocks.list.mockReset();
  mocks.describe.mockReset();
  mocks.read.mockReset();
  mocks.apply.mockReset();
  mocks.selectTable = undefined;
  mocks.gridProps = undefined;
  mocks.runGridSave = undefined;
  mocks.hasPendingChanges = false;
  mocks.setConnectionId = undefined;
  mocks.refreshBrowser = undefined;
  vi.stubGlobal(
    "confirm",
    vi.fn(() => true),
  );
});

afterEach(() => {
  app?.unmount();
  app = undefined;
  host?.remove();
  host = undefined;
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("ExternalTableBrowser concurrency guards", () => {
  it("keeps the newest connection page when older describe/read calls finish later", async () => {
    const describeA = deferred<ExternalTableSchema>();
    const readA = deferred<PageSnapshot>();
    mocks.list.mockImplementation(async (connectionId: string) => (connectionId === "conn-a" ? [tableA] : [tableC]));
    mocks.describe.mockImplementation((connectionId: string, table: ExternalTableRef) => (connectionId === "conn-a" ? describeA.promise : Promise.resolve(schema(table))));
    mocks.read.mockImplementation((connectionId: string, request: { table: ExternalTableRef }) => (connectionId === "conn-a" ? readA.promise : Promise.resolve(page(request.table, "C", "snapshot-c"))));

    await mountBrowser();
    expect(mocks.describe).toHaveBeenCalledWith("conn-a", tableA);

    mocks.setConnectionId?.("conn-c");
    await flushUi();
    expect((mocks.gridProps.result as any).rows).toEqual([["C"]]);

    describeA.resolve(schema(tableA));
    await flushUi();
    readA.resolve(page(tableA, "A", "snapshot-a"));
    await flushUi();

    expect((mocks.gridProps.result as any).rows).toEqual([["C"]]);
  });

  it("locks navigation during save and ignores a completion owned by the previous connection", async () => {
    const save = deferred<any>();
    mocks.list.mockImplementation(async (connectionId: string) => (connectionId === "conn-a" ? [tableA, tableB] : [tableC]));
    mocks.describe.mockImplementation(async (_connectionId: string, table: ExternalTableRef) => schema(table));
    mocks.read.mockImplementation(async (_connectionId: string, request: { table: ExternalTableRef }) => {
      if (request.table.tableKey === tableA.tableKey) return page(tableA, "A", "snapshot-a");
      if (request.table.tableKey === tableB.tableKey) return page(tableB, "B", "snapshot-b");
      return page(tableC, "C", "snapshot-c");
    });
    mocks.apply.mockReturnValue(save.promise);

    await mountBrowser();
    mocks.hasPendingChanges = true;
    const savePromise = mocks.runGridSave!(new Map([[0, new Map([[0, "A saved"]])]]));
    await flushUi();

    expect(mocks.apply).toHaveBeenCalledWith("conn-a", {
      table: tableA,
      snapshotToken: "snapshot-a",
      operations: [
        {
          kind: "update",
          operationId: "update-1",
          rowKey: "row:table-a",
          columnKey: "col:1",
          oldValue: "A",
          newValue: "A saved",
        },
      ],
    });
    const describeCallsBeforeNavigation = mocks.describe.mock.calls.length;
    mocks.selectTable?.(tableB.tableKey);
    await expect(mocks.refreshBrowser?.()).resolves.toBe(false);
    await flushUi();
    expect(mocks.describe).toHaveBeenCalledTimes(describeCallsBeforeNavigation);
    expect(window.confirm).not.toHaveBeenCalled();

    mocks.setConnectionId?.("conn-c");
    await flushUi();
    expect((mocks.gridProps.result as any).rows).toEqual([["C"]]);

    save.resolve({
      operationResults: [{ operationId: "update-1", outcome: "applied" }],
      newSnapshotToken: "snapshot-a2",
      reloadRequired: false,
      saveBlocked: false,
    });
    await expect(savePromise).rejects.toThrow("externalTable.saveContextChanged");
    await flushUi();

    expect((mocks.gridProps.result as any).rows).toEqual([["C"]]);
    expect(document.body.textContent).not.toContain("externalTable.saveApplied");
  });
});
