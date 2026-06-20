import assert from "node:assert/strict";
import test from "node:test";
import { externalRecordIdColumn, isFeishuBitableTableEditable, isFeishuSheetsGridEditable } from "../apps/desktop/src/lib/externalTableEditing.ts";

test("Feishu Bitable editing uses the primary key column returned by the backend", () => {
  assert.equal(externalRecordIdColumn(["__dbx_record_id__"], ["__dbx_record_id__", "Name"]), "__dbx_record_id__");
  assert.equal(externalRecordIdColumn(["__dbx_record_id_2__"], ["__dbx_record_id__", "__dbx_record_id_2__", "Name"]), "__dbx_record_id_2__");
  assert.equal(externalRecordIdColumn(["__dbx_record_id__"], ["Name"]), undefined);
});

test("Feishu Bitable table grid is editable only with table context and visible record id primary key", () => {
  assert.equal(
    isFeishuBitableTableEditable({
      databaseType: "feishu_bitable",
      context: "table-data",
      connectionId: "conn",
      tableMeta: { columns: [], primaryKeys: ["__dbx_record_id__"] },
      resultColumns: ["__dbx_record_id__", "Name"],
    }),
    true,
  );
  assert.equal(
    isFeishuBitableTableEditable({
      databaseType: "feishu_bitable",
      context: "results",
      connectionId: "conn",
      tableMeta: { columns: [], primaryKeys: ["__dbx_record_id__"] },
      resultColumns: ["__dbx_record_id__", "Name"],
    }),
    false,
  );
  assert.equal(
    isFeishuBitableTableEditable({
      databaseType: "feishu_bitable",
      context: "table-data",
      connectionId: "conn",
      tableMeta: { columns: [], primaryKeys: ["__dbx_record_id__"] },
      resultColumns: ["Name"],
    }),
    false,
  );
});

test("Feishu Sheets is not treated as a normal editable grid", () => {
  assert.equal(isFeishuSheetsGridEditable("feishu_sheets"), false);
  assert.equal(isFeishuSheetsGridEditable("feishu_bitable"), true);
});
