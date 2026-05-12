import { z } from "zod";
import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";

const DEFAULT_BASE_URL = "https://open.feishu.cn";
const TOKEN_REFRESH_SKEW_SECONDS = 300;
const SHEETS_DEFAULT_RENDER_OPTION = "ToString";
const SHEETS_DEFAULT_DATE_TIME_OPTION = "FormattedString";
const BITABLE_TABLE_PAGE_SIZE = 100;
const BITABLE_FIELD_PAGE_SIZE = 100;
const BITABLE_RECORD_PAGE_SIZE = 500;
const BITABLE_DEFAULT_MAX_RECORDS = 1000;
const BITABLE_BATCH_CREATE_LIMIT = 500;
const BITABLE_BATCH_UPDATE_LIMIT = 1000;
const BITABLE_BATCH_DELETE_LIMIT = 500;

type JsonObject = Record<string, unknown>;
type JsonArray = unknown[];
type JsonRecord = Record<string, unknown>;

interface FeishuAuthOptions {
  base_url?: string;
  access_token?: string;
  app_id?: string;
  app_secret?: string;
}

interface CachedToken {
  token: string;
  expiresAt: number;
}

interface FeishuEnvelope<T> {
  code?: number;
  msg?: string;
  data?: T;
}

interface FeishuTokenResponse {
  code?: number;
  msg?: string;
  tenant_access_token?: string;
  expire?: number;
}

interface SheetInfoData {
  spreadsheet?: JsonObject;
}

interface SheetQueryData {
  sheets?: SheetMeta[];
}

interface SheetMeta {
  sheet_id?: string;
  title?: string;
  resource_type?: string;
  grid_properties?: {
    row_count?: number;
    column_count?: number;
  };
}

interface SheetValuesData {
  valueRange?: {
    range?: string;
    revision?: number;
    values?: unknown[][];
  };
  revision?: number;
}

interface BitablePagedData<T> {
  items?: T[];
  has_more?: boolean;
  page_token?: string;
  total?: number;
}

interface BitableRecord {
  record_id?: string;
  fields?: JsonObject;
  created_time?: number;
  last_modified_time?: number;
}

const authShape = {
  base_url: z
    .string()
    .optional()
    .describe("Feishu/Lark OpenAPI base URL; defaults to DBX_FEISHU_BASE_URL, FEISHU_BASE_URL, or https://open.feishu.cn"),
  access_token: z
    .string()
    .optional()
    .describe("Optional tenant_access_token or user_access_token. If omitted, DBX_FEISHU_ACCESS_TOKEN/FEISHU_ACCESS_TOKEN is used before app credentials."),
  app_id: z.string().optional().describe("Optional Feishu app_id. Defaults to DBX_FEISHU_APP_ID or FEISHU_APP_ID."),
  app_secret: z.string().optional().describe("Optional Feishu app_secret. Defaults to DBX_FEISHU_APP_SECRET or FEISHU_APP_SECRET."),
};

const spreadsheetShape = {
  spreadsheet_token: z.string().optional().describe("Spreadsheet token from a /sheets/ URL."),
  url: z.string().optional().describe("Spreadsheet URL; used to extract spreadsheet_token when spreadsheet_token is omitted."),
};

const baseTokenShape = {
  base_token: z.string().optional().describe("Bitable/Base app token from a /base/ URL."),
  url: z.string().optional().describe("Bitable/Base URL; used to extract base_token when base_token is omitted."),
};

const jsonObjectSchema = z.record(z.unknown());
const sheetValuesSchema = z.array(z.array(z.unknown()));
const bitableRecordsSchema = z
  .array(jsonObjectSchema)
  .describe('Records. Each item can be {"fields": {...}} or a raw fields object.');

const tokenCache = new Map<string, CachedToken>();

class FeishuApiError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "FeishuApiError";
  }
}

class FeishuClient {
  private readonly baseUrl: string;
  private readonly accessToken?: string;
  private readonly appId?: string;
  private readonly appSecret?: string;

