import type { ConnectionConfig, DatabaseType } from "@/types/database";
import { isExternalTableDatabaseType, type CsvExternalConfig, type ExternalTableDatabaseType, type FeishuBaseExternalConfig, type FeishuSheetsExternalConfig, type XlsxExternalConfig } from "@/types/externalTable";

export type ExternalTableConnectionConfig = CsvExternalConfig | XlsxExternalConfig | FeishuSheetsExternalConfig | FeishuBaseExternalConfig;

function configRecord(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : {};
}

function stringValue(record: Record<string, unknown>, key: string, fallback = ""): string {
  return typeof record[key] === "string" ? record[key] : fallback;
}

function optionalString(record: Record<string, unknown>, key: string): string | undefined {
  return stringValue(record, key).trim() || undefined;
}

function booleanValue(record: Record<string, unknown>, key: string, fallback: boolean): boolean {
  return typeof record[key] === "boolean" ? record[key] : fallback;
}

export function defaultExternalTableConfig(dbType: ExternalTableDatabaseType): ExternalTableConnectionConfig {
  if (dbType === "csv") return { delimiter: ",", hasHeader: true, encoding: "utf-8" };
  if (dbType === "xlsx") return { hasHeader: true };
  if (dbType === "feishu-sheets") return { spreadsheetToken: "", hasHeader: true };
  return { baseToken: "" };
}

export function normalizeExternalTableConfig(dbType: ExternalTableDatabaseType, value: unknown): ExternalTableConnectionConfig {
  const record = configRecord(value);
  if (dbType === "csv") {
    return {
      delimiter: stringValue(record, "delimiter", ",") || ",",
      hasHeader: booleanValue(record, "hasHeader", true),
      encoding: stringValue(record, "encoding", "utf-8").trim() || "utf-8",
    };
  }
  if (dbType === "xlsx") {
    return {
      hasHeader: booleanValue(record, "hasHeader", true),
      dataRange: optionalString(record, "dataRange"),
    };
  }
  if (dbType === "feishu-sheets") {
    return {
      spreadsheetToken: stringValue(record, "spreadsheetToken").trim(),
      sheetId: optionalString(record, "sheetId"),
      dataRange: optionalString(record, "dataRange"),
      hasHeader: booleanValue(record, "hasHeader", true),
    };
  }
  return {
    baseToken: stringValue(record, "baseToken").trim(),
    tableId: optionalString(record, "tableId"),
    viewId: optionalString(record, "viewId"),
  };
}

export function connectionPickerOptionVisible(dbType: DatabaseType | string, desktop: boolean): boolean {
  return desktop || !isExternalTableDatabaseType(dbType);
}

export function externalTableConnectionTargetIsComplete(config: Pick<ConnectionConfig, "db_type" | "host" | "username" | "password" | "external_config">): boolean {
  if (!isExternalTableDatabaseType(config.db_type)) return false;
  if (config.db_type === "csv" || config.db_type === "xlsx") return config.host.trim().length > 0;

  const resourceToken = config.db_type === "feishu-sheets" ? (normalizeExternalTableConfig("feishu-sheets", config.external_config) as FeishuSheetsExternalConfig).spreadsheetToken : (normalizeExternalTableConfig("feishu-base", config.external_config) as FeishuBaseExternalConfig).baseToken;
  return config.username.trim().length > 0 && config.password.length > 0 && resourceToken.length > 0;
}

export function normalizeExternalTableConnectionForSubmit(config: ConnectionConfig): void {
  if (!isExternalTableDatabaseType(config.db_type)) return;

  config.external_config = normalizeExternalTableConfig(config.db_type, config.external_config);
  config.database = undefined;
  config.default_schema = undefined;
  config.visible_databases = undefined;
  config.visible_database_patterns = undefined;
  config.visible_schemas = undefined;
  config.show_system_schemas = undefined;
  config.production_databases = undefined;
  config.attached_databases = [];
  config.init_script = undefined;
  config.connection_string = undefined;
  config.url_params = "";
  config.agent_java_options = [];
  config.jdbc_driver_class = undefined;
  config.jdbc_driver_paths = [];
  config.transport_layers = [];
  config.ssl = false;
  config.ca_cert_path = "";
  config.client_cert_path = "";
  config.client_key_path = "";

  if (config.db_type === "csv" || config.db_type === "xlsx") {
    config.host = config.host.trim();
    config.port = 0;
    config.username = "";
    config.password = "";
  } else {
    config.host = "";
    config.port = 443;
    config.username = config.username.trim();
  }
}
