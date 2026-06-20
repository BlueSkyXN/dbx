import assert from "node:assert/strict";
import test from "node:test";
import { applyDirtyRowsToResultRows } from "../apps/desktop/src/lib/dataGridResultRows.ts";

test("applies dirty cell values to matching result rows", () => {
  const rows = [
    ["r1", "old", 1],
    ["r2", "keep", 2],
  ];
  const dirtyRows = new Map([
    [
      0,
      new Map([
        [1, "new"],
        [2, 3],
      ]),
    ],
  ]);

  applyDirtyRowsToResultRows(rows, dirtyRows);

  assert.deepEqual(rows, [
    ["r1", "new", 3],
    ["r2", "keep", 2],
  ]);
});

test("ignores dirty rows outside the current result page", () => {
  const rows = [["r1", "old"]];
  const dirtyRows = new Map([[2, new Map([[1, "new"]])]]);

  applyDirtyRowsToResultRows(rows, dirtyRows);

  assert.deepEqual(rows, [["r1", "old"]]);
});