  constructor(options: FeishuAuthOptions = {}) {
    this.baseUrl = cleanBaseUrl(
      firstNonEmpty(options.base_url, process.env.DBX_FEISHU_BASE_URL, process.env.FEISHU_BASE_URL, process.env.LARK_OPEN_BASE_URL),
    );
    this.accessToken = cleanAccessToken(
      firstNonEmpty(options.access_token, process.env.DBX_FEISHU_ACCESS_TOKEN, process.env.FEISHU_ACCESS_TOKEN, process.env.LARK_ACCESS_TOKEN),
    );
    this.appId = firstNonEmpty(options.app_id, process.env.DBX_FEISHU_APP_ID, process.env.FEISHU_APP_ID, process.env.LARK_APP_ID);
    this.appSecret = firstNonEmpty(
      options.app_secret,
      process.env.DBX_FEISHU_APP_SECRET,
      process.env.FEISHU_APP_SECRET,
      process.env.LARK_APP_SECRET,
    );
  }

  async tenantAccessToken(): Promise<string> {
    if (this.accessToken) return this.accessToken;
    if (!this.appId || !this.appSecret) {
      throw new FeishuApiError("Feishu access token or app_id/app_secret is required.");
    }

    const cacheKey = `${this.baseUrl}:${this.appId}:${this.appSecret}`;
    const cached = tokenCache.get(cacheKey);
    const now = Math.floor(Date.now() / 1000);
    if (cached && cached.expiresAt > now) return cached.token;

    const res = await fetch(`${this.baseUrl}/open-apis/auth/v3/tenant_access_token/internal`, {
      method: "POST",
      headers: { "Content-Type": "application/json; charset=utf-8" },
      body: JSON.stringify({
        app_id: this.appId,
        app_secret: this.appSecret,
      }),
    });
    const body = await readResponseText(res, "Feishu token request");
    const parsed = parseJson<FeishuTokenResponse>(body, "Feishu token response");
    if (parsed.code !== 0) {
      throw new FeishuApiError(`Feishu token request failed: code=${parsed.code ?? "unknown"} msg=${parsed.msg ?? ""}`);
    }
    if (!parsed.tenant_access_token) {
      throw new FeishuApiError("Feishu token response missing tenant_access_token.");
    }

    const ttl = Math.max(60, (parsed.expire ?? 7200) - TOKEN_REFRESH_SKEW_SECONDS);
    tokenCache.set(cacheKey, {
      token: parsed.tenant_access_token,
      expiresAt: now + ttl,
    });
    return parsed.tenant_access_token;
  }

  async requestData<T>(method: string, path: string, query?: Record<string, unknown>, body?: unknown): Promise<T> {
    const token = await this.tenantAccessToken();
    const url = new URL(`${this.baseUrl}${path}`);
    for (const [key, value] of Object.entries(query ?? {})) {
      if (value === undefined || value === null || value === "") continue;
      if (Array.isArray(value)) {
        for (const item of value) url.searchParams.append(key, String(item));
      } else {
        url.searchParams.set(key, String(value));
      }
    }

    const init: RequestInit = {
      method,
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json; charset=utf-8",
      },
    };
    if (body !== undefined) init.body = JSON.stringify(body);

    const res = await fetch(url, init);
    const text = await readResponseText(res, `Feishu ${method} ${path}`);
    const parsed = parseJson<FeishuEnvelope<T>>(text, `Feishu ${method} ${path} response`);
    if (parsed.code !== 0) {
      throw new FeishuApiError(`Feishu API error: code=${parsed.code ?? "unknown"} msg=${parsed.msg ?? ""}`);
    }
    return (parsed.data ?? ({} as T)) as T;
  }
}

