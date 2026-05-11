<script setup lang="ts">
import { ref, computed, watch } from "vue";
import { useI18n } from "vue-i18n";
import {
  Database,
  Table,
  Columns3,
  Eye,
  ChevronRight,
  ChevronDown,
  Loader2,
  FolderOpen,
  FolderClosed,
  Trash2,
  TerminalSquare,
  RefreshCw,
  Copy,
  TableProperties,
  Key,
  Link,
  Zap,
  ListTree,
  Pencil,
  Plug,
  Unplug,
  Pin,
  ArrowRightLeft,
  Download,
  FileCode,
  Network,
  FileUp,
  PencilRuler,
  Search,
  FolderInput,
  FolderPlus,
  Eraser,
  Scissors,
  CopyPlus,
  Plus,
} from "lucide-vue-next";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
  ContextMenuSub,
  ContextMenuSubTrigger,
  ContextMenuSubContent,
} from "@/components/ui/context-menu";
import { useConnectionStore } from "@/stores/connectionStore";
import { useQueryStore } from "@/stores/queryStore";
import { useToast } from "@/composables/useToast";
import { useDatabaseOptions } from "@/composables/useDatabaseOptions";
import type { DatabaseType, QueryResult, TreeNode, TreeNodeType } from "@/types/database";
import * as api from "@/lib/api";
import { uuid } from "@/lib/utils";
import { resolveDefaultDatabase } from "@/lib/defaultDatabase";
import {
  DATABASE_EXPORT_PAGE_SIZE,
  DATABASE_EXPORT_ROW_LIMIT,
  buildDatabaseSqlExport,
  buildExportPageSql,
  type ExportedTableSql,
} from "@/lib/databaseExport";
import { qualifiedTableName as buildQualifiedTableName, quoteTableIdentifier } from "@/lib/tableSelectSql";
import {
  SQL_FILE_UNSUPPORTED_TYPES,
  DIAGRAM_SUPPORTED_TYPES,
  DATABASE_SEARCH_SUPPORTED_TYPES,
  TABLE_IMPORT_SUPPORTED_TYPES,
  TABLE_STRUCTURE_SUPPORTED_TYPES,
  FIELD_LINEAGE_SUPPORTED_TYPES,
  TREE_SCHEMA_TYPES,
  PG_LIKE_STRUCTURE_TYPES,
  CREATE_DATABASE_SUPPORTED_TYPES,
  EXTERNAL_TABULAR_TYPES,
  isSchemaAware,
  usesFetchFirst,
} from "@/lib/databaseCapabilities";
import { treeNodeRowAction } from "@/lib/treeNodeClick";
import DangerConfirmDialog from "@/components/editor/DangerConfirmDialog.vue";
import { isTauriRuntime } from "@/lib/tauriRuntime";
import DatabaseIcon from "@/components/icons/DatabaseIcon.vue";
import ConnectionErrorIndicator from "@/components/connection/ConnectionErrorIndicator.vue";
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";

const { t } = useI18n();
const connectionStore = useConnectionStore();
const queryStore = useQueryStore();
const { toast } = useToast();
const { getDatabaseOptions } = useDatabaseOptions();

const props = defineProps<{
  node: TreeNode;
  depth: number;
  dragDisabled?: boolean;
  pendingRename?: boolean;
}>();

const emit = defineEmits<{
  "rename-started": [];
}>();

const isExportingDatabase = ref(false);

function currentDatabaseType(): DatabaseType | undefined {
  return props.node.connectionId ? connectionStore.getConfig(props.node.connectionId)?.db_type : undefined;
}

function quoteIdent(name: string): string {
  return quoteTableIdentifier(currentDatabaseType(), name);
}

function qualifiedTableName(tableName: string, schema?: string): string {
  return buildQualifiedTableName({
    databaseType: currentDatabaseType(),
    schema,
    tableName,
  });
}

