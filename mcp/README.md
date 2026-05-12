# DBX MCP Server

MCP server for [DBX](https://github.com/t8y2/dbx) — lets AI agents (Claude Code, Cursor, etc.) query your databases using connections already configured in DBX.

[中文](#中文说明) | English

## Features

- **Zero config** — Automatically reads your DBX connections (including passwords from system keyring)
- **Database tools** — List/add/remove connections, list tables, describe table, execute SQL, open table in DBX UI
- **Connection pooling** — Reuses database connections across queries
- **PostgreSQL & MySQL** — Supports PostgreSQL, MySQL, and compatible databases (Doris, StarRocks, etc.)
- **Feishu OpenAPI tools** — Read/write/append Feishu Sheets and list/search/create/update/delete Feishu Bitable records
- **DBX UI integration** — Open tables directly in the DBX desktop app from your AI agent

## Quick Start

### 1. Install

```bash
npm install -g @dbx-app/mcp-server
```

Or run directly:

```bash
npx @dbx-app/mcp-server
```

### 2. Configure Claude Code

Add to your project's `.mcp.json`:

```json
{
  "mcpServers": {
    "dbx": {
      "command": "dbx-mcp-server"
    }
  }
}
```

Or for development (from source):

```json
{
  "mcpServers": {
    "dbx": {
      "command": "npx",
      "args": ["tsx", "mcp/src/index.ts"],
      "cwd": "/path/to/dbx"
    }
  }
}
```

### 3. Use

In Claude Code, just ask:

- "List my database connections"
- "Show the tables in my local-pg connection"
- "Describe the users table"
- "Query the average salary from employees"
- "Open the orders table in DBX"

## Tools

| Tool | Description |
|---|---|
| `dbx_list_connections` | List all database connections configured in DBX |
| `dbx_add_connection` | Add a new database connection |
| `dbx_remove_connection` | Remove a database connection |
| `dbx_list_tables` | List tables and views for a connection |
| `dbx_describe_table` | Get column definitions for a table |
| `dbx_execute_query` | Execute a SQL query (max 100 rows) |
| `dbx_open_table` | Open a table in DBX desktop app UI |
| `dbx_execute_and_show` | Execute a SQL query in DBX desktop app UI |
| `dbx_feishu_get_tenant_access_token` | Get a Feishu tenant access token |
| `dbx_feishu_sheets_info` | Get spreadsheet and worksheet metadata |
| `dbx_feishu_sheets_read` | Read Feishu Sheets cell values |
| `dbx_feishu_sheets_write` | Overwrite a Feishu Sheets range |
| `dbx_feishu_sheets_append` | Append rows to Feishu Sheets |
| `dbx_feishu_bitable_list_tables` | List tables in a Feishu Bitable/Base |
| `dbx_feishu_bitable_list_fields` | List fields in a Bitable table |
| `dbx_feishu_bitable_search_records` | Search/list Bitable records |
| `dbx_feishu_bitable_create_records` | Batch create Bitable records |
| `dbx_feishu_bitable_update_records` | Batch update Bitable records |
| `dbx_feishu_bitable_delete_records` | Batch delete Bitable records |

### Feishu Authentication

Feishu tools accept `base_url`, `access_token`, `app_id`, and `app_secret` per call. For normal MCP use, configure credentials as environment variables instead:

| Variable | Description |
|---|---|
| `DBX_FEISHU_BASE_URL` / `FEISHU_BASE_URL` | OpenAPI base URL, default `https://open.feishu.cn` |
| `DBX_FEISHU_ACCESS_TOKEN` / `FEISHU_ACCESS_TOKEN` | Optional pre-issued tenant/user access token |
| `DBX_FEISHU_APP_ID` / `FEISHU_APP_ID` | Self-built app ID used to fetch `tenant_access_token` |
| `DBX_FEISHU_APP_SECRET` / `FEISHU_APP_SECRET` | Self-built app secret |

If `access_token` is not provided, the MCP server fetches and caches `tenant_access_token` from `/open-apis/auth/v3/tenant_access_token/internal`. Feishu Sheets accepts `spreadsheet_token` or a `/sheets/` URL. Bitable accepts `base_token` or a `/base/` URL.

## How It Works

```
AI Agent → MCP Server → Database
                ↓
         DBX SQLite database (dbx.db)
```

The MCP server reads your database connections from DBX's SQLite database:

- **macOS**: `~/Library/Application Support/com.dbx.app/dbx.db`
- **Linux**: `~/.config/com.dbx.app/dbx.db`
- **Windows**: `%APPDATA%\com.dbx.app\dbx.db`

## DBX UI Integration

The `dbx_open_table` tool communicates with the running DBX app to open tables directly in the UI. This requires DBX to be running. If DBX is not running, the tool will return an error message.

## Requirements

- [DBX](https://github.com/t8y2/dbx) installed with at least one connection configured
- Node.js 18+

## License

MIT

---

## 中文说明

[DBX](https://github.com/t8y2/dbx) 的 MCP Server，让 AI 编程助手（Claude Code、Cursor 等）直接使用 DBX 中已配置的数据库连接查询数据。

### 特性

- **零配置** — 自动读取 DBX 的连接配置
- **数据库工具** — 列出/添加/删除连接、列出表、查看表结构、执行 SQL、在 DBX 中打开表
- **连接池** — 跨查询复用数据库连接
- **PostgreSQL 和 MySQL** — 支持 PostgreSQL、MySQL 及兼容数据库（Doris、StarRocks 等）
- **飞书 OpenAPI 工具** — 读取/写入/追加飞书电子表格，查询/新增/更新/删除飞书多维表格记录
- **DBX UI 联动** — 从 AI 助手直接在 DBX 桌面端打开表

### 快速开始

#### 1. 安装

```bash
npm install -g @dbx-app/mcp-server
```

或直接运行：

```bash
npx @dbx-app/mcp-server
```

#### 2. 配置 Claude Code

在项目的 `.mcp.json` 中添加：

```json
{
  "mcpServers": {
    "dbx": {
      "command": "dbx-mcp-server"
    }
  }
}
```

#### 3. 使用

在 Claude Code 中直接说：

- "列出我的数据库连接"
- "查看 local-pg 上有哪些表"
- "查看 users 表的结构"
- "查询最近 7 天的订单数量"
- "打开 orders 表"

### 工具列表

| 工具 | 说明 |
|---|---|
| `dbx_list_connections` | 列出 DBX 中所有已配置的数据库连接 |
| `dbx_add_connection` | 添加新的数据库连接 |
| `dbx_remove_connection` | 删除数据库连接 |
| `dbx_list_tables` | 列出指定连接的表和视图 |
| `dbx_describe_table` | 获取表的列定义 |
| `dbx_execute_query` | 执行 SQL 查询（最多返回 100 行） |
| `dbx_open_table` | 在 DBX 桌面端打开指定表 |
| `dbx_execute_and_show` | 在 DBX 桌面端执行 SQL 并展示结果 |
| `dbx_feishu_get_tenant_access_token` | 获取飞书 tenant access token |
| `dbx_feishu_sheets_info` | 获取电子表格和工作表元数据 |
| `dbx_feishu_sheets_read` | 读取飞书电子表格单元格 |
| `dbx_feishu_sheets_write` | 覆盖写入飞书电子表格范围 |
| `dbx_feishu_sheets_append` | 追加飞书电子表格行 |
| `dbx_feishu_bitable_list_tables` | 列出飞书多维表格数据表 |
| `dbx_feishu_bitable_list_fields` | 列出多维表格字段 |
| `dbx_feishu_bitable_search_records` | 查询多维表格记录 |
| `dbx_feishu_bitable_create_records` | 批量新增多维表格记录 |
| `dbx_feishu_bitable_update_records` | 批量更新多维表格记录 |
| `dbx_feishu_bitable_delete_records` | 批量删除多维表格记录 |

### 飞书认证

飞书工具支持每次调用传入 `base_url`、`access_token`、`app_id`、`app_secret`。常规 MCP 使用建议通过环境变量配置：

| 变量 | 说明 |
|---|---|
| `DBX_FEISHU_BASE_URL` / `FEISHU_BASE_URL` | OpenAPI 基础地址，默认 `https://open.feishu.cn` |
| `DBX_FEISHU_ACCESS_TOKEN` / `FEISHU_ACCESS_TOKEN` | 可选的预签发 tenant/user access token |
| `DBX_FEISHU_APP_ID` / `FEISHU_APP_ID` | 自建应用 app ID，用于获取 `tenant_access_token` |
| `DBX_FEISHU_APP_SECRET` / `FEISHU_APP_SECRET` | 自建应用 app secret |

如果没有提供 `access_token`，MCP server 会通过 `/open-apis/auth/v3/tenant_access_token/internal` 获取并缓存 `tenant_access_token`。飞书电子表格支持传 `spreadsheet_token` 或 `/sheets/` URL；多维表格支持传 `base_token` 或 `/base/` URL。

### 工作原理

MCP Server 从 DBX 的 SQLite 数据库读取连接信息：

- **macOS**: `~/Library/Application Support/com.dbx.app/dbx.db`
- **Linux**: `~/.config/com.dbx.app/dbx.db`
- **Windows**: `%APPDATA%\com.dbx.app\dbx.db`

### DBX UI 联动

`dbx_open_table` 工具通过本地 HTTP 接口与运行中的 DBX 应用通信，直接在 UI 中打开表。需要 DBX 正在运行。

### 系统要求

- 已安装 [DBX](https://github.com/t8y2/dbx) 并配置了至少一个数据库连接
- Node.js 18+