export function registerFeishuTools(server: McpServer): void {
  server.tool(
    "dbx_feishu_get_tenant_access_token",
    "Get a Feishu tenant_access_token using self-built app credentials. Useful for checking Feishu MCP auth configuration.",
    {
      ...authShape,
    },
    async (args) => withFeishuError(async () => {
      const token = await new FeishuClient(args).tenantAccessToken();
      return jsonText({
        token_type: args.access_token ? "provided_access_token" : "tenant_access_token",
        access_token: token,
      });
    }),
  );

  server.tool(
    "dbx_feishu_sheets_info",
    "Get spreadsheet metadata and worksheet list from Feishu Sheets.",
    {
      ...authShape,
      ...spreadsheetShape,
    },
    async (args) => withFeishuError(async () => {
      const client = new FeishuClient(args);
      const token = resolveSpreadsheetToken(args.spreadsheet_token, args.url);
      const spreadsheet = await client.requestData<SheetInfoData>("GET", `/open-apis/sheets/v3/spreadsheets/${enc(token)}`);
      const sheets = await client.requestData<SheetQueryData>("GET", `/open-apis/sheets/v3/spreadsheets/${enc(token)}/sheets/query`);
      return jsonText({
        spreadsheet_token: token,
        spreadsheet,
        sheets,
      });
    }),
  );

  server.tool(
    "dbx_feishu_sheets_read",
    "Read cell values from Feishu Sheets.",
    {
      ...authShape,
      ...spreadsheetShape,
      range: z.string().optional().describe("Range such as <sheetId>!A1:D10, A1:D10 with sheet_id, a single cell, or a sheet_id."),
      sheet_id: z.string().optional().describe("Worksheet ID used when range is relative or omitted."),
      value_render_option: z
        .enum(["ToString", "FormattedValue", "Formula", "UnformattedValue"])
        .default(SHEETS_DEFAULT_RENDER_OPTION)
        .describe("Value render option."),
      date_time_render_option: z
        .enum(["SerialNumber", "FormattedString"])
        .default(SHEETS_DEFAULT_DATE_TIME_OPTION)
        .describe("Date/time render option."),
    },
    async (args) => withFeishuError(async () => {
      const client = new FeishuClient(args);
      const token = resolveSpreadsheetToken(args.spreadsheet_token, args.url);
      const range = await resolveSheetReadRange(client, token, args.sheet_id, args.range);
      const data = await client.requestData<SheetValuesData>(
        "GET",
        `/open-apis/sheets/v2/spreadsheets/${enc(token)}/values/${enc(range)}`,
        {
          valueRenderOption: args.value_render_option,
          dateTimeRenderOption: args.date_time_render_option,
        },
      );
      return jsonText(data);
    }),
  );

  server.tool(
    "dbx_feishu_sheets_write",
    "Overwrite a cell range in Feishu Sheets.",
    {
      ...authShape,
      ...spreadsheetShape,
      range: z.string().optional().describe("Write range such as <sheetId>!A1:D10, A1:D10 with sheet_id, a single start cell, or a sheet_id."),
      sheet_id: z.string().optional().describe("Worksheet ID used when range is relative or omitted."),
      values: sheetValuesSchema.describe("Two-dimensional array of values to write."),
    },
    async (args) => withFeishuError(async () => {
      const client = new FeishuClient(args);
      const token = resolveSpreadsheetToken(args.spreadsheet_token, args.url);
      const range = await resolveSheetWriteRange(client, token, args.sheet_id, args.range, args.values);
      const data = await client.requestData<JsonObject>("PUT", `/open-apis/sheets/v2/spreadsheets/${enc(token)}/values`, undefined, {
        valueRange: {
          range,
          values: args.values,
        },
      });
      return jsonText(data);
    }),
  );

  server.tool(
    "dbx_feishu_sheets_append",
    "Append rows to Feishu Sheets.",
    {
      ...authShape,
      ...spreadsheetShape,
      range: z.string().optional().describe("Append range such as <sheetId>!A1:D10, A1:D10 with sheet_id, a single start cell, or a sheet_id."),
      sheet_id: z.string().optional().describe("Worksheet ID used when range is relative or omitted."),
      values: sheetValuesSchema.describe("Two-dimensional array of rows to append."),
      insert_data_option: z.enum(["INSERT_ROWS", "OVERWRITE"]).default("INSERT_ROWS").describe("Append insert mode."),
    },
    async (args) => withFeishuError(async () => {
      const client = new FeishuClient(args);
      const token = resolveSpreadsheetToken(args.spreadsheet_token, args.url);
      const range = await resolveSheetPointRange(client, token, args.sheet_id, args.range);
      const data = await client.requestData<JsonObject>(
        "POST",
        `/open-apis/sheets/v2/spreadsheets/${enc(token)}/values_append`,
        { insertDataOption: args.insert_data_option },
        {
          valueRange: {
            range,
            values: args.values,
          },
        },
      );
      return jsonText(data);
    }),
  );

  server.tool(
    "dbx_feishu_bitable_list_tables",
    "List tables in a Feishu Bitable/Base.",
    {
      ...authShape,
      ...baseTokenShape,
    },
    async (args) => withFeishuError(async () => {
      const client = new FeishuClient(args);
      const baseToken = resolveBaseToken(args.base_token, args.url);
      const items = await fetchAllBitablePages<JsonObject>(client, `/open-apis/bitable/v1/apps/${enc(baseToken)}/tables`, {
        page_size: BITABLE_TABLE_PAGE_SIZE,
      });
      return jsonText({ base_token: baseToken, items, count: items.length });
    }),
  );

  server.tool(
    "dbx_feishu_bitable_list_fields",
    "List fields in a Feishu Bitable table.",
    {
      ...authShape,
      ...baseTokenShape,
      table_id: z.string().describe("Bitable table ID."),
      view_id: z.string().optional().describe("Optional view ID."),
    },
    async (args) => withFeishuError(async () => {
      const client = new FeishuClient(args);
      const baseToken = resolveBaseToken(args.base_token, args.url);
      const items = await fetchAllBitablePages<JsonObject>(
        client,
        `/open-apis/bitable/v1/apps/${enc(baseToken)}/tables/${enc(args.table_id)}/fields`,
        {
          page_size: BITABLE_FIELD_PAGE_SIZE,
          view_id: args.view_id,
        },
      );
      return jsonText({ base_token: baseToken, table_id: args.table_id, items, count: items.length });
    }),
  );

  server.tool(
    "dbx_feishu_bitable_search_records",
    "Search/list records in a Feishu Bitable table using the records/search endpoint.",
    {
      ...authShape,
      ...baseTokenShape,
      table_id: z.string().describe("Bitable table ID."),
      view_id: z.string().optional().describe("Optional view ID."),
      field_names: z.array(z.string()).optional().describe("Optional list of field names to return."),
      filter: jsonObjectSchema.optional().describe("Optional Feishu Bitable filter object."),
      sort: z.array(jsonObjectSchema).optional().describe("Optional Feishu Bitable sort array."),
      automatic_fields: z.boolean().default(false).describe("Whether to include automatic fields."),
      user_id_type: z.enum(["open_id", "union_id", "user_id"]).default("open_id").describe("User ID type for user fields."),
      page_size: z.number().int().min(1).max(500).default(BITABLE_RECORD_PAGE_SIZE).describe("Page size, max 500."),
      max_records: z.number().int().min(1).max(10000).default(BITABLE_DEFAULT_MAX_RECORDS).describe("Maximum records to fetch."),
    },
    async (args) => withFeishuError(async () => {
      const client = new FeishuClient(args);
      const baseToken = resolveBaseToken(args.base_token, args.url);
      const items = await searchBitableRecords(client, baseToken, args.table_id, {
        view_id: args.view_id,
        field_names: args.field_names,
        filter: args.filter,
        sort: args.sort,
        automatic_fields: args.automatic_fields,
        user_id_type: args.user_id_type,
        page_size: args.page_size,
        max_records: args.max_records,
      });
      return jsonText({ base_token: baseToken, table_id: args.table_id, items, count: items.length });
    }),
  );

  server.tool(
    "dbx_feishu_bitable_create_records",
    "Batch create records in a Feishu Bitable table.",
    {
      ...authShape,
      ...baseTokenShape,
      table_id: z.string().describe("Bitable table ID."),
      records: bitableRecordsSchema,
      user_id_type: z.enum(["open_id", "union_id", "user_id"]).default("open_id").describe("User ID type for user fields."),
    },
    async (args) => withFeishuError(async () => {
      const client = new FeishuClient(args);
      const baseToken = resolveBaseToken(args.base_token, args.url);
      const records = args.records.map(toBitableRecordBody);
      const raw = await callBitableBatch<JsonObject>(
        client,
        "POST",
        `/open-apis/bitable/v1/apps/${enc(baseToken)}/tables/${enc(args.table_id)}/records/batch_create`,
        records,
        BITABLE_BATCH_CREATE_LIMIT,
        { user_id_type: args.user_id_type },
        "records",
      );
      return jsonText({ base_token: baseToken, table_id: args.table_id, affected_rows: countReturnedRecords(raw), raw });
    }),
  );

  server.tool(
    "dbx_feishu_bitable_update_records",
    "Batch update records in a Feishu Bitable table.",
    {
      ...authShape,
      ...baseTokenShape,
      table_id: z.string().describe("Bitable table ID."),
      records: z.array(jsonObjectSchema).describe('Records shaped as {"record_id": "...", "fields": {...}}.'),
      user_id_type: z.enum(["open_id", "union_id", "user_id"]).default("open_id").describe("User ID type for user fields."),
    },
    async (args) => withFeishuError(async () => {
      const client = new FeishuClient(args);
      const baseToken = resolveBaseToken(args.base_token, args.url);
      const records = args.records.map(toBitableUpdateBody);
      const raw = await callBitableBatch<JsonObject>(
        client,
        "POST",
        `/open-apis/bitable/v1/apps/${enc(baseToken)}/tables/${enc(args.table_id)}/records/batch_update`,
        records,
        BITABLE_BATCH_UPDATE_LIMIT,
        { user_id_type: args.user_id_type },
        "records",
      );
      return jsonText({ base_token: baseToken, table_id: args.table_id, affected_rows: countReturnedRecords(raw), raw });
    }),
  );

  server.tool(
    "dbx_feishu_bitable_delete_records",
    "Batch delete records from a Feishu Bitable table.",
    {
      ...authShape,
      ...baseTokenShape,
      table_id: z.string().describe("Bitable table ID."),
      record_ids: z.array(z.string()).describe("Record IDs to delete."),
    },
    async (args) => withFeishuError(async () => {
      const client = new FeishuClient(args);
      const baseToken = resolveBaseToken(args.base_token, args.url);
      const recordIds = args.record_ids.map((id) => id.trim()).filter(Boolean);
      const raw = await callBitableBatch<JsonArray>(
        client,
        "POST",
        `/open-apis/bitable/v1/apps/${enc(baseToken)}/tables/${enc(args.table_id)}/records/batch_delete`,
        recordIds,
        BITABLE_BATCH_DELETE_LIMIT,
        undefined,
        "records",
      );
      return jsonText({ base_token: baseToken, table_id: args.table_id, affected_rows: recordIds.length, raw });
    }),
  );
}

