import { describe, expect, it } from "vitest";

import type { ConnectionConfig } from "@/types/database";
import { connectionPickerOptionVisible, externalTableConnectionTargetIsComplete, normalizeExternalTableConfig, normalizeExternalTableConnectionForSubmit } from "./externalTableConnection";

function connection(overrides: Partial<ConnectionConfig>): ConnectionConfig {
  return {
    id: "external",
    name: "External",
    db_type: "csv",
    host: "",
    port: 0,
    username: "",
    password: "",
    transport_layers: [],
    ...overrides,
  };
}

describe("externalTableConnection", () => {
  it("hides all four Desktop-only connection types from the Web picker", () => {
    for (const type of ["csv", "xlsx", "feishu-sheets", "feishu-base"] as const) {
      expect(connectionPickerOptionVisible(type, false), type).toBe(false);
      expect(connectionPickerOptionVisible(type, true), type).toBe(true);
    }
    expect(connectionPickerOptionVisible("mysql", false)).toBe(true);
  });

  it("normalizes optional resource IDs without putting Feishu secrets in external_config", () => {
    const external = normalizeExternalTableConfig("feishu-sheets", {
      spreadsheetToken: "  sht_1  ",
      sheetId: "  sh_1  ",
      dataRange: " ",
      hasHeader: false,
      appSecret: "must-not-survive",
    });

    expect(external).toEqual({ spreadsheetToken: "sht_1", sheetId: "sh_1", dataRange: undefined, hasHeader: false });
    expect(JSON.stringify(external)).not.toContain("must-not-survive");
  });

  it("keeps only the file target and external options for CSV submit", () => {
    const config = connection({
      db_type: "csv",
      host: "  /tmp/people.csv  ",
      port: 3306,
      username: "stale-user",
      password: "stale-password",
      database: "stale-db",
      production_databases: ["stale-db"],
      connection_string: "stale-url",
      jdbc_driver_class: "stale.Driver",
      jdbc_driver_paths: ["/tmp/stale.jar"],
      transport_layers: [{ type: "proxy", id: "proxy", enabled: true, proxy_type: "http", host: "proxy", port: 8080, username: "", password: "" }],
      external_config: { delimiter: "\t", hasHeader: false, encoding: "gb18030" },
    });

    normalizeExternalTableConnectionForSubmit(config);

    expect(config.host).toBe("/tmp/people.csv");
    expect(config.port).toBe(0);
    expect(config.username).toBe("");
    expect(config.password).toBe("");
    expect(config.database).toBeUndefined();
    expect(config.production_databases).toBeUndefined();
    expect(config.connection_string).toBeUndefined();
    expect(config.jdbc_driver_class).toBeUndefined();
    expect(config.jdbc_driver_paths).toEqual([]);
    expect(config.transport_layers).toEqual([]);
    expect(config.external_config).toEqual({ delimiter: "\t", hasHeader: false, encoding: "gb18030" });
  });

  it("requires Feishu app credentials and the source resource token", () => {
    const config = connection({
      db_type: "feishu-base",
      username: "cli_app_id",
      password: "app-secret",
      external_config: { baseToken: "bas_1" },
    });

    expect(externalTableConnectionTargetIsComplete(config)).toBe(true);
    config.password = "";
    expect(externalTableConnectionTargetIsComplete(config)).toBe(false);
  });
});
