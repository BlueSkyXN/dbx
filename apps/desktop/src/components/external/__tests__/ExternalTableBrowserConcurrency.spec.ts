import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const source = readFileSync(new URL("../ExternalTableBrowser.vue", import.meta.url), "utf8");

describe("ExternalTableBrowser concurrency guards", () => {
  it("ignores stale table loads and saves against the page-owned table", () => {
    expect(source).toContain("const generation = ++loadGeneration");
    expect(source).toContain("generation !== loadGeneration");
    expect(source).toContain("selectedTable.value?.tableKey !== table.tableKey");
    expect(source).toContain("table: currentPage.table");
    expect(source).toContain("tableMatches(currentPage.table, selectedTable.value)");
  });
});