async function withFeishuError(fn: () => Promise<ReturnType<typeof jsonText>>): Promise<ReturnType<typeof jsonText>> {
  try {
    return await fn();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    return text(`Feishu error: ${message}`);
  }
}

function text(s: string) {
  return { content: [{ type: "text" as const, text: s }] };
}

function jsonText(value: unknown) {
  return text(JSON.stringify(value, null, 2));
}

function cleanBaseUrl(value?: string): string {
  const base = firstNonEmpty(value) ?? DEFAULT_BASE_URL;
  return base.replace(/\/+$/, "");
}

function firstNonEmpty(...values: Array<string | undefined>): string | undefined {
  for (const value of values) {
    const trimmed = value?.trim();
    if (trimmed) return trimmed;
  }
  return undefined;
}

function cleanAccessToken(value: string | undefined): string | undefined {
  const token = value?.trim();
  if (!token) return undefined;
  const bearer = /^bearer\s+(.+)$/i.exec(token);
  return (bearer?.[1] ?? token).trim();
}

async function readResponseText(response: Response, label: string): Promise<string> {
  const textBody = await response.text();
  if (!response.ok) {
    throw new FeishuApiError(`${label} failed with HTTP ${response.status}: ${textBody}`);
  }
  return textBody;
}

function parseJson<T>(body: string, label: string): T {
  try {
    return JSON.parse(body) as T;
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    throw new FeishuApiError(`${label} is not valid JSON: ${message}; body=${body.slice(0, 500)}`);
  }
}