function safeFileName(name: string): string {
  return name.replace(/[\\/:*?"<>|]+/g, "_").trim() || "database";
}

function getIconInfo(node: TreeNode): { icon: any; colorClass: string } | null {
  switch (node.type) {
    case "connection":
      return null;
    case "connection-group":
      return { icon: node.isExpanded ? FolderOpen : FolderClosed, colorClass: "text-amber-500" };
    case "database":
      return { icon: Database, colorClass: "text-yellow-500" };
    case "schema":
      return { icon: FolderOpen, colorClass: "text-sky-400" };
    case "table":
      return { icon: Table, colorClass: "text-green-500" };
    case "view":
      return { icon: Eye, colorClass: "text-purple-500" };
    case "column":
      return { icon: Columns3, colorClass: "text-muted-foreground" };
    case "group-columns":
      return { icon: ListTree, colorClass: "text-green-400" };
    case "group-indexes":
      return { icon: Key, colorClass: "text-amber-500" };
    case "group-fkeys":
      return { icon: Link, colorClass: "text-blue-400" };
    case "group-triggers":
      return { icon: Zap, colorClass: "text-orange-400" };
    case "index":
      return { icon: Key, colorClass: "text-amber-400" };
    case "fkey":
      return { icon: Link, colorClass: "text-blue-300" };
    case "trigger":
      return { icon: Zap, colorClass: "text-orange-300" };
    case "redis-db":
      return { icon: Database, colorClass: "text-red-400" };
    case "mongo-db":
      return { icon: Database, colorClass: "text-green-500" };
    case "mongo-collection":
      return { icon: Table, colorClass: "text-green-400" };
    default:
      return { icon: Database, colorClass: "text-muted-foreground" };
  }
}

const leafTypes: Set<TreeNodeType> = new Set(["column", "index", "fkey", "trigger", "redis-db", "mongo-collection"]);
const groupTypes: Set<TreeNodeType> = new Set(["group-columns", "group-indexes", "group-fkeys", "group-triggers"]);
const pinnableTypes: Set<TreeNodeType> = new Set([
  "connection-group",
  "database",
  "schema",
  "table",
  "view",
  "redis-db",
  "mongo-db",
  "mongo-collection",
]);

function isGroupLabel(node: TreeNode): boolean {
  return groupTypes.has(node.type);
}

async function toggle() {
  const node = props.node;
  if (node.isLoading) return;

  if (node.type === "connection-group") {
    node.isExpanded = !node.isExpanded;
    connectionStore.toggleConnectionGroupCollapsed(node.id);
    return;
  }

  if (node.isExpanded) {
    node.isExpanded = false;
    return;
  }

  try {
    if (node.type === "connection" && node.connectionId) {
      const config = connectionStore.getConfig(node.connectionId);
      if (config?.db_type === "redis") {
        await connectionStore.loadRedisDatabases(node.connectionId);
      } else if (config?.db_type === "mongodb" || config?.db_type === "elasticsearch") {
        await connectionStore.loadMongoDatabases(node.connectionId);
      } else {
        await connectionStore.loadDatabases(node.connectionId);
      }
    } else if (node.type === "redis-db" && node.connectionId && node.database) {
      const tabTitle = `${connectionStore.getConfig(node.connectionId)?.name || "Redis"}:db${node.database}`;
      queryStore.createTab(node.connectionId, node.database, tabTitle, "redis");
    } else if (node.type === "mongo-db" && node.connectionId && node.database) {
      await connectionStore.loadMongoCollections(node.connectionId, node.database);
    } else if (node.type === "mongo-collection" && node.connectionId && node.database) {
      const tabTitle = `${node.database}.${node.label}`;
      const tab = queryStore.createTab(node.connectionId, node.database, tabTitle, "mongo");
      queryStore.updateSql(tab, node.label);
    } else if (node.type === "database" && node.connectionId && node.database) {
      const config = connectionStore.getConfig(node.connectionId);
      if (config?.db_type === "sqlserver") {
        await connectionStore.loadSqlServerDatabaseObjects(node.connectionId, node.database);
      } else if (config?.db_type && TREE_SCHEMA_TYPES.has(config.db_type)) {
        await connectionStore.loadSchemas(node.connectionId, node.database);
      } else {
        await connectionStore.loadTables(node.connectionId, node.database);
      }
    } else if (node.type === "schema" && node.connectionId && node.database && node.schema) {
      await connectionStore.loadTables(node.connectionId, node.database, node.schema);
    } else if ((node.type === "table" || node.type === "view") && node.connectionId && node.database) {
      await connectionStore.loadTableGroups(node.connectionId, node.database, node.label, node.schema);
    } else if (node.type === "group-columns" && node.connectionId && node.database && node.tableName) {
      await connectionStore.loadColumns(node.connectionId, node.database, node.tableName, node.schema);
    } else if (node.type === "group-indexes" && node.connectionId && node.database && node.tableName) {
      await connectionStore.loadIndexes(node.connectionId, node.database, node.tableName, node.schema);
    } else if (node.type === "group-fkeys" && node.connectionId && node.database && node.tableName) {
      await connectionStore.loadForeignKeys(node.connectionId, node.database, node.tableName, node.schema);
    } else if (node.type === "group-triggers" && node.connectionId && node.database && node.tableName) {
      await connectionStore.loadTriggers(node.connectionId, node.database, node.tableName, node.schema);
    }
  } catch (e: any) {
    toast(t("connection.connectFailed", { message: e?.message || String(e) }), 5000);
  }
}

function onClick() {
  const node = props.node;
  const action = treeNodeRowAction(node.type, canExpand);
  if (action === "open-data") {
    openData();
  } else if (action === "toggle") {
    toggle();
  }
}

async function openData() {
  const node = props.node;
  if (!(node.type === "table" || node.type === "view") || !node.connectionId || !node.database) return;
  const config = connectionStore.getConfig(node.connectionId);
  const tabId = queryStore.createTab(node.connectionId, node.database, node.label, "data");
  queryStore.setExecuting(tabId, true);

  try {
    await connectionStore.ensureConnected(node.connectionId);
    if (!config) throw new Error("Connection config not found");

    const qualifiedName =
      isSchemaAware(config.db_type) && node.schema
        ? `${quoteIdent(node.schema)}.${quoteIdent(node.label)}`
        : quoteIdent(node.label);

    const querySchema = node.schema || node.database;
    const columns = await api.getColumns(node.connectionId, node.database, querySchema, node.label);
    const pks = columns.filter((c) => c.is_primary_key).map((c) => c.name);
    const order = pks.length ? ` ORDER BY ${pks.map((pk) => `${quoteIdent(pk)} ASC`).join(", ")}` : "";
    let sql: string;
    if (usesFetchFirst(config.db_type)) {
      sql = `SELECT * FROM ${qualifiedName}${order} FETCH FIRST 100 ROWS ONLY`;
    } else if (config.db_type === "sqlserver") {
      sql = `SELECT TOP 100 * FROM ${qualifiedName}${order}`;
    } else {
      sql = `SELECT * FROM ${qualifiedName}${order} LIMIT 100;`;
    }
    queryStore.updateSql(tabId, sql);
    queryStore.setTableMeta(tabId, {
      schema: node.schema,
      tableName: node.label,
      columns,
      primaryKeys: pks,
    });

    await queryStore.executeTabSql(tabId, sql);
  } catch (e: any) {
    queryStore.setErrorResult(tabId, e);
  }
}

async function newQuery() {
  const node = props.node;
  if (!node.connectionId) return;
  try {
    await connectionStore.ensureConnected(node.connectionId);
    connectionStore.activeConnectionId = node.connectionId;
    if (node.database) {
      queryStore.createTab(node.connectionId, node.database, undefined, "query");
      return;
    }
    const connection = connectionStore.getConfig(node.connectionId);
    if (!connection) return;
    const options = await getDatabaseOptions(node.connectionId);
    queryStore.createTab(node.connectionId, resolveDefaultDatabase(connection, options), undefined, "query");
  } catch (e: any) {
    toast(t("connection.connectFailed", { message: e?.message || String(e) }), 5000);
  }
}

async function setNodeAsDefaultDatabase() {
  const node = props.node;
  if (!node.connectionId || !node.database) return;
  try {
    await connectionStore.setDefaultDatabase(node.connectionId, node.database);
  } catch (e: any) {
    toast(t("connection.saveFailed", { message: e?.message || String(e) }), 5000);
  }
}

async function clearNodeDefaultDatabase() {
  const node = props.node;
  if (!node.connectionId) return;
  try {
    await connectionStore.clearDefaultDatabase(node.connectionId);
  } catch (e: any) {
    toast(t("connection.saveFailed", { message: e?.message || String(e) }), 5000);
  }
}

async function refresh() {
  try {
    await connectionStore.refreshTreeNode(props.node);
  } catch (e: any) {
    toast(t("connection.connectFailed", { message: e?.message || String(e) }), 5000);
  }
}

async function refreshExternalSnapshot() {
  if (!props.node.connectionId) return;
  try {
    await connectionStore.refreshExternalConnection(props.node.connectionId);
    toast(t("contextMenu.refreshExternalSnapshotSuccess"), 3000);
  } catch (e: any) {
    toast(t("contextMenu.refreshExternalSnapshotFailed", { message: e?.message || String(e) }), 5000);
  }
}

const showDeleteConfirm = ref(false);

function deleteConnection() {
  showDeleteConfirm.value = true;
}

async function confirmDelete() {
  const node = props.node;
  if (node.connectionId) {
    try {
      await connectionStore.disconnect(node.connectionId);
      await connectionStore.removeConnection(node.connectionId);
    } catch (e: any) {
      toast(t("connection.saveFailed", { message: e?.message || String(e) }), 5000);
    }
  }
}

function copyName() {
  navigator.clipboard.writeText(props.node.label);
}

async function duplicateConnection() {
  const connId = props.node.connectionId;
  if (!connId) return;
  const config = connectionStore.getConfig(connId);
  if (!config) return;
  const newConfig = { ...config, id: uuid(), name: `${config.name} (Copy)` };
  await connectionStore.addConnection(newConfig);
}

// --- Table Management Operations ---
const showDropTableConfirm = ref(false);
const showEmptyTableConfirm = ref(false);
const showTruncateTableConfirm = ref(false);
const showDuplicateDialog = ref(false);
const duplicateTableName = ref("");

const showCreateDatabaseDialog = ref(false);
const createDatabaseName = ref("");
const showDropDatabaseConfirm = ref(false);
const showCreateSchemaDialog = ref(false);
const createSchemaName = ref("");
const showDropSchemaConfirm = ref(false);

const isTableNotView = computed(() => props.node.type === "table");

const supportsTruncate = computed(() => {
  const dbType = currentDatabaseType();
  return dbType !== "sqlite" && dbType !== "duckdb";
});

const canCreateTable = computed(() => {
  const config = props.node.connectionId ? connectionStore.getConfig(props.node.connectionId) : undefined;
  return (
    (props.node.type === "database" || props.node.type === "schema") &&
    !!props.node.database &&
    !!config &&
    TABLE_STRUCTURE_SUPPORTED_TYPES.has(config.db_type)
  );
});

const canCreateDatabase = computed(() => {
  const config = props.node.connectionId ? connectionStore.getConfig(props.node.connectionId) : undefined;
  return props.node.type === "connection" && !!config && CREATE_DATABASE_SUPPORTED_TYPES.has(config.db_type);
});

const canDropDatabase = computed(() => {
  const config = props.node.connectionId ? connectionStore.getConfig(props.node.connectionId) : undefined;
  return props.node.type === "database" && !!config && CREATE_DATABASE_SUPPORTED_TYPES.has(config.db_type);
});

const canCreateSchema = computed(() => {
  const config = props.node.connectionId ? connectionStore.getConfig(props.node.connectionId) : undefined;
  return props.node.type === "database" && !!config && TREE_SCHEMA_TYPES.has(config.db_type);
});

const canDropSchema = computed(() => {
  const config = props.node.connectionId ? connectionStore.getConfig(props.node.connectionId) : undefined;
  return props.node.type === "schema" && !!config && TREE_SCHEMA_TYPES.has(config.db_type);
});

function buildDropTableSql(): string {
  return `DROP TABLE ${qualifiedTableName(props.node.label, props.node.schema)};`;
}

function buildEmptyTableSql(): string {
  return `DELETE FROM ${qualifiedTableName(props.node.label, props.node.schema)};`;
}

function buildTruncateTableSql(): string {
  const dbType = currentDatabaseType();
  const name = qualifiedTableName(props.node.label, props.node.schema);
  if (dbType === "sqlite" || dbType === "duckdb") return `DELETE FROM ${name};`;
  return `TRUNCATE TABLE ${name};`;
}

function dropTable() {
  showDropTableConfirm.value = true;
}

async function refreshTableList(node: TreeNode) {
  if (!node.connectionId || !node.database) return;
  const config = connectionStore.getConfig(node.connectionId);
  if (config?.db_type === "sqlserver" && node.schema?.toLowerCase() === "dbo") {
    await connectionStore.loadSqlServerDatabaseObjects(node.connectionId, node.database, { force: true });
  } else if (node.schema) {
    await connectionStore.loadTables(node.connectionId, node.database, node.schema, { force: true });
  } else {
    await connectionStore.loadTables(node.connectionId, node.database, undefined, { force: true });
  }
}

async function confirmDropTable() {
  const node = props.node;
  if (!node.connectionId || !node.database) return;
  try {
    await connectionStore.ensureConnected(node.connectionId);
    await api.executeQuery(node.connectionId, node.database, buildDropTableSql(), node.schema);
    toast(t("contextMenu.dropTableSuccess", { name: node.label }), 3000);
    await refreshTableList(node);
  } catch (e: any) {
    toast(t("contextMenu.tableOperationFailed", { message: e?.message || String(e) }), 5000);
  }
}

function emptyTable() {
  showEmptyTableConfirm.value = true;
}

async function confirmEmptyTable() {
  const node = props.node;
  if (!node.connectionId || !node.database) return;
  try {
    await connectionStore.ensureConnected(node.connectionId);
    await api.executeQuery(node.connectionId, node.database, buildEmptyTableSql(), node.schema);
    toast(t("contextMenu.emptyTableSuccess", { name: node.label }), 3000);
  } catch (e: any) {
    toast(t("contextMenu.tableOperationFailed", { message: e?.message || String(e) }), 5000);
  }
}

function truncateTable() {
  showTruncateTableConfirm.value = true;
}

async function confirmTruncateTable() {
  const node = props.node;
  if (!node.connectionId || !node.database) return;
  try {
    await connectionStore.ensureConnected(node.connectionId);
    await api.executeQuery(node.connectionId, node.database, buildTruncateTableSql(), node.schema);
    toast(t("contextMenu.truncateTableSuccess", { name: node.label }), 3000);
  } catch (e: any) {
    toast(t("contextMenu.tableOperationFailed", { message: e?.message || String(e) }), 5000);
  }
}

function buildDropDatabaseSql(): string {
  return `DROP DATABASE ${quoteIdent(props.node.label)};`;
}

function buildDropSchemaSql(): string {
  const dbType = currentDatabaseType();
  const name = quoteIdent(props.node.label);
  if (dbType === "postgres" || dbType === "gaussdb") return `DROP SCHEMA ${name} CASCADE;`;
  return `DROP SCHEMA ${name};`;
}

function openCreateDatabaseDialog() {
  createDatabaseName.value = "";
  showCreateDatabaseDialog.value = true;
}

async function confirmCreateDatabase() {
  const node = props.node;
  const name = createDatabaseName.value.trim();
  if (!name || !node.connectionId) return;
  showCreateDatabaseDialog.value = false;
  try {
    await connectionStore.ensureConnected(node.connectionId);
    const sql = `CREATE DATABASE ${quoteIdent(name)};`;
    await api.executeQuery(node.connectionId, "", sql);
    toast(t("contextMenu.createDatabaseSuccess", { name }), 3000);
    await connectionStore.loadDatabases(node.connectionId, { force: true });
  } catch (e: any) {
    toast(t("contextMenu.tableOperationFailed", { message: e?.message || String(e) }), 5000);
  }
}

function dropDatabase() {
  showDropDatabaseConfirm.value = true;
}

async function confirmDropDatabase() {
  const node = props.node;
  if (!node.connectionId) return;
  try {
    await connectionStore.ensureConnected(node.connectionId);
    await api.executeQuery(node.connectionId, "", buildDropDatabaseSql());
    toast(t("contextMenu.dropDatabaseSuccess", { name: node.label }), 3000);
    await connectionStore.loadDatabases(node.connectionId, { force: true });
  } catch (e: any) {
    toast(t("contextMenu.tableOperationFailed", { message: e?.message || String(e) }), 5000);
  }
}

function openCreateSchemaDialog() {
  createSchemaName.value = "";
  showCreateSchemaDialog.value = true;
}

async function confirmCreateSchema() {
  const node = props.node;
  const name = createSchemaName.value.trim();
  if (!name || !node.connectionId || !node.database) return;
  showCreateSchemaDialog.value = false;
  try {
    await connectionStore.ensureConnected(node.connectionId);
    const sql = `CREATE SCHEMA ${quoteIdent(name)};`;
    await api.executeQuery(node.connectionId, node.database, sql);
    toast(t("contextMenu.createSchemaSuccess", { name }), 3000);
    const config = connectionStore.getConfig(node.connectionId);
    if (config?.db_type === "sqlserver") {
      await connectionStore.loadSqlServerDatabaseObjects(node.connectionId, node.database, { force: true });
    } else {
      await connectionStore.loadSchemas(node.connectionId, node.database, { force: true });
    }
  } catch (e: any) {
    toast(t("contextMenu.tableOperationFailed", { message: e?.message || String(e) }), 5000);
  }
}

function dropSchema() {
  showDropSchemaConfirm.value = true;
}

async function confirmDropSchema() {
  const node = props.node;
  if (!node.connectionId || !node.database) return;
  try {
    await connectionStore.ensureConnected(node.connectionId);
    await api.executeQuery(node.connectionId, node.database, buildDropSchemaSql());
    toast(t("contextMenu.dropSchemaSuccess", { name: node.label }), 3000);
    const config = connectionStore.getConfig(node.connectionId);
    if (config?.db_type === "sqlserver") {
      await connectionStore.loadSqlServerDatabaseObjects(node.connectionId, node.database, { force: true });
    } else {
      await connectionStore.loadSchemas(node.connectionId, node.database, { force: true });
    }
  } catch (e: any) {
    toast(t("contextMenu.tableOperationFailed", { message: e?.message || String(e) }), 5000);
  }
}

function duplicateStructure() {
  duplicateTableName.value = `${props.node.label}_copy`;
  showDuplicateDialog.value = true;
}

async function confirmDuplicateStructure() {
  const node = props.node;
  const newName = duplicateTableName.value.trim();
  if (!newName || !node.connectionId || !node.database) return;
  showDuplicateDialog.value = false;
  try {
    await connectionStore.ensureConnected(node.connectionId);
    const dbType = currentDatabaseType();
    const source = qualifiedTableName(node.label, node.schema);
    const target = qualifiedTableName(newName, node.schema);
    let sql: string;
    if (dbType === "mysql") {
      sql = `CREATE TABLE ${target} LIKE ${source};`;
    } else if (dbType && PG_LIKE_STRUCTURE_TYPES.has(dbType)) {
      sql = `CREATE TABLE ${target} (LIKE ${source} INCLUDING ALL);`;
    } else if (dbType === "sqlserver") {
      sql = `SELECT TOP 0 * INTO ${target} FROM ${source};`;
    } else if (usesFetchFirst(dbType)) {
      sql = `CREATE TABLE ${target} AS SELECT * FROM ${source} WHERE 1=0`;
    } else {
      sql = `CREATE TABLE ${target} AS SELECT * FROM ${source} WHERE 0;`;
    }
    await api.executeQuery(node.connectionId, node.database, sql, node.schema);
    toast(t("contextMenu.duplicateStructureSuccess", { name: newName }), 3000);
    await refreshTableList(node);
  } catch (e: any) {
    toast(t("contextMenu.tableOperationFailed", { message: e?.message || String(e) }), 5000);
  }
}

function createTable() {
  const node = props.node;
  if (!node.connectionId || !node.database) return;
  connectionStore.structureEditorSource = {
    connectionId: node.connectionId,
    database: node.database,
    schema: node.schema,
    tableName: "",
  };
}

async function collectDatabaseExportTables(): Promise<Array<{ schema?: string; name: string; displayName: string }>> {
  const node = props.node;
  if (!node.connectionId || !node.database) return [];

  const config = connectionStore.getConfig(node.connectionId);
  if (node.type === "schema" && node.schema) {
    const tables = await api.listTables(node.connectionId, node.database, node.schema);
    return tables.map((table) => ({
      schema: node.schema,
      name: table.name,
      displayName: `${node.schema}.${table.name}`,
    }));
  }

  if (isSchemaAware(config?.db_type)) {
    const schemas = await api.listSchemas(node.connectionId, node.database);
    const groups = await Promise.all(
      schemas.map(async (schema) => {
        const tables = await api.listTables(node.connectionId!, node.database!, schema);
        return tables.map((table) => ({
          schema,
          name: table.name,
          displayName: `${schema}.${table.name}`,
        }));
      }),
    );
    return groups.flat();
  }

  const tables = await api.listTables(node.connectionId, node.database, node.database);
  return tables.map((table) => ({
    name: table.name,
    displayName: table.name,
  }));
}

async function fetchExportTableRows(
  connectionId: string,
  database: string,
  table: { schema?: string; name: string },
  databaseType?: DatabaseType,
): Promise<{ columns: QueryResult["columns"]; rows: QueryResult["rows"]; truncated: boolean }> {
  const rows: QueryResult["rows"] = [];
  let columns: QueryResult["columns"] = [];
  let offset = 0;

  while (rows.length < DATABASE_EXPORT_ROW_LIMIT) {
    const remaining = DATABASE_EXPORT_ROW_LIMIT - rows.length;
    const limit =
      databaseType === "sqlserver" ? DATABASE_EXPORT_ROW_LIMIT : Math.min(DATABASE_EXPORT_PAGE_SIZE, remaining);
    const sql = buildExportPageSql({
      databaseType,
      schema: table.schema,
      tableName: table.name,
      limit,
      offset: databaseType === "sqlserver" ? undefined : offset,
    });
    const result = await api.executeQuery(connectionId, database, sql);
    if (columns.length === 0) columns = result.columns;
    rows.push(...result.rows);

    if (result.rows.length < limit || databaseType === "sqlserver") break;
    offset += result.rows.length;
  }

  return {
    columns,
    rows,
    truncated: rows.length >= DATABASE_EXPORT_ROW_LIMIT,
  };
}

async function saveFileContent(content: string, defaultFileName: string, filterName: string, filterExt: string) {
  if (isTauriRuntime()) {
    const { save } = await import("@tauri-apps/plugin-dialog");
    const { writeTextFile } = await import("@tauri-apps/plugin-fs");
    const path = await save({
      defaultPath: defaultFileName,
      filters: [{ name: filterName, extensions: [filterExt] }],
    });
    if (path) await writeTextFile(path, content);
  } else {
    const blob = new Blob([content], { type: "text/plain" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = defaultFileName;
    a.click();
    URL.revokeObjectURL(url);
  }
}

async function exportDatabase() {
  const node = props.node;
  if (!(node.type === "database" || node.type === "schema") || !node.connectionId || !node.database) return;

  isExportingDatabase.value = true;
  try {
    await connectionStore.ensureConnected(node.connectionId);
    const config = connectionStore.getConfig(node.connectionId);
    const tables = await collectDatabaseExportTables();
    const exportedTables: ExportedTableSql[] = [];

    for (const table of tables) {
      const querySchema = table.schema || node.database;
      const qualifiedName = qualifiedTableName(table.name, table.schema);
      const ddl = await api.getTableDdl(node.connectionId, node.database, querySchema, table.name);
      const result = await fetchExportTableRows(node.connectionId, node.database, table, config?.db_type);
      exportedTables.push({
        displayName: table.displayName,
        qualifiedTableName: qualifiedName,
        ddl,
        columns: result.columns,
        rows: result.rows,
        truncated: result.truncated,
      });
    }

    const scopeName = node.type === "schema" && node.schema ? `${node.database}.${node.schema}` : node.database;
    const content = buildDatabaseSqlExport({
      databaseName: scopeName,
      tables: exportedTables,
      quoteIdentifier: quoteIdent,
      rowLimitPerTable: DATABASE_EXPORT_ROW_LIMIT,
    });

    await saveFileContent(content, `${safeFileName(scopeName)}.sql`, "SQL", "sql");
    toast(
      t("contextMenu.exportDatabaseSuccess", {
        count: exportedTables.length,
        limit: DATABASE_EXPORT_ROW_LIMIT,
      }),
      3000,
    );
  } catch (e: any) {
    console.error("Export database failed:", e);
    toast(t("contextMenu.exportDatabaseFailed", { message: e?.message || String(e) }), 5000);
  } finally {
    isExportingDatabase.value = false;
  }
}

async function exportStructure() {
  const node = props.node;
  if (!node.connectionId || !node.database) return;
  try {
    await connectionStore.ensureConnected(node.connectionId);
    const ddl = await api.getTableDdl(node.connectionId, node.database, node.schema || node.database, node.label);
    await saveFileContent(ddl + "\n", `${node.label}.sql`, "SQL", "sql");
  } catch (e: any) {
    console.error("Export structure failed:", e);
  }
}

async function exportData(format: "csv" | "json" | "sql") {
  const node = props.node;
  if (!node.connectionId || !node.database) return;
  const config = connectionStore.getConfig(node.connectionId);
  if (!config) return;

  try {
    await connectionStore.ensureConnected(node.connectionId);
    const qualifiedName =
      isSchemaAware(config.db_type) && node.schema
        ? `${quoteIdent(node.schema)}.${quoteIdent(node.label)}`
        : quoteIdent(node.label);
    const result = await api.executeQuery(node.connectionId, node.database, `SELECT * FROM ${qualifiedName}`);

    let content: string;
    let ext: string;

    if (format === "csv") {
      ext = "csv";
      const esc = (v: string) => `"${v.replace(/"/g, '""')}"`;
      const header = result.columns.map(esc).join(",");
      const body = result.rows.map((row) => row.map((c) => esc(c === null ? "" : String(c))).join(",")).join("\n");
      content = `${header}\n${body}`;
    } else if (format === "json") {
      ext = "json";
      const data = result.rows.map((row) => {
        const obj: Record<string, unknown> = {};
        result.columns.forEach((col, i) => {
          obj[col] = row[i];
        });
        return obj;
      });
      content = JSON.stringify(data, null, 2);
    } else {
      ext = "sql";
      const cols = result.columns.map((c) => quoteIdent(c)).join(", ");
      const lines = result.rows.map((row) => {
        const vals = row
          .map((v) => {
            if (v === null) return "NULL";
            if (typeof v === "number" || typeof v === "boolean") return String(v);
            return `'${String(v).replace(/'/g, "''")}'`;
          })
          .join(", ");
        return `INSERT INTO ${qualifiedName} (${cols}) VALUES (${vals});`;
      });
      content = lines.join("\n");
    }

    await saveFileContent(content, `${node.label}.${ext}`, ext.toUpperCase(), ext);
  } catch (e: any) {
    console.error("Export data failed:", e);
  }
}

function editConnection() {
  if (props.node.connectionId) {
    connectionStore.startEditing(props.node.connectionId);
  }
}

function disconnectConnection() {
  if (props.node.connectionId) {
    connectionStore.disconnect(props.node.connectionId);
    props.node.isExpanded = false;
    props.node.children = [];
  }
}

function openTransfer() {
  if (props.node.connectionId) {
    connectionStore.transferSource = {
      connectionId: props.node.connectionId,
      database: props.node.database ?? "",
    };
  }
}

function openSchemaDiff() {
  if (props.node.connectionId) {
    connectionStore.schemaDiffSource = {
      connectionId: props.node.connectionId,
      database: props.node.database ?? "",
    };
  }
}

function openSqlFileExecution() {
  if (props.node.connectionId) {
    connectionStore.sqlFileSource = {
      connectionId: props.node.connectionId,
      database: props.node.database ?? "",
    };
  }
}

function openDiagram() {
  const node = props.node;
  if (!node.connectionId || !node.database) return;
  connectionStore.diagramSource = {
    connectionId: node.connectionId,
    database: node.database,
    schema: node.schema,
    tableName: node.type === "table" ? node.label : undefined,
  };
}

function openDatabaseSearch() {
  const node = props.node;
  if (!node.connectionId || !node.database) return;
  connectionStore.databaseSearchSource = {
    connectionId: node.connectionId,
    database: node.database,
    schema: node.type === "schema" ? node.schema : undefined,
  };
}

function openTableImport() {
  const node = props.node;
  if (node.type !== "table" || !node.connectionId || !node.database) return;
  connectionStore.tableImportSource = {
    connectionId: node.connectionId,
    database: node.database,
    schema: node.schema,
    tableName: node.label,
  };
}

function openStructureEditor() {
  const node = props.node;
  if (node.type !== "table" || !node.connectionId || !node.database) return;
  connectionStore.structureEditorSource = {
    connectionId: node.connectionId,
    database: node.database,
    schema: node.schema,
    tableName: node.label,
  };
}

function openFieldLineage() {
  const node = props.node;
  const column = node.type === "column" && node.meta && "name" in node.meta ? node.meta.name : node.label;
  if (node.type !== "column" || !node.connectionId || !node.database || !node.tableName || !column) return;
  connectionStore.fieldLineageSource = {
    connectionId: node.connectionId,
    database: node.database,
    schema: node.schema,
    tableName: node.tableName,
    columnName: column,
  };
}

const canExpand = !leafTypes.has(props.node.type);
const canPin = computed(() => pinnableTypes.has(props.node.type));
const canOpenSqlFileExecution = computed(() => {
  const config = props.node.connectionId ? connectionStore.getConfig(props.node.connectionId) : undefined;
  return !!config && !SQL_FILE_UNSUPPORTED_TYPES.has(config.db_type);
});
const canOpenDiagram = computed(() => {
  const config = props.node.connectionId ? connectionStore.getConfig(props.node.connectionId) : undefined;
  return !!props.node.database && !!config && DIAGRAM_SUPPORTED_TYPES.has(config.db_type);
});
const canOpenDatabaseSearch = computed(() => {
  const config = props.node.connectionId ? connectionStore.getConfig(props.node.connectionId) : undefined;
  return !!props.node.database && !!config && DATABASE_SEARCH_SUPPORTED_TYPES.has(config.db_type);
});
const canOpenTableImport = computed(() => {
  const config = props.node.connectionId ? connectionStore.getConfig(props.node.connectionId) : undefined;
  return (
    props.node.type === "table" && !!props.node.database && !!config && TABLE_IMPORT_SUPPORTED_TYPES.has(config.db_type)
  );
});
const canOpenStructureEditor = computed(() => {
  const config = props.node.connectionId ? connectionStore.getConfig(props.node.connectionId) : undefined;
  return (
    props.node.type === "table" &&
    !!props.node.database &&
    !!config &&
    TABLE_STRUCTURE_SUPPORTED_TYPES.has(config.db_type)
  );
});
const canOpenFieldLineage = computed(() => {
  const config = props.node.connectionId ? connectionStore.getConfig(props.node.connectionId) : undefined;
  return (
    props.node.type === "column" &&
    !!props.node.database &&
    !!props.node.tableName &&
    !!config &&
    FIELD_LINEAGE_SUPPORTED_TYPES.has(config.db_type)
  );
});
const isPinned = computed(() => props.node.pinned || connectionStore.isTreeNodePinned(props.node.id));
const isNodeDefaultDatabase = computed(
  () =>
    (props.node.type === "database" || props.node.type === "redis-db" || props.node.type === "mongo-db") &&
    !!props.node.connectionId &&
    !!props.node.database &&
    connectionStore.isDefaultDatabase(props.node.connectionId, props.node.database),
);
const hasTypeMenu = computed(() => {
  const t = props.node.type;
  return (
    t === "connection" ||
    t === "database" ||
    t === "schema" ||
    t === "table" ||
    t === "view" ||
    t === "column" ||
    isGroupLabel(props.node)
  );
});
const columnComment = computed(() =>
  props.node.type === "column" && props.node.meta && "comment" in props.node.meta
    ? (props.node.meta as any).comment
    : null,
);
const paddingLeft = `${props.depth * 16 + 8}px`;
const isConnected = computed(
  () =>
    props.node.type === "connection" &&
    !!props.node.connectionId &&
    connectionStore.connectedIds.has(props.node.connectionId),
);
const canRefreshExternalSnapshot = computed(() => {
  const dbType = currentDatabaseType();
  return props.node.type === "connection" && !!dbType && EXTERNAL_TABULAR_TYPES.has(dbType);
});

function connectionIconType(connectionId?: string) {
  const config = connectionId ? connectionStore.getConfig(connectionId) : undefined;
  return config?.driver_profile || config?.db_type || "postgres";
}

const connectionColor = computed(() => {
  const connectionId = props.node.connectionId;
  return connectionId ? connectionStore.getConfig(connectionId)?.color || "" : "";
});

const CHILDREN_PAGE_SIZE = 100;
const displayLimit = ref(CHILDREN_PAGE_SIZE);

const visibleChildren = computed(() => {
  if (!props.node.children) return [];
  return props.node.children.slice(0, displayLimit.value);
});

const hasMoreChildren = computed(() => (props.node.children?.length ?? 0) > displayLimit.value);

const remainingCount = computed(() => (props.node.children?.length ?? 0) - displayLimit.value);

function togglePin() {
  connectionStore.toggleTreeNodePin(props.node.id);
}

async function showMore() {
  if ((props.node.children?.length ?? 0) > displayLimit.value) {
    displayLimit.value += CHILDREN_PAGE_SIZE;
  }
}

// --- Connection Group Management ---
const isRenamingGroup = ref(false);
const renameInput = ref("");

function startRenameGroup() {
  renameInput.value = props.node.label;
  isRenamingGroup.value = true;
  emit("rename-started");
}

watch(
  () => props.pendingRename,
  (val) => {
    if (val && props.node.type === "connection-group") {
      startRenameGroup();
    }
  },
  { immediate: true },
);

function finishRenameGroup() {
  isRenamingGroup.value = false;
  const trimmed = renameInput.value.trim();
  if (!trimmed) {
    connectionStore.deleteConnectionGroup(props.node.id);
    return;
  }
  if (trimmed !== props.node.label) {
    connectionStore.renameConnectionGroup(props.node.id, trimmed);
  }
}

function deleteConnectionGroup() {
  showDeleteGroupConfirm.value = true;
}

function newConnectionInGroup() {
  connectionStore.startCreatingConnectionInGroup(props.node.id);
}

function confirmDeleteGroup() {
  connectionStore.deleteConnectionGroup(props.node.id);
  showDeleteGroupConfirm.value = false;
}

const showDeleteGroupConfirm = ref(false);

function moveToGroup(groupId: string | null) {
  if (props.node.connectionId) {
    connectionStore.moveConnectionToGroup(props.node.connectionId, groupId);
  }
}

const showMoveToNewGroupDialog = ref(false);
const moveToNewGroupName = ref("");

function moveToNewGroup() {
  moveToNewGroupName.value = "";
  showMoveToNewGroupDialog.value = true;
}

function confirmMoveToNewGroup() {
  const name = moveToNewGroupName.value.trim();
  if (name && props.node.connectionId) {
    const groupId = connectionStore.createConnectionGroup(name);
    connectionStore.moveConnectionToGroup(props.node.connectionId, groupId);
  }
  showMoveToNewGroupDialog.value = false;
}

const availableGroups = computed(() => connectionStore.sidebarLayout.groups);

const currentGroupId = computed(() => {
  if (props.node.type !== "connection" || !props.node.connectionId) return null;
  for (const entry of connectionStore.sidebarLayout.order) {
    if (entry.type === "group" && entry.connectionIds.includes(props.node.connectionId)) {
      return entry.id;
    }
  }
  return null;
});

// --- Drag and Drop ---
import { useDragSort } from "@/composables/useDragSort";

const {
  state: dragState,
  startDrag,
  updateTarget,
  clearTarget,
} = useDragSort((draggedId, targetId, position) => connectionStore.reorderSidebarEntry(draggedId, targetId, position));

const isDraggable = computed(() => {
  if (props.dragDisabled) return false;
  return props.node.type === "connection" || props.node.type === "connection-group";
});

const isDropTarget = computed(() => props.node.type === "connection" || props.node.type === "connection-group");

const showDropBefore = computed(
  () => dragState.active && dragState.targetId === props.node.id && dragState.dropPosition === "before",
);
const showDropAfter = computed(
  () => dragState.active && dragState.targetId === props.node.id && dragState.dropPosition === "after",
);
const showDropInside = computed(
  () => dragState.active && dragState.targetId === props.node.id && dragState.dropPosition === "inside",
);
const isDragging = computed(() => dragState.active && dragState.draggedId === props.node.id);
</script>

<template>
  <ContextMenu>
    <ContextMenuTrigger as-child>
      <div>
        <div
          class="group flex min-w-0 items-center gap-1.5 py-1 px-2 rounded-sm cursor-pointer hover:bg-accent transition-colors relative"
          :class="{
            'ring-1 ring-primary/50 bg-primary/5': showDropInside,
            'opacity-50': isDragging,
          }"
          :style="{ paddingLeft }"
          @click="onClick"
          @mousedown="isDraggable ? startDrag($event, node.id, node.type) : undefined"
          @mousemove="isDropTarget ? updateTarget($event, node.id, node.type) : undefined"
          @mouseleave="clearTarget(node.id)"
        >
          <div
            v-if="showDropBefore"
            class="absolute right-2 top-0 h-0.5 bg-primary rounded-full pointer-events-none"
            :style="{ left: paddingLeft }"
          />
          <div
            v-if="showDropAfter"
            class="absolute right-2 bottom-0 h-0.5 bg-primary rounded-full pointer-events-none"
            :style="{ left: paddingLeft }"
          />
          <template v-if="canExpand">
            <button
              type="button"
              class="-m-0.5 flex h-4 w-4 shrink-0 items-center justify-center rounded-sm text-muted-foreground hover:bg-muted hover:text-foreground"
              @click.stop="toggle"
            >
              <Loader2 v-if="node.isLoading" class="w-3.5 h-3.5 animate-spin" />
              <ChevronDown v-else-if="node.isExpanded" class="w-3.5 h-3.5" />
              <ChevronRight v-else class="w-3.5 h-3.5" />
            </button>
          </template>
          <span v-else class="w-3.5 h-3.5 shrink-0" />
          <DatabaseIcon
            v-if="node.type === 'connection'"
            :db-type="connectionIconType(node.connectionId)"
            class="w-3.5 h-3.5 shrink-0"
          />
          <component
            v-else
            :is="getIconInfo(node)?.icon || Database"
            class="w-3.5 h-3.5 shrink-0"
            :class="getIconInfo(node)?.colorClass"
          />
          <span
            v-if="node.type === 'connection'"
            class="h-3 w-1.5 rounded-full shrink-0"
            :style="{ backgroundColor: connectionColor || '#9ca3af' }"
          />
          <input
            v-if="isRenamingGroup"
            v-model="renameInput"
            class="min-w-0 flex-1 truncate bg-transparent border border-primary/50 rounded px-1 text-xs outline-none"
            @blur="finishRenameGroup"
            @keydown.enter.prevent="finishRenameGroup"
            @keydown.escape.prevent="isRenamingGroup = false"
            @click.stop
            @vue:mounted="($event: any) => $event.el.focus()"
          />
          <span v-else class="min-w-0 flex-1 truncate">{{ isGroupLabel(node) ? t(node.label) : node.label }}</span>
          <Badge v-if="isNodeDefaultDatabase" variant="secondary" class="h-4 px-1.5 text-[10px]">
            {{ t("editor.defaultDatabase") }}
          </Badge>
          <span v-if="columnComment" class="truncate text-muted-foreground/60 text-[10px] max-w-[40%]">{{
            columnComment
          }}</span>
          <span
            v-if="
              node.type === 'connection' && node.connectionId && connectionStore.connectedIds.has(node.connectionId)
            "
            class="w-1.5 h-1.5 rounded-full bg-green-500 shrink-0"
          />
          <ConnectionErrorIndicator
            v-if="node.type === 'connection'"
            :connection-id="node.connectionId"
            trigger-class="h-4 w-4"
          />
          <button
            v-if="canPin"
            class="rounded p-0.5 text-muted-foreground hover:bg-muted-foreground/15 hover:text-foreground focus:opacity-100"
            :class="isPinned ? 'opacity-100 text-primary' : 'opacity-0 group-hover:opacity-100'"
            :title="isPinned ? t('contextMenu.unpin') : t('contextMenu.pin')"
            @click.stop="togglePin"
          >
            <Pin class="w-3 h-3" :class="{ 'fill-current': isPinned }" />
          </button>
        </div>
        <template v-if="node.isExpanded && node.children">
          <TreeItem
            v-for="child in visibleChildren"
            :key="child.id"
            :node="child"
            :depth="depth + 1"
            :drag-disabled="dragDisabled"
          />
          <div
            v-if="hasMoreChildren"
            class="flex items-center gap-1.5 py-1 px-2 cursor-pointer hover:bg-accent text-xs text-muted-foreground"
            :style="{ paddingLeft: `${(depth + 1) * 16 + 8}px` }"
            @click="showMore"
          >
            <Loader2 v-if="node.isLoading" class="w-3 h-3 shrink-0 animate-spin" />
            <span>{{ t("sidebar.showMore", { count: Math.min(CHILDREN_PAGE_SIZE, remainingCount) }) }}</span>
          </div>
        </template>
      </div>
    </ContextMenuTrigger>

    <ContextMenuContent class="w-auto min-w-36">
      <ContextMenuItem v-if="canPin" @click="togglePin">
        <Pin class="w-4 h-4" :class="{ 'fill-current': isPinned }" />
        {{ isPinned ? t("contextMenu.unpin") : t("contextMenu.pin") }}
      </ContextMenuItem>
      <ContextMenuSeparator v-if="canPin && hasTypeMenu" />

      <template v-if="node.type === 'connection'">
        <ContextMenuItem v-if="!isConnected" @click="toggle">
          <Plug class="w-4 h-4" /> {{ t("contextMenu.openConnection") }}
        </ContextMenuItem>
        <ContextMenuItem v-else @click="disconnectConnection">
          <Unplug class="w-4 h-4" /> {{ t("contextMenu.closeConnection") }}
        </ContextMenuItem>
        <ContextMenuItem @click="newQuery">
          <TerminalSquare class="w-4 h-4" /> {{ t("contextMenu.newQuery") }}
        </ContextMenuItem>
        <ContextMenuItem v-if="canOpenSqlFileExecution" @click="openSqlFileExecution">
          <FileCode class="w-4 h-4" /> {{ t("sqlFile.title") }}
        </ContextMenuItem>
        <ContextMenuItem v-if="canCreateDatabase" @click="openCreateDatabaseDialog">
          <Plus class="w-4 h-4" /> {{ t("contextMenu.createDatabase") }}
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuSub v-if="availableGroups.length > 0 || currentGroupId">
          <ContextMenuSubTrigger>
            <FolderInput class="w-4 h-4" /> {{ t("connectionGroup.moveToGroup") }}
          </ContextMenuSubTrigger>
          <ContextMenuSubContent>
            <ContextMenuItem
              v-for="group in availableGroups"
              :key="group.id"
              :disabled="group.id === currentGroupId"
              @click="moveToGroup(group.id)"
            >
              <FolderOpen class="w-4 h-4" /> {{ group.name }}
            </ContextMenuItem>
            <ContextMenuSeparator v-if="currentGroupId" />
            <ContextMenuItem v-if="currentGroupId" @click="moveToGroup(null)">
              {{ t("connectionGroup.ungrouped") }}
            </ContextMenuItem>
            <ContextMenuSeparator />
            <ContextMenuItem @click="moveToNewGroup">
              <FolderPlus class="w-4 h-4" /> {{ t("connectionGroup.newGroup") }}
            </ContextMenuItem>
          </ContextMenuSubContent>
        </ContextMenuSub>
        <ContextMenuItem v-else @click="moveToNewGroup">
          <FolderPlus class="w-4 h-4" /> {{ t("connectionGroup.moveToNewGroup") }}
        </ContextMenuItem>
        <ContextMenuItem @click="refresh">
          <RefreshCw class="w-4 h-4" /> {{ t("contextMenu.refreshChildren") }}
        </ContextMenuItem>
        <ContextMenuItem v-if="canRefreshExternalSnapshot" @click="refreshExternalSnapshot">
          <RefreshCw class="w-4 h-4" /> {{ t("contextMenu.refreshExternalSnapshot") }}
        </ContextMenuItem>
        <ContextMenuItem @click="editConnection">
          <Pencil class="w-4 h-4" /> {{ t("contextMenu.editConnection") }}
        </ContextMenuItem>
        <ContextMenuItem @click="duplicateConnection">
          <CopyPlus class="w-4 h-4" /> {{ t("contextMenu.duplicateConnection") }}
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem class="text-destructive" @click="deleteConnection">
          <Trash2 class="w-4 h-4" /> {{ t("contextMenu.deleteConnection") }}
        </ContextMenuItem>
      </template>

      <template v-if="node.type === 'connection-group'">
        <ContextMenuItem @click="newConnectionInGroup">
          <Plus class="w-4 h-4" /> {{ t("toolbar.newConnection") }}
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem @click="startRenameGroup">
          <Pencil class="w-4 h-4" /> {{ t("connectionGroup.renameGroup") }}
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem class="text-destructive" @click="deleteConnectionGroup">
          <Trash2 class="w-4 h-4" /> {{ t("connectionGroup.deleteGroup") }}
        </ContextMenuItem>
      </template>

      <template v-if="node.type === 'database' || node.type === 'schema'">
        <ContextMenuItem @click="newQuery">
          <TerminalSquare class="w-4 h-4" /> {{ t("contextMenu.newQuery") }}
        </ContextMenuItem>
        <ContextMenuItem v-if="node.type === 'database' && !isNodeDefaultDatabase" @click="setNodeAsDefaultDatabase">
          <Database class="w-4 h-4" /> {{ t("contextMenu.setDefaultDatabase") }}
        </ContextMenuItem>
        <ContextMenuItem v-if="node.type === 'database' && isNodeDefaultDatabase" @click="clearNodeDefaultDatabase">
          <Database class="w-4 h-4" /> {{ t("contextMenu.clearDefaultDatabase") }}
        </ContextMenuItem>
        <ContextMenuItem v-if="canCreateTable" @click="createTable">
          <Plus class="w-4 h-4" /> {{ t("contextMenu.createTable") }}
        </ContextMenuItem>
        <ContextMenuItem v-if="canCreateSchema" @click="openCreateSchemaDialog">
          <Plus class="w-4 h-4" /> {{ t("contextMenu.createSchema") }}
        </ContextMenuItem>
        <ContextMenuItem v-if="canOpenSqlFileExecution" @click="openSqlFileExecution">
          <FileCode class="w-4 h-4" /> {{ t("sqlFile.title") }}
        </ContextMenuItem>
        <ContextMenuItem v-if="canOpenDiagram" @click="openDiagram">
          <Network class="w-4 h-4" /> {{ t("diagram.open") }}
        </ContextMenuItem>
        <ContextMenuItem v-if="canOpenDatabaseSearch" @click="openDatabaseSearch">
          <Search class="w-4 h-4" /> {{ t("databaseSearch.open") }}
        </ContextMenuItem>
        <ContextMenuItem @click="refresh">
          <RefreshCw class="w-4 h-4" /> {{ t("contextMenu.refreshChildren") }}
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem @click="openTransfer">
          <ArrowRightLeft class="w-4 h-4" /> {{ t("transfer.dataTransfer") }}
        </ContextMenuItem>
        <ContextMenuItem @click="openSchemaDiff">
          <ArrowRightLeft class="w-4 h-4" /> {{ t("diff.title") }}
        </ContextMenuItem>
        <ContextMenuItem :disabled="isExportingDatabase" @click="exportDatabase">
          <Loader2 v-if="isExportingDatabase" class="w-4 h-4 animate-spin" />
          <Download v-else class="w-4 h-4" />
          {{ t("contextMenu.exportDatabase") }}
        </ContextMenuItem>
        <template v-if="canDropDatabase || canDropSchema">
          <ContextMenuSeparator />
          <ContextMenuItem v-if="canDropDatabase" class="text-destructive" @click="dropDatabase">
            <Trash2 class="w-4 h-4" /> {{ t("contextMenu.dropDatabase") }}
          </ContextMenuItem>
          <ContextMenuItem v-if="canDropSchema" class="text-destructive" @click="dropSchema">
            <Trash2 class="w-4 h-4" /> {{ t("contextMenu.dropSchema") }}
          </ContextMenuItem>
        </template>
      </template>

      <template v-if="node.type === 'redis-db' || node.type === 'mongo-db'">
        <ContextMenuItem @click="newQuery">
          <TerminalSquare class="w-4 h-4" /> {{ t("contextMenu.newQuery") }}
        </ContextMenuItem>
        <ContextMenuItem v-if="!isNodeDefaultDatabase" @click="setNodeAsDefaultDatabase">
          <Database class="w-4 h-4" /> {{ t("contextMenu.setDefaultDatabase") }}
        </ContextMenuItem>
        <ContextMenuItem v-if="isNodeDefaultDatabase" @click="clearNodeDefaultDatabase">
          <Database class="w-4 h-4" /> {{ t("contextMenu.clearDefaultDatabase") }}
        </ContextMenuItem>
      </template>

      <template v-if="node.type === 'table' || node.type === 'view'">
        <ContextMenuItem @click="openData">
          <TableProperties class="w-4 h-4" /> {{ t("contextMenu.viewData") }}
        </ContextMenuItem>
        <ContextMenuItem v-if="canOpenStructureEditor" @click="openStructureEditor">
          <PencilRuler class="w-4 h-4" /> {{ t("contextMenu.editStructure") }}
        </ContextMenuItem>
        <ContextMenuItem @click="newQuery">
          <TerminalSquare class="w-4 h-4" /> {{ t("contextMenu.newQuery") }}
        </ContextMenuItem>
        <ContextMenuItem v-if="canOpenDiagram" @click="openDiagram">
          <Network class="w-4 h-4" /> {{ t("diagram.open") }}
        </ContextMenuItem>
        <ContextMenuItem v-if="canOpenTableImport" @click="openTableImport">
          <FileUp class="w-4 h-4" /> {{ t("contextMenu.importData") }}
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuSub>
          <ContextMenuSubTrigger>
            <Download class="w-4 h-4" /> {{ t("contextMenu.exportData") }}
          </ContextMenuSubTrigger>
          <ContextMenuSubContent>
            <ContextMenuItem @click="exportData('csv')">CSV</ContextMenuItem>
            <ContextMenuItem @click="exportData('json')">JSON</ContextMenuItem>
            <ContextMenuItem @click="exportData('sql')">SQL INSERT</ContextMenuItem>
          </ContextMenuSubContent>
        </ContextMenuSub>
        <ContextMenuItem @click="exportStructure">
          <FileCode class="w-4 h-4" /> {{ t("contextMenu.exportStructure") }}
        </ContextMenuItem>
        <template v-if="isTableNotView">
          <ContextMenuSeparator />
          <ContextMenuItem @click="duplicateStructure">
            <CopyPlus class="w-4 h-4" /> {{ t("contextMenu.duplicateStructure") }}
          </ContextMenuItem>
          <ContextMenuSeparator />
          <ContextMenuItem v-if="supportsTruncate" class="text-destructive" @click="truncateTable">
            <Scissors class="w-4 h-4" /> {{ t("contextMenu.truncateTable") }}
          </ContextMenuItem>
          <ContextMenuItem class="text-destructive" @click="emptyTable">
            <Eraser class="w-4 h-4" /> {{ t("contextMenu.emptyTable") }}
          </ContextMenuItem>
          <ContextMenuItem class="text-destructive" @click="dropTable">
            <Trash2 class="w-4 h-4" /> {{ t("contextMenu.dropTable") }}
          </ContextMenuItem>
        </template>
        <ContextMenuSeparator />
        <ContextMenuItem @click="refresh">
          <RefreshCw class="w-4 h-4" /> {{ t("contextMenu.refreshChildren") }}
        </ContextMenuItem>
      </template>

      <template v-if="node.type === 'column'">
        <ContextMenuItem v-if="canOpenFieldLineage" @click="openFieldLineage">
          <Network class="w-4 h-4" /> {{ t("lineage.open") }}
        </ContextMenuItem>
      </template>

      <template v-if="isGroupLabel(node)">
        <ContextMenuItem @click="refresh">
          <RefreshCw class="w-4 h-4" /> {{ t("contextMenu.refreshChildren") }}
        </ContextMenuItem>
      </template>

      <template v-if="node.type !== 'connection'">
        <ContextMenuSeparator v-if="hasTypeMenu" />
        <ContextMenuItem @click="copyName"> <Copy class="w-4 h-4" /> {{ t("contextMenu.copyName") }} </ContextMenuItem>
      </template>
    </ContextMenuContent>
  </ContextMenu>

  <Dialog v-model:open="showDeleteConfirm">
    <DialogContent class="sm:max-w-[400px]">
      <DialogHeader>
        <DialogTitle>{{ t("contextMenu.confirmDeleteTitle") }}</DialogTitle>
      </DialogHeader>
      <p class="text-sm text-muted-foreground">
        {{ t("contextMenu.confirmDeleteMessage", { name: node.label }) }}
      </p>
      <DialogFooter>
        <Button variant="outline" @click="showDeleteConfirm = false">{{ t("dangerDialog.cancel") }}</Button>
        <Button
          variant="destructive"
          @click="
            showDeleteConfirm = false;
            confirmDelete();
          "
          >{{ t("contextMenu.deleteConnection") }}</Button
        >
      </DialogFooter>
    </DialogContent>
  </Dialog>

  <Dialog v-model:open="showMoveToNewGroupDialog">
    <DialogContent class="sm:max-w-[360px]">
      <DialogHeader>
        <DialogTitle>{{ t("connectionGroup.createGroup") }}</DialogTitle>
      </DialogHeader>
      <Input
        v-model="moveToNewGroupName"
        :placeholder="t('connectionGroup.groupNamePlaceholder')"
        @keydown.enter.prevent="confirmMoveToNewGroup"
      />
      <DialogFooter>
        <Button variant="outline" @click="showMoveToNewGroupDialog = false">{{ t("dangerDialog.cancel") }}</Button>
        <Button :disabled="!moveToNewGroupName.trim()" @click="confirmMoveToNewGroup">{{
          t("connectionGroup.createGroup")
        }}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>

  <Dialog v-model:open="showDeleteGroupConfirm">
    <DialogContent class="sm:max-w-[400px]">
      <DialogHeader>
        <DialogTitle>{{ t("connectionGroup.deleteGroupConfirmTitle") }}</DialogTitle>
      </DialogHeader>
      <p class="text-sm text-muted-foreground">
        {{ t("connectionGroup.deleteGroupConfirmMessage", { name: node.label }) }}
      </p>
      <DialogFooter>
        <Button variant="outline" @click="showDeleteGroupConfirm = false">{{ t("dangerDialog.cancel") }}</Button>
        <Button variant="destructive" @click="confirmDeleteGroup">{{ t("connectionGroup.deleteGroup") }}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>

  <DangerConfirmDialog
    v-model:open="showDropTableConfirm"
    :title="t('contextMenu.confirmDropTableTitle')"
    :message="t('contextMenu.confirmDropTableMessage', { name: node.label })"
    :sql="buildDropTableSql()"
    :confirm-label="t('contextMenu.dropTable')"
    @confirm="confirmDropTable"
  />

  <DangerConfirmDialog
    v-model:open="showEmptyTableConfirm"
    :title="t('contextMenu.confirmEmptyTableTitle')"
    :message="t('contextMenu.confirmEmptyTableMessage', { name: node.label })"
    :sql="buildEmptyTableSql()"
    :confirm-label="t('contextMenu.emptyTable')"
    @confirm="confirmEmptyTable"
  />

  <DangerConfirmDialog
    v-model:open="showTruncateTableConfirm"
    :title="t('contextMenu.confirmTruncateTableTitle')"
    :message="t('contextMenu.confirmTruncateTableMessage', { name: node.label })"
    :sql="buildTruncateTableSql()"
    :confirm-label="t('contextMenu.truncateTable')"
    @confirm="confirmTruncateTable"
  />

  <Dialog v-model:open="showDuplicateDialog">
    <DialogContent class="sm:max-w-[400px]">
      <DialogHeader>
        <DialogTitle>{{ t("contextMenu.duplicateNameTitle") }}</DialogTitle>
      </DialogHeader>
      <Input
        v-model="duplicateTableName"
        :placeholder="t('contextMenu.duplicateNamePlaceholder')"
        @keydown.enter.prevent="confirmDuplicateStructure"
      />
      <DialogFooter>
        <Button variant="outline" @click="showDuplicateDialog = false">{{ t("dangerDialog.cancel") }}</Button>
        <Button :disabled="!duplicateTableName.trim()" @click="confirmDuplicateStructure">{{
          t("dangerDialog.confirm")
        }}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>

  <Dialog v-model:open="showCreateDatabaseDialog">
    <DialogContent class="sm:max-w-[400px]">
      <DialogHeader>
        <DialogTitle>{{ t("contextMenu.createDatabase") }}</DialogTitle>
      </DialogHeader>
      <Input
        v-model="createDatabaseName"
        :placeholder="t('contextMenu.createDatabaseNamePlaceholder')"
        @keydown.enter.prevent="confirmCreateDatabase"
      />
      <DialogFooter>
        <Button variant="outline" @click="showCreateDatabaseDialog = false">{{ t("dangerDialog.cancel") }}</Button>
        <Button :disabled="!createDatabaseName.trim()" @click="confirmCreateDatabase">{{
          t("dangerDialog.confirm")
        }}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>

  <DangerConfirmDialog
    v-model:open="showDropDatabaseConfirm"
    :title="t('contextMenu.confirmDropDatabaseTitle')"
    :message="t('contextMenu.confirmDropDatabaseMessage', { name: node.label })"
    :sql="buildDropDatabaseSql()"
    :confirm-label="t('contextMenu.dropDatabase')"
    @confirm="confirmDropDatabase"
  />

  <Dialog v-model:open="showCreateSchemaDialog">
    <DialogContent class="sm:max-w-[400px]">
      <DialogHeader>
        <DialogTitle>{{ t("contextMenu.createSchema") }}</DialogTitle>
      </DialogHeader>
      <Input
        v-model="createSchemaName"
        :placeholder="t('contextMenu.createSchemaNamePlaceholder')"
        @keydown.enter.prevent="confirmCreateSchema"
      />
      <DialogFooter>
        <Button variant="outline" @click="showCreateSchemaDialog = false">{{ t("dangerDialog.cancel") }}</Button>
        <Button :disabled="!createSchemaName.trim()" @click="confirmCreateSchema">{{
          t("dangerDialog.confirm")
        }}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>

  <DangerConfirmDialog
    v-model:open="showDropSchemaConfirm"
    :title="t('contextMenu.confirmDropSchemaTitle')"
    :message="t('contextMenu.confirmDropSchemaMessage', { name: node.label })"
    :sql="buildDropSchemaSql()"
    :confirm-label="t('contextMenu.dropSchema')"
    @confirm="confirmDropSchema"
  />
</template>