function enc(value: string): string {
  return encodeURIComponent(value);
}

function extractTokenFromUrl(input: string | undefined, prefixes: string[]): string | undefined {
  const trimmed = input?.trim();
  if (!trimmed) return undefined;
  for (const prefix of prefixes) {
    const index = trimmed.indexOf(prefix);
    if (index < 0) continue;
    const rest = trimmed.slice(index + prefix.length);
    return rest.split(/[/?#]/, 1)[0] || undefined;
  }
  return undefined;
}

function resolveSpreadsheetToken(token?: string, url?: string): string {
  const resolved = firstNonEmpty(token, extractTokenFromUrl(url, ["/sheets/", "/spreadsheets/"]));
  if (!resolved) throw new FeishuApiError("specify spreadsheet_token or url.");
  return resolved;
}

function resolveBaseToken(token?: string, url?: string): string {
  const resolved = firstNonEmpty(token, extractTokenFromUrl(url, ["/base/", "/bitable/"]));
  if (!resolved) throw new FeishuApiError("specify base_token or url.");
  return resolved;
}

async function getFirstSheetId(client: FeishuClient, spreadsheetToken: string): Promise<string> {
  const data = await client.requestData<SheetQueryData>("GET", `/open-apis/sheets/v3/spreadsheets/${enc(spreadsheetToken)}/sheets/query`);
  const sheet = (data.sheets ?? []).find((item) => (item.resource_type ?? "sheet") === "sheet" && item.sheet_id);
  if (!sheet?.sheet_id) throw new FeishuApiError("no sheets found in this spreadsheet.");
  return sheet.sheet_id;
}

async function resolveSheetReadRange(client: FeishuClient, spreadsheetToken: string, sheetId?: string, inputRange?: string): Promise<string> {
  let range = firstNonEmpty(inputRange, sheetId);
  if (!range) range = await getFirstSheetId(client, spreadsheetToken);
  return normalizePointRange(sheetId, range);
}

async function resolveSheetPointRange(client: FeishuClient, spreadsheetToken: string, sheetId?: string, inputRange?: string): Promise<string> {
  let range = firstNonEmpty(inputRange, sheetId);
  if (!range) range = await getFirstSheetId(client, spreadsheetToken);
  return normalizePointRange(sheetId, range);
}

async function resolveSheetWriteRange(
  client: FeishuClient,
  spreadsheetToken: string,
  sheetId: string | undefined,
  inputRange: string | undefined,
  values: unknown[][],
): Promise<string> {
  let range = firstNonEmpty(inputRange, sheetId);
  if (!range) range = await getFirstSheetId(client, spreadsheetToken);
  return normalizeWriteRange(sheetId, range, values);
}

function normalizeSheetRange(sheetId: string | undefined, input: string): string {
  const range = input.trim();
  if (!range || range.includes("!") || !sheetId) return range;
  if (looksLikeRelativeRange(range)) return `${sheetId}!${range}`;
  return range;
}

function normalizePointRange(sheetId: string | undefined, input: string): string {
  const range = normalizeSheetRange(sheetId, input);
  const split = splitSheetRange(range);
  if (!split || !singleCellRangePattern.test(split.subRange)) return range;
  return `${split.sheetId}!${split.subRange}:${split.subRange}`;
}

function normalizeWriteRange(sheetId: string | undefined, input: string, values: unknown[][]): string {
  const rows = Math.max(1, values.length);
  const columns = Math.max(1, ...values.map((row) => row.length));
  const trimmed = input.trim();
  if (!trimmed) return buildRectRange(sheetId ?? "", "A1", rows, columns);

  const range = normalizeSheetRange(sheetId, trimmed);
  const split = splitSheetRange(range);
  if (!split) return buildRectRange(range, "A1", rows, columns);
  if (singleCellRangePattern.test(split.subRange)) {
    return buildRectRange(split.sheetId, split.subRange, rows, columns);
  }
  return range;
}

const singleCellRangePattern = /^[A-Za-z]+[1-9][0-9]*$/;
const cellSpanRangePattern = /^[A-Za-z]+[1-9][0-9]*:[A-Za-z]+[1-9][0-9]*$/;
const cellToColRangePattern = /^[A-Za-z]+[1-9][0-9]*:[A-Za-z]+$/;
const colSpanRangePattern = /^[A-Za-z]+:[A-Za-z]+$/;
const rowSpanRangePattern = /^[1-9][0-9]*:[1-9][0-9]*$/;
const cellRefPattern = /^([A-Za-z]+)([1-9][0-9]*)$/;

function looksLikeRelativeRange(input: string): boolean {
  const range = input.trim();
  return (
    singleCellRangePattern.test(range) ||
    cellSpanRangePattern.test(range) ||
    cellToColRangePattern.test(range) ||
    colSpanRangePattern.test(range) ||
    rowSpanRangePattern.test(range)
  );
}

function splitSheetRange(input: string): { sheetId: string; subRange: string } | undefined {
  const [sheetId, subRange, ...rest] = input.trim().split("!");
  if (rest.length || !sheetId || !subRange) return undefined;
  return { sheetId, subRange };
}

function buildRectRange(sheetId: string, anchor: string, rows: number, columns: number): string {
  if (!sheetId) return "";
  return `${sheetId}!${anchor}:${offsetCell(anchor, rows - 1, columns - 1)}`;
}

function offsetCell(cell: string, rowOffset: number, columnOffset: number): string {
  const match = cellRefPattern.exec(cell.trim());
  if (!match) throw new FeishuApiError(`invalid cell reference: ${cell}`);
  const column = columnNameToIndex(match[1]);
  const row = Number(match[2]);
  return `${columnIndexToName(column + columnOffset)}${row + rowOffset}`;
}

function columnNameToIndex(name: string): number {
  let index = 0;
  for (const char of name.toUpperCase()) {
    const code = char.charCodeAt(0);
    if (code < 65 || code > 90) throw new FeishuApiError(`invalid column: ${name}`);
    index = index * 26 + code - 64;
  }
  return index;
}

function columnIndexToName(index: number): string {
  if (index < 1) throw new FeishuApiError(`invalid column index: ${index}`);
  let value = index;
  let result = "";
  while (value > 0) {
    value -= 1;
    result = String.fromCharCode(65 + (value % 26)) + result;
    value = Math.floor(value / 26);
  }
  return result;
}

async function fetchAllBitablePages<T>(
  client: FeishuClient,
  path: string,
  query: Record<string, unknown>,
): Promise<T[]> {
  const items: T[] = [];
  let pageToken: string | undefined;
  do {
    const data = await client.requestData<BitablePagedData<T>>("GET", path, {
      ...query,
      page_token: pageToken,
    });
    items.push(...(data.items ?? []));
    pageToken = data.has_more ? data.page_token : undefined;
  } while (pageToken);
  return items;
}

async function searchBitableRecords(
  client: FeishuClient,
  baseToken: string,
  tableId: string,
  options: {
    view_id?: string;
    field_names?: string[];
    filter?: JsonObject;
    sort?: JsonObject[];
    automatic_fields: boolean;
    user_id_type: string;
    page_size: number;
    max_records: number;
  },
): Promise<BitableRecord[]> {
  const path = `/open-apis/bitable/v1/apps/${enc(baseToken)}/tables/${enc(tableId)}/records/search`;
  const items: BitableRecord[] = [];
  let pageToken: string | undefined;
  do {
    const body: JsonObject = { automatic_fields: options.automatic_fields };
    if (options.view_id) body.view_id = options.view_id;
    if (options.field_names?.length) body.field_names = options.field_names;
    if (options.filter) body.filter = options.filter;
    if (options.sort?.length) body.sort = options.sort;

    const data = await client.requestData<BitablePagedData<BitableRecord>>(
      "POST",
      path,
      {
        page_size: options.page_size,
        page_token: pageToken,
        user_id_type: options.user_id_type,
      },
      body,
    );
    for (const item of data.items ?? []) {
      items.push(item);
      if (items.length >= options.max_records) return items;
    }
    pageToken = data.has_more ? data.page_token : undefined;
  } while (pageToken);
  return items;
}

function toBitableRecordBody(record: JsonObject): JsonObject {
  if (isJsonObject(record.fields)) return { fields: record.fields };
  return { fields: record };
}

function toBitableUpdateBody(record: JsonObject): JsonObject {
  const recordId = typeof record.record_id === "string" ? record.record_id.trim() : "";
  if (!recordId) throw new FeishuApiError('each update record requires a non-empty "record_id".');
  if (!isJsonObject(record.fields)) throw new FeishuApiError('each update record requires a "fields" object.');
  return { record_id: recordId, fields: record.fields };
}

function isJsonObject(value: unknown): value is JsonRecord {
  return !!value && typeof value === "object" && !Array.isArray(value);
}

async function callBitableBatch<T extends unknown[] | JsonObject>(
  client: FeishuClient,
  method: string,
  path: string,
  records: unknown[],
  chunkSize: number,
  query: Record<string, unknown> | undefined,
  bodyKey: string,
): Promise<JsonObject[]> {
  const responses: JsonObject[] = [];
  for (let index = 0; index < records.length; index += chunkSize) {
    const chunk = records.slice(index, index + chunkSize);
    const data = await client.requestData<JsonObject>(method, path, query, { [bodyKey]: chunk as T });
    responses.push(data);
  }
  return responses;
}

function countReturnedRecords(responses: JsonObject[]): number {
  return responses.reduce((sum, response) => {
    const records = response.records;
    return sum + (Array.isArray(records) ? records.length : 0);
  }, 0);
}
