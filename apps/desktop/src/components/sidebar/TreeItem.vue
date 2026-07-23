<script setup lang="ts">
import { ref, computed, inject, shallowRef, watch, onBeforeUnmount } from "vue";
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
  TableProperties,
  Key,
  Link,
  Zap,
  ListTree,
  FileCode,
  Network,
  Server,
  Pin,
  Search,
  Plus,
  ScrollText,
  Braces,
  Package,
  Check,
  UsersRound,
  CalendarClock,
  Lock,
  Archive,
  Square,
  X,
} from "@lucide/vue";
import { useConnectionStore } from "@/stores/connectionStore";
import { useQueryStore } from "@/stores/queryStore";
import { useSettingsStore } from "@/stores/settingsStore";
import { useToast } from "@/composables/useToast";
import { useDatabaseOptions } from "@/composables/useDatabaseOptions";
import type { ColumnInfo, ConnectionConfig, DatabaseType, TreeNode, TreeNodeType } from "@/types/database";
import * as api from "@/lib/api";
import { uuid } from "@/lib/utils";
import { resolveDefaultDatabase } from "@/lib/defaultDatabase";
import { canTreeNodeShowExpander, treeItemPaddingLeft, usesFullWidthTreeLabel } from "@/lib/sidebarTreeItemLayout";
import { buildTableSelectSql } from "@/lib/tableSelectSql";
import { buildTableDeleteTemplate, buildTableInsertTemplate, buildTableSelectTemplate, buildTableUpdateTemplate } from "@/lib/tableSqlTemplates";
import { connectionFilePath, defaultSqliteBackupFileName, isMemorySqlitePath, sqliteBackupSourcePath } from "@/lib/connectionFile";
import { revealPathInFileManager } from "@/lib/tauri";
import { clearActiveTableReferencePayload, createTableReferencePayload, createTableReferenceDropEvent, setActiveTableReferencePayload, type QueryEditorTableReferencePayload } from "@/lib/queryEditorTableDrop";
import { editablePrimaryKeys, usesSyntheticRowIdKey } from "@/lib/tableEditing";
import {
  EXTERNAL_TABULAR_TYPES,
  supportsDatabaseCreation,
  supportsDatabaseSearch,
  supportsFieldLineage,
  supportsObjectBrowserTreeNode,
  supportsSchemaDiagram,
  supportsSqlFileExecution,
  supportsTableImport,
  supportsTableTruncate,
  supportsTableStructureEditing,
  usesTreeSchemaMode,
} from "@/lib/databaseCapabilities";
import { copyNameForTreeNode, objectSourceKindForTreeNode, sidebarSelectionCopyAction, treeNodeRowAction, treeNodeRowDoubleClickAction } from "@/lib/treeNodeClick";
import { formatSqlInsert } from "@/lib/exportFormats";
import { fetchTableDataForExport } from "@/lib/tableDataExport";
import { buildCreateDatabaseSql, buildDuckDbAttachDatabaseSql, duckDbAttachedDatabaseNameFromPath, supportsCreateDatabaseCharset, uniqueDuckDbAttachedDatabaseName } from "@/lib/createDatabaseSql";
import {
  buildCreateSchemaSql,
  buildDropDatabaseSql,
  buildDropObjectSql,
  buildDropSchemaSql,
  buildDropTableSql,
  buildDropTableChildObjectSql,
  buildDuplicateTableStructureSql,
  buildEmptyTableSql,
  buildTruncateTableSql,
  type DropTableChildObjectSqlOptions,
  type DropObjectSqlOptions,
  type TableChildObjectType,
  type TableAdminSqlOptions,
} from "@/lib/dbAdminSql";
import { buildRenameObjectSql, supportsObjectRename, type RenameableObjectType } from "@/lib/objectRenameSql";
import { buildRoutineRenameObjectSourceStatements, supportsSourceBackedRoutineRename } from "@/lib/objectSourceEditor";
import { buildViewDdl } from "@/lib/viewDdl";
import DdlViewDialog from "@/components/objects/DdlViewDialog.vue";
import { getTableStructureCapabilities } from "@/lib/tableStructureCapabilities";
import { codeMirrorSqlDialect, connectionObjectTreeNodeSchema, connectionObjectTreeQuerySchema, connectionUsesDatabaseObjectTreeMode, effectiveDatabaseTypeForConnection, tableStructureDatabaseTypeForConnection } from "@/lib/jdbcDialect";
import { hexToRgba } from "@/lib/color";
import { focusSidebarRenameInput } from "@/lib/sidebarRenameFocus";
import { hasTreeNodeDatabaseContext } from "@/lib/treeNodeContext";
import { sidebarDisplayTableName } from "@/lib/sidebarTableNameDisplay";
import { selectedTreeNodesInVisibleOrder as orderSelectedTreeNodes, treeSelectionRangeIdsByIndex, treeSelectionRangeIds } from "@/lib/sidebarTreeSelection";
import { selectedConnectionDeleteTargets } from "@/lib/sidebarConnectionSelection";
import { supportsDatabaseUserAdmin } from "@/lib/databaseUserAdmin";
import { sidebarTreeContextKey } from "@/lib/sidebarTreeContext";
import DangerConfirmDialog from "@/components/editor/DangerConfirmDialog.vue";
import ProcedureExecutionDialog from "@/components/objects/ProcedureExecutionDialog.vue";
import { useExportTracker, type ExportTask } from "@/composables/useExportTracker";
import { isTauriRuntime } from "@/lib/tauriRuntime";
import { copyToClipboard } from "@/lib/clipboard";
import { hasEnabledTransportLayers } from "@/lib/connectionTransport";
import { formatShortcut } from "@/lib/shortcutRegistry";
import { rankSavedSqlHistory, type SavedSqlHistoryScope } from "@/lib/savedSqlHistory";
import { isSqlServerLinkedNode } from "@/lib/sqlServerLinkedServers";
import DatabaseIcon from "@/components/icons/DatabaseIcon.vue";
import ConnectionErrorIndicator from "@/components/connection/ConnectionErrorIndicator.vue";
import ProductionContextBadge from "@/components/common/ProductionContextBadge.vue";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import LightTooltip from "@/components/ui/LightTooltip.vue";
import type { ColumnInfo, ConnectionConfig, DatabaseType, TreeNode, TreeNodeType } from "@/types/database";
import { canTreeNodeShowExpander, sidebarTreeNodeComment, trailingCommentAvailableWidth, trailingCommentGapPx, treeItemPaddingLeft, treeLabelWidthClass, usesFullWidthTreeLabel } from "@/lib/sidebar/sidebarTreeItemLayout";
import { clearActiveTableReferencePayload, createTableReferencePayload, createTableReferenceDropEvent, setActiveTableReferencePayload, type QueryEditorTableReferencePayload } from "@/lib/editor/queryEditorTableDrop";
import { formatSidebarObjectStorage } from "@/lib/sidebar/sidebarDatabaseStorage";
import { dataTabOpenModeFromTreeClick } from "@/lib/sidebar/dataTabOpenPolicy";
import { effectiveDatabaseTypeForConnection } from "@/lib/database/jdbcDialect";
import { hexToRgba } from "@/lib/common/color";
import { sidebarDisplayTableName } from "@/lib/sidebar/sidebarTableNameDisplay";
import { shouldMeasureSidebarLabelOverflow } from "@/lib/sidebar/sidebarLabelTooltip";
import { treeSelectionRangeIdsByIndex, treeSelectionRangeIds } from "@/lib/sidebar/sidebarTreeSelection";
import { isSidebarDatabaseOpened } from "@/lib/sidebar/sidebarDatabaseOpenState";
import { sidebarTreeContextKey } from "@/lib/sidebar/sidebarTreeContext";
import { isWindows } from "@/lib/backend/platform";
import { flattenTree } from "@/composables/useFlatTree";
import { productionContextForDatabase } from "@/lib/database/productionSafety";
import { focusSidebarRenameInput } from "@/lib/sidebar/sidebarRenameFocus";
// --- Drag and Drop ---
import { useDragSort } from "@/composables/useDragSort";
import { sidebarTreeRuntimeKey } from "@/lib/sidebar/sidebarTreeRuntime";

const { t } = useI18n();

const labelRef = ref<HTMLElement>();

const rowRef = ref<HTMLElement>();

const trailingCommentLayoutRef = ref<HTMLElement>();

const trailingCommentLeadingRef = ref<HTMLElement>();

const trailingCommentMaxWidth = ref(0);

const labelOverflowing = ref(false);

let labelResizeObserver: ResizeObserver | null = null;

let trailingCommentResizeObserver: ResizeObserver | null = null;

let labelMeasureFrame = 0;

let trailingCommentMeasureFrame = 0;

function cancelLabelOverflowMeasure() {
  if (!labelMeasureFrame) return;
  window.cancelAnimationFrame(labelMeasureFrame);
  labelMeasureFrame = 0;
}

function measureLabelOverflow(): boolean {
  const el = labelRef.value;
  if (!el || !shouldMeasureLabelOverflow()) return false;
  const style = window.getComputedStyle(el);
  if (style.overflowX === "visible" || style.textOverflow !== "ellipsis") return false;
  return el.scrollWidth - el.clientWidth > 2;
}

function updateLabelOverflow() {
  labelOverflowing.value = measureLabelOverflow();
}

function scheduleLabelOverflowMeasure() {
  if (typeof window === "undefined") {
    updateLabelOverflow();
    return;
  }
  cancelLabelOverflowMeasure();
  // Keep synchronous layout reads out of the hover path; they are expensive in
  // large virtualized sidebar trees, especially on Linux WebKitGTK without GPU help.
  labelMeasureFrame = window.requestAnimationFrame(() => {
    labelMeasureFrame = 0;
    updateLabelOverflow();
  });
}

function handleMouseEnter() {
  if (!shouldMeasureLabelOverflow()) {
    labelOverflowing.value = false;
    return;
  }
  updateLabelOverflow();
  if (typeof ResizeObserver !== "undefined" && labelRef.value && !labelResizeObserver) {
    labelResizeObserver = new ResizeObserver(scheduleLabelOverflowMeasure);
    labelResizeObserver.observe(labelRef.value);
  }
}

function handleMouseLeave() {
  labelResizeObserver?.disconnect();
  labelResizeObserver = null;
  cancelLabelOverflowMeasure();
}

const connectionStore = useConnectionStore();

const queryStore = useQueryStore();

const settingsStore = useSettingsStore();

const { toast } = useToast();

const useWindowsSidebarCommentFont = isWindows();

const props = defineProps<{
  node: TreeNode;
  depth: number;
  dragDisabled?: boolean;
  pendingRename?: boolean;
  highlighted?: boolean;
  commentLabelWidth?: number;
}>();

const emit = defineEmits<{
  "rename-started": [];
  "group-created": [groupId: string];
  "context-menu": [event: MouseEvent, node: TreeNode];
}>();

const sidebarTreeRuntime = inject(sidebarTreeRuntimeKey);
if (!sidebarTreeRuntime) throw new Error("TreeItem must be rendered inside ConnectionTree");
const treeRuntime = sidebarTreeRuntime;
const sidebarTreeContext = inject(sidebarTreeContextKey, null);

const stopPasteHandlerRegistration = watch(
  () => props.node.id,
  (nodeId, _previousNodeId, onCleanup) => {
    const unregister = sidebarTreeContext?.registerPasteHandler?.(nodeId, () => treeRuntime.requestPaste(props.node));
    if (unregister) onCleanup(unregister);
  },
  { immediate: true },
);

const activeNode = shallowRef<TreeNode>(props.node);

const showProductionBadge = computed(() => {
  const connectionId = activeNode.value.connectionId;
  const context = productionContextForDatabase(connectionId ? connectionStore.getConfig(connectionId) : undefined, activeNode.value.database);
  return context.active && ["connection", "database", "redis-db", "mongo-db"].includes(activeNode.value.type);
});

function currentDatabaseType(): DatabaseType | undefined {
  return activeNode.value.connectionId ? effectiveDatabaseTypeForConnection(connectionStore.getConfig(activeNode.value.connectionId)) : undefined;
}

function getIconInfo(node: TreeNode): { icon: any; colorClass: string } | null {
  switch (node.type) {
    case "connection":
      return null;
    case "connection-group":
      return { icon: node.isExpanded ? FolderOpen : FolderClosed, colorClass: "text-amber-500" };
    case "database":
      return { icon: Database, colorClass: "text-yellow-500" };
    case "linked-server-root":
      return { icon: Network, colorClass: "text-blue-500" };
    case "linked-server":
      return { icon: Server, colorClass: "text-blue-400" };
    case "linked-server-catalog":
      return { icon: Database, colorClass: "text-yellow-500" };
    case "linked-server-schema":
      return { icon: FolderOpen, colorClass: "text-sky-400" };
    case "schema":
      return { icon: FolderOpen, colorClass: "text-sky-400" };
    case "table":
      return { icon: Table, colorClass: "text-green-500" };
    case "view":
      return { icon: Eye, colorClass: "text-purple-500" };
    case "materialized_view":
      return { icon: Eye, colorClass: "text-indigo-500" };
    case "column":
      if ((node.meta as ColumnInfo).is_primary_key) {
        return { icon: Columns3, colorClass: "text-orange-400" };
      } else {
        return { icon: Columns3, colorClass: "text-muted-foreground" };
      }
    case "group-columns":
      return { icon: ListTree, colorClass: "text-green-400" };
    case "group-indexes":
      return { icon: Key, colorClass: "text-amber-500" };
    case "group-fkeys":
      return { icon: Link, colorClass: "text-blue-400" };
    case "group-triggers":
      return { icon: Zap, colorClass: "text-orange-400" };
    case "object-browser":
      return { icon: TableProperties, colorClass: "text-primary" };
    case "user-admin":
      return { icon: UsersRound, colorClass: "text-primary" };
    case "dameng-job-admin":
      return { icon: CalendarClock, colorClass: "text-primary" };
    case "index":
      return { icon: Key, colorClass: "text-amber-400" };
    case "fkey":
      return { icon: Link, colorClass: "text-blue-300" };
    case "trigger":
      return { icon: Zap, colorClass: "text-orange-300" };
    case "redis-db":
      return { icon: Database, colorClass: "text-red-400" };
    case "mq-tenant":
      return { icon: FolderOpen, colorClass: "text-sky-400" };
    case "nacos-namespace":
      return { icon: FolderOpen, colorClass: "text-sky-500" };
    case "etcd-root":
      return { icon: Database, colorClass: "text-sky-500" };
    case "zookeeper-root":
      return { icon: Database, colorClass: "text-blue-500" };
    case "mongo-db":
      return { icon: Database, colorClass: "text-yellow-500" };
    case "mongo-gridfs":
    case "mongo-buckets":
      return { icon: Archive, colorClass: "text-cyan-500" };
    case "mongo-bucket":
      return { icon: Archive, colorClass: "text-cyan-400" };
    case "mongo-collection":
      return { icon: Table, colorClass: "text-green-400" };
    case "vector-collection":
      return { icon: TableProperties, colorClass: "text-cyan-400" };
    case "elasticsearch-index":
      return { icon: Table, colorClass: "text-emerald-400" };
    case "procedure":
      return { icon: ScrollText, colorClass: "text-blue-500" };
    case "function":
      return { icon: Braces, colorClass: "text-amber-500" };
    case "sequence":
      return { icon: ListTree, colorClass: "text-emerald-500" };
    case "package":
      return { icon: Package, colorClass: "text-cyan-500" };
    case "package-body":
      return { icon: FileCode, colorClass: "text-cyan-400" };
    case "type":
      return { icon: Braces, colorClass: "text-violet-500" };
    case "type-body":
      return { icon: FileCode, colorClass: "text-violet-400" };
    case "group-tables":
      return { icon: Table, colorClass: "text-green-500" };
    case "group-views":
      return { icon: Eye, colorClass: "text-purple-500" };
    case "group-materialized-views":
      return { icon: Eye, colorClass: "text-indigo-500" };
    case "group-procedures":
      return { icon: ScrollText, colorClass: "text-blue-500" };
    case "group-functions":
      return { icon: Braces, colorClass: "text-amber-500" };
    case "group-sequences":
      return { icon: ListTree, colorClass: "text-emerald-500" };
    case "group-packages":
      return { icon: Package, colorClass: "text-cyan-500" };
    case "group-types":
      return { icon: Braces, colorClass: "text-violet-500" };
    case "group-partitions":
      return { icon: node.isExpanded ? FolderOpen : FolderClosed, colorClass: "text-green-400" };
    case "group-extensions":
      return { icon: Package, colorClass: "text-violet-500" };
    case "extension":
      return { icon: Package, colorClass: "text-violet-400" };
    case "load-more":
      return { icon: Plus, colorClass: "text-primary" };
    default:
      return { icon: Database, colorClass: "text-muted-foreground" };
  }
}

const groupTypes: Set<TreeNodeType> = new Set([
  "group-columns",
  "group-indexes",
  "group-fkeys",
  "group-triggers",
  "group-tables",
  "group-views",
  "group-materialized-views",
  "group-procedures",
  "group-functions",
  "group-sequences",
  "group-packages",
  "group-types",
  "group-partitions",
  "group-extensions",
]);

function isGroupLabel(node: TreeNode): boolean {
  return groupTypes.has(node.type);
}

function displayLabel(node: TreeNode): string {
  if (node.type === "load-more") return t(node.label);
  if (node.type === "object-browser") return t(node.label, { count: node.objectCount ?? 0 });
  if (node.type === "user-admin" || node.type === "dameng-job-admin") return t(node.label);
  if (node.type === "linked-server-root") return t(node.label);
  if (node.label === "tree.defaultDatabase") return t(node.label);
  return isGroupLabel(node) ? t(node.label) : node.label;
}

function visibleLabel(node: TreeNode): string {
  const withValidity = (label: string) => (node.valid === false ? `${label} · INVALID` : label);
  if (node.type === "table" || node.type === "view" || node.type === "materialized_view" || node.type === "mongo-collection" || node.type === "vector-collection" || node.type === "elasticsearch-index") {
    return withValidity(sidebarDisplayTableName(node.label, settingsStore.editorSettings.sidebarHiddenTablePrefixes));
  }
  return withValidity(displayLabel(node));
}

type DetailTooltipRow = {
  label: string;
  value: string;
  multiline?: boolean;
};

function cleanTooltipValue(value: string | number | null | undefined): string {
  return String(value ?? "").trim();
}

function isLocalFileConnection(config: Pick<ConnectionConfig, "db_type" | "port">): boolean {
  return config.db_type === "sqlite" || config.db_type === "duckdb" || config.db_type === "access" || (config.db_type === "h2" && config.port === 0);
}

function redactedConnectionString(value: string): string {
  return value.replace(/(:\/\/[^/\s:@?#;]+):([^@\s/?#;]+)@/g, "$1:***@").replace(/([?&;](?:password|pwd|pass|token|secret|key)=)[^&;]*/gi, "$1***");
}

function connectionTooltipScheme(config: Pick<ConnectionConfig, "db_type" | "ssl">): string {
  switch (config.db_type) {
    case "postgres":
    case "gaussdb":
    case "kwdb":
    case "yashandb":
    case "redshift":
    case "questdb":
      return "postgresql";
    case "sqlserver":
      return "mssql";
    case "elasticsearch":
    case "qdrant":
    case "milvus":
    case "weaviate":
    case "chromadb":
    case "rqlite":
    case "turso":
    case "mq":
      return config.ssl ? "https" : "http";
    case "cloudflare-d1":
      return "https";
    case "dameng":
      return "dm";
    default:
      return config.db_type;
  }
}

function hostForDisplay(host: string): string {
  if (!host.includes(":") || host.startsWith("[") || host.includes("://")) return host;
  return `[${host}]`;
}

function connectionTooltipUrl(config: ConnectionConfig): string {
  const explicit = cleanTooltipValue(config.connection_string);
  if (explicit) return redactedConnectionString(explicit);

  const host = cleanTooltipValue(config.host);
  if (!host) return "";
  if (host.includes("://")) return redactedConnectionString(host);

  if (isLocalFileConnection(config)) {
    if (config.db_type === "access") return `jdbc:ucanaccess://${host}`;
    return `${config.db_type}://${host}`;
  }

  const scheme = connectionTooltipScheme(config);
  const port = Number(config.port) > 0 ? `:${config.port}` : "";
  const user = cleanTooltipValue(config.username);
  const userInfo = user ? `${encodeURIComponent(user)}@` : "";
  const database = cleanTooltipValue(config.database);
  const path = database ? `/${encodeURIComponent(database)}` : "";
  const params = cleanTooltipValue(config.url_params);
  const query = params ? (params.startsWith("?") ? params : `?${params}`) : "";
  return redactedConnectionString(`${scheme}://${userInfo}${hostForDisplay(host)}${port}${path}${query}`);
}

const detailTooltip = computed(() => {
  const node = activeNode.value;
  if (node.type === "connection" && node.connectionId) {
    const config = connectionStore.getConfig(node.connectionId);
    if (!config) return null;
    const hostLabel = isLocalFileConnection(config) ? t("connection.filePath") : t("connection.host");
    const rows: DetailTooltipRow[] = [
      { label: t("connection.name"), value: cleanTooltipValue(config.name) },
      { label: "URL", value: connectionTooltipUrl(config), multiline: true },
      { label: hostLabel, value: cleanTooltipValue(config.host), multiline: isLocalFileConnection(config) },
      { label: "Port", value: Number(config.port) > 0 ? String(config.port) : "" },
      { label: t("connection.database"), value: cleanTooltipValue(config.database) },
      { label: t("connection.user"), value: cleanTooltipValue(config.username) },
      { label: t("connection.type"), value: config.driver_label || config.driver_profile || config.db_type },
      { label: t("connection.databaseInfo.productVersion"), value: cleanTooltipValue(config.database_info?.productVersion) },
    ].filter((row) => row.value);
    return { rows };
  }
  const comment = node.type === "column" && node.meta && "comment" in node.meta ? (node.meta as ColumnInfo).comment : node.comment;
  if (!comment || (node.type !== "schema" && node.type !== "table" && node.type !== "view" && node.type !== "column")) return null;
  const rows: DetailTooltipRow[] = [
    { label: t("connection.name"), value: visibleLabel(node) },
    { label: t("structureEditor.comment"), value: cleanTooltipValue(comment), multiline: true },
  ].filter((row) => row.value);
  return { rows };
});

function isTooltipDisabled(): boolean {
  if (detailTooltip.value?.rows.length) return isRenamingGroup.value;
  return isRenamingGroup.value || !labelOverflowing.value;
}

function visibleTreeNodes(): TreeNode[] {
  if (sidebarTreeContext) return sidebarTreeContext.getVisibleNodes();
  return flattenTree(connectionStore.treeNodes).map((item) => item.node);
}

function selectSingleTreeNode(node: TreeNode) {
  // Re-clicking the selected row should not replace the selection array and
  // force visible tree rows to recompute.
  if (!connectionStore.connectionMultiSelectActive && connectionStore.selectedTreeNodeId === node.id && connectionStore.treeSelectionAnchorId === node.id && connectionStore.selectedTreeNodeIds.length === 1 && connectionStore.selectedTreeNodeIds[0] === node.id) {
    return;
  }
  connectionStore.connectionMultiSelectActive = false;
  connectionStore.selectedTreeNodeId = node.id;
  connectionStore.selectedTreeNodeIds = [node.id];
  connectionStore.treeSelectionAnchorId = node.id;
}

function toggleTreeNodeSelection(node: TreeNode) {
  connectionStore.connectionMultiSelectActive = false;
  const ids = new Set(connectionStore.selectedTreeNodeIds);
  if (ids.has(node.id)) ids.delete(node.id);
  else ids.add(node.id);
  connectionStore.selectedTreeNodeIds = ids.size ? [...ids] : [node.id];
  connectionStore.selectedTreeNodeId = node.id;
  connectionStore.treeSelectionAnchorId = node.id;
}

function selectTreeNodeRange(node: TreeNode) {
  connectionStore.connectionMultiSelectActive = false;
  const visible = visibleTreeNodes();
  const anchorId = connectionStore.treeSelectionAnchorId || connectionStore.selectedTreeNodeId || node.id;
  const currentIndex = sidebarTreeContext ? sidebarTreeContext.getVisibleNodeIndex(node.id) : -1;
  const anchorIndex = sidebarTreeContext ? sidebarTreeContext.getVisibleNodeIndex(anchorId) : -1;

  if (sidebarTreeContext && currentIndex >= 0 && anchorIndex >= 0) {
    connectionStore.selectedTreeNodeIds = treeSelectionRangeIdsByIndex(visible, currentIndex, anchorIndex, node.id);
    connectionStore.selectedTreeNodeId = node.id;
    return;
  }

  if (!visible.some((item) => item.id === anchorId) || !visible.some((item) => item.id === node.id)) {
    selectSingleTreeNode(node);
    return;
  }

  const rangeIds = treeSelectionRangeIds(visible, node.id, anchorId, connectionStore.selectedTreeNodeId);
  connectionStore.selectedTreeNodeIds = rangeIds;
  connectionStore.selectedTreeNodeId = node.id;
}

function selectedConnectionIdsForAction(): string[] {
  const connectionIds = new Set(connectionStore.connections.map((connection) => connection.id));
  return connectionStore.selectedTreeNodeIds.filter((id) => connectionIds.has(id));
}

const isConnectionSelectionChecked = computed(() => {
  if (!connectionStore.connectionMultiSelectActive || activeNode.value.type !== "connection" || !activeNode.value.connectionId) return false;
  return connectionStore.selectedTreeNodeIds.includes(activeNode.value.connectionId);
});

function toggleConnectionMultiSelection(event: MouseEvent) {
  event.preventDefault();
  event.stopPropagation();
  if (activeNode.value.type !== "connection" || !activeNode.value.connectionId) return;

  // Keep connection-id normalization off the row render path; this handler only
  // runs when the checkbox is clicked, while the checked state updates often.
  const next = new Set(connectionStore.connectionMultiSelectActive ? selectedConnectionIdsForAction() : []);
  if (next.has(activeNode.value.connectionId)) next.delete(activeNode.value.connectionId);
  else next.add(activeNode.value.connectionId);

  const ids = [...next];
  connectionStore.selectedTreeNodeIds = ids;
  connectionStore.selectedTreeNodeId = ids.includes(activeNode.value.connectionId) ? activeNode.value.connectionId : (ids[0] ?? null);
  connectionStore.treeSelectionAnchorId = activeNode.value.connectionId;
  connectionStore.connectionMultiSelectActive = ids.length > 0;
  rowRef.value?.focus({ preventScroll: true });
}

async function cancelConnectionAttempt() {
  if (!activeNode.value.connectionId) return;
  try {
    const cancelled = await connectionStore.cancelConnecting(activeNode.value.connectionId);
    if (cancelled) toast(t("connection.connectCancelled"), 2000);
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
    toast(t("connection.connectFailed", { message: translateBackendError(t, e?.message || String(e)) }), 5000);
    if (e?.message?.includes("driver is not installed") || (e?.message?.includes("JRE") && e?.message?.includes("not installed"))) {
      window.dispatchEvent(new Event("dbx-open-driver-store"));
    }
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

function connectionDeleteTargets() {
  return selectedConnectionDeleteTargets(props.node, selectedTreeNodesInVisibleOrder());
}

function connectionDeleteMenuLabel(): string {
  const count = connectionDeleteTargets().length;
  return count > 1 ? t("contextMenu.deleteSelectedConnections", { count }) : t("contextMenu.deleteConnection");
}

function connectionDeleteConfirmMessage(): string {
  const targets = connectionDeleteTargets();
  return targets.length > 1 ? t("contextMenu.confirmDeleteSelectedMessage", { count: targets.length }) : t("contextMenu.confirmDeleteMessage", { name: props.node.label });
}

function deleteConnection() {
  if (!connectionDeleteTargets().length) return;
  showDeleteConfirm.value = true;
}

async function confirmDelete() {
  const targets = connectionDeleteTargets();
  if (!targets.length) return;
  const connectionIds = targets.map((target) => target.connectionId);
  try {
    await connectionStore.removeConnections(connectionIds);
    for (const connectionId of connectionIds) {
      connectionStore.disconnect(connectionId).catch((error) => {
        console.warn("[DBX][connection:delete:disconnect-failed]", { connectionId, error });
      });
    }
    toast(targets.length > 1 ? t("connection.deletedSelected", { count: targets.length }) : t("connection.deleted"), 2000);
  } catch (e: any) {
    toast(t("connection.saveFailed", { message: e?.message || String(e) }), 5000);
  }
}

async function copyName() {
  updateTreeClipboardForNodes([props.node]);
  try {
    await copyToClipboard(copyNameForTreeNode(props.node));
    toast(t("connection.copied"), 2000);
  } catch (e: any) {
    toast(t("grid.copyFailed", { message: e?.message || String(e) }), 5000);
  }
}

async function copyFinalProxyPort() {
  const connectionId = props.node.connectionId;
  const config = connectionId ? connectionStore.getConfig(connectionId) : undefined;
  if (!config || !hasEnabledTransportLayers(config)) return;

  try {
    const port = await api.connectionFinalProxyPort(config);
    await copyToClipboard(String(port));
    toast(t("contextMenu.finalProxyPortCopied", { port }), 2000);
  } catch (e: any) {
    toast(t("grid.copyFailed", { message: translateBackendError(t, e?.message || String(e)) }), 5000);
  }
}

async function copySelectedNames() {
  const selectedNodes = selectedTreeNodesInVisibleOrder();
  const nodes = selectedNodes.length > 1 && selectedNodes.some((node) => node.id === props.node.id) ? selectedNodes : [props.node];
  updateTreeClipboardForNodes(nodes);
  try {
    await copyToClipboard(nodes.map(copyNameForTreeNode).join("\n"));
    toast(t("connection.copied"), 2000);
  } catch (e: any) {
    toast(t("grid.copyFailed", { message: e?.message || String(e) }), 5000);
  }
}

function updateTreeClipboardForNodes(nodes: TreeNode[]) {
  const tableNodes = nodes.filter((node): node is DuplicateStructureSource => node.type === "table" && !!node.connectionId && !!node.database && typeof node.label === "string");
  if (nodes.length !== 1 || tableNodes.length !== 1) {
    connectionStore.treeClipboard = null;
    return;
  }
  const table = tableNodes[0]!;
  connectionStore.treeClipboard = {
    kind: "table-structure",
    connectionId: table.connectionId,
    database: table.database,
    schema: table.schema,
    tableName: table.label,
  };
}

async function duplicateConnection() {
  const connId = props.node.connectionId;
  if (!connId) return;
  const config = connectionStore.getConfig(connId);
  if (!config) return;
  const newConfig = { ...config, id: uuid(), name: `${config.name} (Copy)` };
  await connectionStore.addConnection(newConfig);
  toast(t("connection.duplicated"), 2000);
}

// --- Table Management Operations ---
const showDropTableConfirm = ref(false);
const showDropTableChildObjectConfirm = ref(false);
const showBatchDropConfirm = ref(false);
const showStructurePreviewDialog = ref(false);
const showStructureDocCopyDialog = ref(false);
const structurePreviewSql = ref("");
const structurePreviewTitle = ref("");
const structurePreviewDefaultFileName = ref("structure.sql");
const structurePreviewError = ref("");
const structureDocCopyText = ref("");
const structureDocCopyTitle = ref("");
const isLoadingStructurePreview = ref(false);
const showEmptyTableConfirm = ref(false);
const showTruncateTableConfirm = ref(false);
const showRenameObjectDialog = ref(false);
const renameObjectName = ref("");
const renameObjectError = ref("");
const renameObjectPreviewSql = ref("");
const dropTablePreviewSql = ref("");
const emptyTablePreviewSql = ref("");
const truncateTablePreviewSql = ref("");
const dropObjectPreviewSql = ref("");
const dropTableChildObjectPreviewSql = ref("");
const batchDropPreviewSql = ref("");
const dropDatabasePreviewSql = ref("");
const dropSchemaPreviewSql = ref("");
const showDuplicateDialog = ref(false);
const duplicateTableName = ref("");
const duplicateStructureSource = ref<DuplicateStructureSource | null>(null);

const ddlTarget = ref<TreeNode | null>(null);
const showDdlDialog = ref(false);
const ddlDialect = computed(() => {
  if (!ddlTarget.value?.connectionId) return "mysql";
  return codeMirrorSqlDialect(effectiveDatabaseTypeForConnection(connectionStore.getConfig(ddlTarget.value.connectionId)));
});
const showCreateDatabaseDialog = ref(false);
const createDatabaseName = ref("");
const createDatabaseCharset = ref("utf8mb4");
const createDatabaseCollation = ref("utf8mb4_unicode_ci");
const showDropDatabaseConfirm = ref(false);
const dropDatabaseLoading = ref(false);
const showFlushRedisDbConfirm = ref(false);
const showCreateSchemaDialog = ref(false);
const createSchemaName = ref("");
const showDropSchemaConfirm = ref(false);

// --- Procedure / Function Management ---
const showDropObjectConfirm = ref(false);
const showProcedureExecutionConfirm = ref(false);

function dropObjectSqlOptions(): DropObjectSqlOptions | null {
  return dropObjectSqlOptionsForNode(props.node);
}

function dropObjectSqlOptionsForNode(node: TreeNode): DropObjectSqlOptions | null {
  if (node.type !== "view" && node.type !== "materialized_view" && node.type !== "procedure" && node.type !== "function") return null;
  return {
    databaseType: tableStructureDatabaseTypeForNode(node),
    objectType: node.type === "view" ? "VIEW" : node.type === "materialized_view" ? "MATERIALIZED_VIEW" : node.type === "procedure" ? "PROCEDURE" : "FUNCTION",
    schema: node.schema,
    name: node.label,
  };
}

function tableChildDropObjectType(type: TreeNodeType): TableChildObjectType | null {
  if (type === "column") return "COLUMN";
  if (type === "index") return "INDEX";
  if (type === "fkey") return "FOREIGN_KEY";
  if (type === "trigger") return "TRIGGER";
  return null;
}

function tableChildDropObjectName(node: TreeNode): string {
  if (node.type === "column") return node.meta && "name" in node.meta ? node.meta.name : node.label.replace(/\s+\(.+\)$/, "");
  if (node.type === "index") return node.meta && "name" in node.meta ? node.meta.name : node.label.replace(/\s+\(.+\)$/, "");
  if (node.type === "fkey") return node.meta && "name" in node.meta ? node.meta.name : node.label;
  if (node.type === "trigger") return node.meta && "name" in node.meta ? node.meta.name : node.label.replace(/\s+\(.+\)$/, "");
  return node.label;
}

function dropTableChildObjectSqlOptions(): DropTableChildObjectSqlOptions | null {
  return dropTableChildObjectSqlOptionsForNode(props.node);
}

function dropTableChildObjectSqlOptionsForNode(node: TreeNode): DropTableChildObjectSqlOptions | null {
  const objectType = tableChildDropObjectType(node.type);
  if (!objectType || !node.tableName) return null;
  const name = tableChildDropObjectName(node).trim();
  if (!name) return null;
  return {
    databaseType: databaseTypeForNode(node),
    objectType,
    schema: node.schema,
    tableName: node.tableName,
    name,
  };
}

const canDropTableChildObject = computed(() => {
  return canDropTableChildObjectNode(props.node);
});

function canDropTableChildObjectNode(node: TreeNode): boolean {
  const options = dropTableChildObjectSqlOptionsForNode(node);
  if (!options) return false;
  const capabilities = getTableStructureCapabilities(options.databaseType);
  if (options.objectType === "COLUMN") return capabilities.dropColumn;
  if (options.objectType === "INDEX") return capabilities.dropIndex;
  return true;
}

function dropObjectMenuLabel(): string {
  if (props.node.type === "view") return t("contextMenu.dropView");
  if (props.node.type === "materialized_view") return t("contextMenu.dropView");
  if (props.node.type === "procedure") return t("contextMenu.dropProcedure");
  if (props.node.type === "function") return t("contextMenu.dropFunction");
  return t("contextMenu.dropObject");
}

function dropObjectConfirmTitle(): string {
  if (props.node.type === "view") return t("contextMenu.confirmDropViewTitle");
  if (props.node.type === "materialized_view") return t("contextMenu.confirmDropViewTitle");
  if (props.node.type === "procedure") return t("contextMenu.confirmDropProcedureTitle");
  if (props.node.type === "function") return t("contextMenu.confirmDropFunctionTitle");
  return t("contextMenu.confirmDropObjectTitle");
}

function dropObjectConfirmMessage(): string {
  if (props.node.type === "view") return t("contextMenu.confirmDropViewMessage", { name: props.node.label });
  if (props.node.type === "materialized_view") return t("contextMenu.confirmDropViewMessage", { name: props.node.label });
  if (props.node.type === "procedure") return t("contextMenu.confirmDropProcedureMessage", { name: props.node.label });
  if (props.node.type === "function") return t("contextMenu.confirmDropFunctionMessage", { name: props.node.label });
  return t("contextMenu.confirmDropObjectMessage", { name: props.node.label });
}

function dropTableChildObjectMenuLabel(): string {
  if (props.node.type === "column") return t("contextMenu.dropColumn");
  if (props.node.type === "index") return t("contextMenu.dropIndex");
  if (props.node.type === "fkey") return t("contextMenu.dropForeignKey");
  if (props.node.type === "trigger") return t("contextMenu.dropTrigger");
  return t("contextMenu.dropObject");
}

function dropTableChildObjectConfirmTitle(): string {
  if (props.node.type === "column") return t("contextMenu.confirmDropColumnTitle");
  if (props.node.type === "index") return t("contextMenu.confirmDropIndexTitle");
  if (props.node.type === "fkey") return t("contextMenu.confirmDropForeignKeyTitle");
  if (props.node.type === "trigger") return t("contextMenu.confirmDropTriggerTitle");
  return t("contextMenu.confirmDropObjectTitle");
}

function dropTableChildObjectConfirmMessage(): string {
  return t("contextMenu.confirmDropTableChildObjectMessage", {
    name: tableChildDropObjectName(props.node),
    table: props.node.tableName || "",
  });
}

async function refreshDropObjectPreviewSql() {
  const options = dropObjectSqlOptions();
  dropObjectPreviewSql.value = "";
  dropObjectPreviewSql.value = options ? await buildDropObjectSql(options).catch(() => "") : "";
}

async function refreshDropTableChildObjectPreviewSql() {
  const options = dropTableChildObjectSqlOptions();
  dropTableChildObjectPreviewSql.value = "";
  dropTableChildObjectPreviewSql.value = options ? await buildDropTableChildObjectSql(options).catch(() => "") : "";
}

function viewObjectSource() {
  const node = props.node;
  if (!node.connectionId || !node.database) return;
  const objectType = objectSourceKindForTreeNode(node.type);
  if (!objectType) return;
  const schema = node.schema || node.database;
  connectionStore
    .ensureConnected(node.connectionId)
    .then(() => {
      connectionStore.activeConnectionId = node.connectionId!;
      return api.getObjectSource(node.connectionId!, node.database!, schema, node.label, objectType as any);
    })
    .then(async (result) => {
      const tabId = queryStore.createTab(node.connectionId!, node.database!, `Source - ${node.label}`);
      queryStore.updateSql(tabId, result.source);
      if (objectType !== "SEQUENCE") {
        queryStore.setObjectSource(tabId, {
          schema,
          name: node.label,
          objectType,
        });
      }
      queryStore.markTabClean(queryStore.tabs.find((tab) => tab.id === tabId));
    })
    .catch((e: any) => {
      toast(e?.message || String(e), 5000);
    });
}

function viewObjectDdl() {
  const node = props.node;
  if ((node.type !== "view" && node.type !== "materialized_view") || !node.connectionId || !node.database) return;
  const schema = node.schema || node.database;
  const objectType = node.type === "materialized_view" ? "MATERIALIZED_VIEW" : "VIEW";
  connectionStore
    .ensureConnected(node.connectionId)
    .then(() => {
      connectionStore.activeConnectionId = node.connectionId!;
      return api.getObjectSource(node.connectionId!, node.database!, schema, node.label, objectType);
    })
    .then(async (result) => {
      const connection = connectionStore.getConfig(node.connectionId!);
      const ddl = await buildViewDdl({
        databaseType: effectiveDatabaseTypeForConnection(connection),
        schema,
        name: node.label,
        source: result.source,
      });
      const tabId = queryStore.createTab(node.connectionId!, node.database!, `DDL - ${node.label}`);
      queryStore.updateSql(tabId, ddl);
    })
    .catch((e: any) => {
      toast(e?.message || String(e), 5000);
    });
}

function openProcedureExecution() {
  const node = props.node;
  if (node.type !== "procedure" || !node.connectionId || !node.database) return;
  showProcedureExecutionConfirm.value = true;
}

function openProcedureExecutionSql(sql: string) {
  const node = props.node;
  if (node.type !== "procedure" || !node.connectionId || !node.database || !sql) return;
  const tabId = queryStore.createTab(node.connectionId, node.database, `Execute - ${node.label}`, "query", node.schema);
  queryStore.updateSql(tabId, sql);
}

async function executeProcedureSql(sql: string) {
  const node = props.node;
  if (node.type !== "procedure" || !node.connectionId || !node.database || !sql) return;
  const tabId = queryStore.createTab(node.connectionId, node.database, `Execute - ${node.label}`, "query", node.schema);
  queryStore.updateSql(tabId, sql);
  await queryStore.executeTabSql(tabId, sql);
}

function requestDropObject() {
  void refreshDropObjectPreviewSql();
  showDropObjectConfirm.value = true;
}

function requestDropTableChildObject() {
  if (!canDropTableChildObject.value) return;
  void refreshDropTableChildObjectPreviewSql();
  showDropTableChildObjectConfirm.value = true;
}

function canDropTreeNode(node: TreeNode): boolean {
  if (isSqlServerLinkedNode(node)) return false;
  if (node.type === "table") return !!node.connectionId && !!node.database;
  if (node.type === "view" || node.type === "materialized_view" || node.type === "procedure" || node.type === "function") {
    return !!node.connectionId && !!node.database && !!dropObjectSqlOptionsForNode(node);
  }
  return canDropTableChildObjectNode(node);
}

function selectedBatchDropTargets(): TreeNode[] {
  const selected = selectedTreeNodesInVisibleOrder();
  if (selected.length <= 1 || !selected.some((node) => node.id === props.node.id)) return [];
  const first = selected[0];
  if (!first?.connectionId || !first.database || !selected.every((node) => node.type === first.type)) return [];
  if (!selected.every((node) => node.connectionId === first.connectionId && node.database === first.database && canDropTreeNode(node))) {
    return [];
  }
  return selected;
}

function batchDropMenuLabel(): string {
  return t("contextMenu.batchDrop", { count: selectedBatchDropTargets().length });
}

function batchDropConfirmTitle(): string {
  return t("contextMenu.confirmBatchDropTitle", { count: selectedBatchDropTargets().length });
}

function batchDropConfirmMessage(): string {
  return t("contextMenu.confirmBatchDropMessage", { count: selectedBatchDropTargets().length });
}

async function dropSqlForTreeNode(node: TreeNode): Promise<string | null> {
  if (node.type === "table" && node.connectionId && node.database) {
    return buildDropTableSql({
      databaseType: databaseTypeForNode(node),
      schema: node.schema,
      tableName: node.label,
    });
  }
  const objectOptions = dropObjectSqlOptionsForNode(node);
  if (objectOptions) return buildDropObjectSql(objectOptions);
  const childOptions = dropTableChildObjectSqlOptionsForNode(node);
  if (childOptions && canDropTableChildObjectNode(node)) return buildDropTableChildObjectSql(childOptions);
  return null;
}

async function refreshBatchDropPreviewSql() {
  const targets = selectedBatchDropTargets();
  const statements: string[] = [];
  for (const target of targets) {
    const sql = await dropSqlForTreeNode(target);
    if (sql) statements.push(sql);
  }
  batchDropPreviewSql.value = statements.join("\n");
}

function requestBatchDrop() {
  if (!selectedBatchDropTargets().length) return;
  void refreshBatchDropPreviewSql();
  showBatchDropConfirm.value = true;
}

function requestDropSelectedNodes(): boolean {
  const selected = selectedTreeNodesInVisibleOrder();
  if (selected.length > 1 && selected.some((node) => node.id === props.node.id)) {
    if (!selectedBatchDropTargets().length) return false;
    requestBatchDrop();
    return true;
  }
  return requestDropSelectedNode();
}

function requestDropSelectedNode(): boolean {
  if (props.node.type === "table") {
    dropTable();
    return true;
  }
  if (props.node.type === "view" || props.node.type === "procedure" || props.node.type === "function") {
    requestDropObject();
    return true;
  }
  if (canDropTableChildObject.value) {
    requestDropTableChildObject();
    return true;
  }
  return false;
}

function nodeRenameObjectType(): RenameableObjectType | null {
  if (props.node.type === "table") return "TABLE";
  if (props.node.type === "view") return "VIEW";
  if (props.node.type === "materialized_view") return "MATERIALIZED_VIEW";
  if (props.node.type === "procedure") return "PROCEDURE";
  if (props.node.type === "function") return "FUNCTION";
  return null;
}

const canRenameObject = computed(() => {
  const objectType = nodeRenameObjectType();
  return !!objectType && (supportsObjectRename(currentDatabaseType(), objectType) || supportsSourceBackedRoutineRename(currentDatabaseType(), objectType as any));
});

function openRenameObjectDialog() {
  renameObjectName.value = props.node.label;
  renameObjectError.value = "";
  renameObjectPreviewSql.value = "";
  showRenameObjectDialog.value = true;
}

let renameObjectPreviewRequestId = 0;

async function refreshRenameObjectPreviewSql() {
  const requestId = ++renameObjectPreviewRequestId;
  const objectType = nodeRenameObjectType();
  const newName = renameObjectName.value.trim();
  if (!showRenameObjectDialog.value || !objectType || !newName || newName === props.node.label) {
    renameObjectPreviewSql.value = "";
    return;
  }
  if (supportsSourceBackedRoutineRename(currentDatabaseType(), objectType as any)) {
    renameObjectPreviewSql.value = `-- Recreate ${objectType} from source, then drop the original object.`;
    return;
  }
  try {
    const sql = await buildRenameObjectSql({
      databaseType: currentDatabaseType(),
      objectType,
      schema: props.node.schema,
      oldName: props.node.label,
      newName,
    });
    if (requestId === renameObjectPreviewRequestId) renameObjectPreviewSql.value = sql;
  } catch {
    if (requestId === renameObjectPreviewRequestId) renameObjectPreviewSql.value = "";
  }
}

watch([showRenameObjectDialog, renameObjectName, () => props.node.label, () => props.node.schema, () => props.node.type, () => currentDatabaseType()], () => {
  void refreshRenameObjectPreviewSql();
});

async function confirmRenameObject() {
  const node = props.node;
  const objectType = nodeRenameObjectType();
  const newName = renameObjectName.value.trim();
  if (!objectType || !newName || newName === node.label || !node.connectionId || !node.database) return;
  renameObjectError.value = "";
  try {
    const dbType = currentDatabaseType();
    await connectionStore.ensureConnected(node.connectionId);
    if (supportsSourceBackedRoutineRename(dbType, objectType as any)) {
      const schema = node.schema || node.database;
      const source = await api.getObjectSource(node.connectionId, node.database, schema, node.label, objectType as any);
      const statements = await buildRoutineRenameObjectSourceStatements({
        databaseType: dbType!,
        objectType: objectType as any,
        schema,
        name: node.label,
        newName,
        source: source.source,
      });
      for (const sql of statements) {
        await api.executeQuery(node.connectionId, node.database, sql, schema);
      }
    } else {
      const sql = await buildRenameObjectSql({
        databaseType: dbType,
        objectType,
        schema: node.schema,
        oldName: node.label,
        newName,
      });
      await api.executeQuery(node.connectionId, node.database, sql, node.schema);
    }
    toast(t("contextMenu.renameObjectSuccess", { oldName: node.label, newName }), 3000);
    showRenameObjectDialog.value = false;
    await refreshTableList(node);
  } catch (e: any) {
    renameObjectError.value = e?.message || String(e);
  }
}

async function confirmDropObject() {
  const node = props.node;
  if (!node.connectionId || !node.database) return;
  const options = dropObjectSqlOptions();
  if (!options) return;
  try {
    await connectionStore.ensureConnected(node.connectionId);
    const sql = dropObjectPreviewSql.value || (await buildDropObjectSql(options));
    await api.executeQuery(node.connectionId, node.database, sql, node.schema);
    const msgKey = node.type === "view" ? "contextMenu.dropViewSuccess" : node.type === "materialized_view" ? "contextMenu.dropViewSuccess" : node.type === "procedure" ? "contextMenu.dropProcedureSuccess" : "contextMenu.dropFunctionSuccess";
    toast(t(msgKey, { name: node.label }), 3000);
    if (node.type === "view" || node.type === "materialized_view") {
      connectionStore.removeTreeNode(node.id);
    } else {
      await refreshTableList(node);
    }
  } catch (e: any) {
    toast(t("contextMenu.tableOperationFailed", { message: e?.message || String(e) }), 5000);
  }
}

async function confirmDropTableChildObject() {
  const node = props.node;
  if (!node.connectionId || !node.database) return;
  const options = dropTableChildObjectSqlOptions();
  if (!options) return;
  try {
    await connectionStore.ensureConnected(node.connectionId);
    const sql = dropTableChildObjectPreviewSql.value || (await buildDropTableChildObjectSql(options));
    await api.executeQuery(node.connectionId, node.database, sql, node.schema);
    toast(t("contextMenu.dropTableChildObjectSuccess", { name: options.name }), 3000);
    connectionStore.removeTreeNode(node.id);
  } catch (e: any) {
    toast(t("contextMenu.tableOperationFailed", { message: e?.message || String(e) }), 5000);
  }
}

async function confirmBatchDrop() {
  const targets = selectedBatchDropTargets();
  if (!targets.length) return;
  try {
    for (const target of targets) {
      if (!target.connectionId || !target.database) continue;
      await connectionStore.ensureConnected(target.connectionId);
      const sql = await dropSqlForTreeNode(target);
      if (!sql) continue;
      await api.executeQuery(target.connectionId, target.database, sql, target.schema);
      connectionStore.removeTreeNode(target.id);
    }
    toast(t("contextMenu.batchDropSuccess", { count: targets.length }), 3000);
    showBatchDropConfirm.value = false;
  } catch (e: any) {
    toast(t("contextMenu.tableOperationFailed", { message: e?.message || String(e) }), 5000);
  }
}

const isTableNotView = computed(() => props.node.type === "table" && !isSqlServerLinkedNode(props.node));

const supportsTruncate = computed(() => {
  return supportsTableTruncate(currentDatabaseType());
});

const canCreateTable = computed(() => {
  const config = props.node.connectionId ? connectionStore.getConfig(props.node.connectionId) : undefined;
  return (props.node.type === "database" || props.node.type === "schema" || props.node.type === "group-tables") && !isSqlServerLinkedNode(props.node) && !!props.node.database && supportsTableStructureEditing(tableStructureDatabaseTypeForConnection(config));
});

const canCreateDatabase = computed(() => {
  const config = props.node.connectionId ? connectionStore.getConfig(props.node.connectionId) : undefined;
  return props.node.type === "connection" && (supportsDatabaseCreation(config?.db_type) || config?.db_type === "duckdb");
});

const isDuckDbConnection = computed(() => {
  const config = props.node.connectionId ? connectionStore.getConfig(props.node.connectionId) : undefined;
  return props.node.type === "connection" && config?.db_type === "duckdb";
});

const canSetCreateDatabaseCharset = computed(() => {
  const config = props.node.connectionId ? connectionStore.getConfig(props.node.connectionId) : undefined;
  return supportsCreateDatabaseCharset(config?.db_type, config?.driver_profile);
});

const canDropDatabase = computed(() => {
  const config = props.node.connectionId ? connectionStore.getConfig(props.node.connectionId) : undefined;
  return props.node.type === "database" && !isSqlServerLinkedNode(props.node) && supportsDatabaseCreation(config?.db_type);
});

const canCreateSchema = computed(() => {
  const config = props.node.connectionId ? connectionStore.getConfig(props.node.connectionId) : undefined;
  return props.node.type === "database" && usesTreeSchemaMode(effectiveDatabaseTypeForConnection(config)) && !connectionUsesDatabaseObjectTreeMode(config);
});

const canDropSchema = computed(() => {
  const config = props.node.connectionId ? connectionStore.getConfig(props.node.connectionId) : undefined;
  return props.node.type === "schema" && !isSqlServerLinkedNode(props.node) && usesTreeSchemaMode(effectiveDatabaseTypeForConnection(config)) && !connectionUsesDatabaseObjectTreeMode(config);
});

function tableAdminSqlOptions(): TableAdminSqlOptions {
  return {
    databaseType: currentDatabaseType(),
    schema: props.node.schema,
    tableName: props.node.label,
  };
}

async function refreshDropTablePreviewSql() {
  dropTablePreviewSql.value = "";
  dropTablePreviewSql.value = await buildDropTableSql(tableAdminSqlOptions()).catch(() => "");
}

async function refreshEmptyTablePreviewSql() {
  emptyTablePreviewSql.value = "";
  emptyTablePreviewSql.value = await buildEmptyTableSql(tableAdminSqlOptions()).catch(() => "");
}

async function refreshTruncateTablePreviewSql() {
  truncateTablePreviewSql.value = "";
  truncateTablePreviewSql.value = await buildTruncateTableSql(tableAdminSqlOptions()).catch(() => "");
}

function dropTable() {
  void refreshDropTablePreviewSql();
  showDropTableConfirm.value = true;
}

async function refreshTableList(node: TreeNode) {
  if (!node.connectionId || !node.database) return;
  await connectionStore.refreshObjectListTreeNode(node.connectionId, node.database, node.schema);
}

async function confirmDropTable() {
  const node = props.node;
  if (!node.connectionId || !node.database) return;
  try {
    await connectionStore.ensureConnected(node.connectionId);
    const sql = dropTablePreviewSql.value || (await buildDropTableSql(tableAdminSqlOptions()));
    await api.executeQuery(node.connectionId, node.database, sql, node.schema);
    toast(t("contextMenu.dropTableSuccess", { name: node.label }), 3000);
    connectionStore.removeTreeNode(node.id);
  } catch (e: any) {
    toast(t("contextMenu.tableOperationFailed", { message: e?.message || String(e) }), 5000);
  }
}

function emptyTable() {
  void refreshEmptyTablePreviewSql();
  showEmptyTableConfirm.value = true;
}

async function confirmEmptyTable() {
  const node = props.node;
  if (!node.connectionId || !node.database) return;
  try {
    await connectionStore.ensureConnected(node.connectionId);
    const sql = emptyTablePreviewSql.value || (await buildEmptyTableSql(tableAdminSqlOptions()));
    await api.executeQuery(node.connectionId, node.database, sql, node.schema);
    toast(t("contextMenu.emptyTableSuccess", { name: node.label }), 3000);
  } catch (e: any) {
    toast(t("contextMenu.tableOperationFailed", { message: e?.message || String(e) }), 5000);
  }
}

function truncateTable() {
  void refreshTruncateTablePreviewSql();
  showTruncateTableConfirm.value = true;
}

async function confirmTruncateTable() {
  const node = props.node;
  if (!node.connectionId || !node.database) return;
  try {
    await connectionStore.ensureConnected(node.connectionId);
    const sql = truncateTablePreviewSql.value || (await buildTruncateTableSql(tableAdminSqlOptions()));
    await api.executeQuery(node.connectionId, node.database, sql, node.schema);
    toast(t("contextMenu.truncateTableSuccess", { name: node.label }), 3000);
  } catch (e: any) {
    toast(t("contextMenu.tableOperationFailed", { message: e?.message || String(e) }), 5000);
  }
}

async function refreshDropDatabasePreviewSql() {
  dropDatabasePreviewSql.value = "";
  dropDatabasePreviewSql.value = await buildDropDatabaseSql({
    databaseType: currentDatabaseType(),
    name: props.node.label,
  }).catch(() => "");
}

async function refreshDropSchemaPreviewSql() {
  dropSchemaPreviewSql.value = "";
  dropSchemaPreviewSql.value = await buildDropSchemaSql({
    databaseType: currentDatabaseType(),
    name: props.node.label,
  }).catch(() => "");
}

async function openCreateDatabase() {
  if (isDuckDbConnection.value) {
    await createDuckDbAttachedDatabaseFile();
    return;
  }
  openCreateDatabaseDialog();
}

function openCreateDatabaseDialog() {
  createDatabaseName.value = "";
  createDatabaseCharset.value = "utf8mb4";
  createDatabaseCollation.value = "utf8mb4_unicode_ci";
  showCreateDatabaseDialog.value = true;
}

function ensureDuckDbFileExtension(path: string): string {
  return /\.(duckdb|db)$/i.test(path) ? path : `${path}.duckdb`;
}

async function createDuckDbAttachedDatabaseFile() {
  const node = props.node;
  if (!node.connectionId) return;
  if (!isTauriRuntime()) {
    toast(t("contextMenu.createDuckDbFileDesktopOnly"), 4000);
    return;
  }

  try {
    const { save } = await import("@tauri-apps/plugin-dialog");
    const selectedPath = await save({
      defaultPath: "database.duckdb",
      filters: [{ name: "DuckDB", extensions: ["duckdb", "db"] }],
    });
    if (!selectedPath) return;

    const path = ensureDuckDbFileExtension(selectedPath);
    await connectionStore.ensureConnected(node.connectionId);
    const existingDatabases = await api.listDatabases(node.connectionId);
    const name = uniqueDuckDbAttachedDatabaseName(
      duckDbAttachedDatabaseNameFromPath(path),
      existingDatabases.map((database) => database.name),
    );
    await api.executeQuery(node.connectionId, "", await buildDuckDbAttachDatabaseSql(path, name));

    const config = connectionStore.getConfig(node.connectionId);
    if (config) {
      await connectionStore.updateConnection({
        ...config,
        attached_databases: [...(config.attached_databases ?? []), { name, path }],
      });
    }
    await connectionStore.loadDatabases(node.connectionId, { force: true });
    connectionStore.selectedTreeNodeId = `${node.connectionId}:${name}`;
    toast(t("contextMenu.createDuckDbFileSuccess", { name }), 3000);
  } catch (e: any) {
    toast(t("contextMenu.tableOperationFailed", { message: e?.message || String(e) }), 5000);
  }
}

async function confirmCreateDatabase() {
  const node = props.node;
  const name = createDatabaseName.value.trim();
  if (!name || !node.connectionId) return;
  showCreateDatabaseDialog.value = false;
  try {
    await connectionStore.ensureConnected(node.connectionId);
    const config = connectionStore.getConfig(node.connectionId);
    const sql = await buildCreateDatabaseSql({
      databaseType: config?.db_type,
      driverProfile: config?.driver_profile,
      name,
      charset: createDatabaseCharset.value,
      collation: createDatabaseCollation.value,
    });
    await api.executeQuery(node.connectionId, "", sql);
    toast(t("contextMenu.createDatabaseSuccess", { name }), 3000);
    await connectionStore.loadDatabases(node.connectionId, { force: true });
  } catch (e: any) {
    toast(t("contextMenu.tableOperationFailed", { message: e?.message || String(e) }), 5000);
  }
}

function dropDatabase() {
  void refreshDropDatabasePreviewSql();
  dropDatabaseLoading.value = false;
  showDropDatabaseConfirm.value = true;
}

function flushRedisDb() {
  showFlushRedisDbConfirm.value = true;
}

async function confirmFlushRedisDb() {
  const node = props.node;
  if (node.type !== "redis-db" || !node.connectionId || !node.database) return;
  try {
    await connectionStore.ensureConnected(node.connectionId);
    await api.redisFlushDb(node.connectionId, Number(node.database));
    connectionStore.updateRedisDbKeyStats(node.connectionId, Number(node.database), { loaded: 0, total: 0 });
    window.dispatchEvent(
      new CustomEvent("dbx-redis-db-flushed", {
        detail: { connectionId: node.connectionId, db: Number(node.database) },
      }),
    );
    toast(t("redis.flushDbSuccess", { db: node.database }), 3000);
  } catch (e: any) {
    toast(t("contextMenu.tableOperationFailed", { message: e?.message || String(e) }), 5000);
  }
}

async function confirmDropDatabase() {
  const node = props.node;
  if (!node.connectionId || dropDatabaseLoading.value) return;
  dropDatabaseLoading.value = true;
  try {
    await connectionStore.ensureConnected(node.connectionId);
    const sql =
      dropDatabasePreviewSql.value ||
      (await buildDropDatabaseSql({
        databaseType: currentDatabaseType(),
        name: node.label,
      }));
    await api.executeQuery(node.connectionId, "", sql);
    toast(t("contextMenu.dropDatabaseSuccess", { name: node.label }), 3000);
    await connectionStore.loadDatabases(node.connectionId, { force: true });
    showDropDatabaseConfirm.value = false;
  } catch (e: any) {
    toast(t("contextMenu.tableOperationFailed", { message: e?.message || String(e) }), 5000);
  } finally {
    dropDatabaseLoading.value = false;
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
    const sql = await buildCreateSchemaSql({
      databaseType: currentDatabaseType(),
      name,
    });
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
  void refreshDropSchemaPreviewSql();
  showDropSchemaConfirm.value = true;
}

async function confirmDropSchema() {
  const node = props.node;
  if (!node.connectionId || !node.database) return;
  try {
    await connectionStore.ensureConnected(node.connectionId);
    const sql =
      dropSchemaPreviewSql.value ||
      (await buildDropSchemaSql({
        databaseType: currentDatabaseType(),
        name: node.label,
      }));
    await api.executeQuery(node.connectionId, node.database, sql);
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

function duplicateStructure(source: TreeNode = props.node) {
  if (!isDuplicateStructureSource(source)) return;
  duplicateStructureSource.value = source;
  duplicateTableName.value = `${source.label}_copy`;
  showDuplicateDialog.value = true;
}

function isDuplicateStructureSource(node: TreeNode): node is DuplicateStructureSource {
  return node.type === "table" && !!node.connectionId && !!node.database;
}

async function confirmDuplicateStructure() {
  const node = duplicateStructureSource.value || (isDuplicateStructureSource(props.node) ? props.node : null);
  const newName = duplicateTableName.value.trim();
  if (!newName || !node) return;
  showDuplicateDialog.value = false;
  try {
    await connectionStore.ensureConnected(node.connectionId);
    const databaseType = databaseTypeForNode(node);
    const sql = await buildDuplicateTableStructureSql({
      databaseType,
      schema: node.schema,
      sourceName: node.label,
      targetName: newName,
    });
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
  queryStore.openTableStructure(node.connectionId, node.database, node.schema, "");
}

function createView() {
  const node = props.node;
  if (!node.connectionId || !node.database) return;
  connectionStore.activeConnectionId = node.connectionId;
  const viewName = node.schema ? `${node.schema}.new_view` : "new_view";
  const tabId = queryStore.createTab(node.connectionId, node.database, t("contextMenu.createView"), "query", node.schema);
  queryStore.updateSql(tabId, `CREATE VIEW ${viewName} AS\nSELECT\n  *\nFROM table_name;\n`);
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

async function exportStructure() {
  const targets = structureExportTargets();
  if (!targets.length) return;
  isLoadingStructurePreview.value = true;
  structurePreviewError.value = "";
  structurePreviewSql.value = "";
  structurePreviewTitle.value = targets.length === 1 ? t("contextMenu.exportStructurePreviewTitle", { name: targets[0]!.label }) : t("contextMenu.exportStructurePreviewTitleMultiple", { count: targets.length });
  structurePreviewDefaultFileName.value = targets.length === 1 ? `${targets[0]!.label}.sql` : "structures.sql";
  showStructurePreviewDialog.value = true;
  try {
    const parts: string[] = [];
    for (const target of targets) {
      await connectionStore.ensureConnected(target.connectionId);
      const ddl = await api.getTableDdl(target.connectionId, target.database, target.schema || target.database, target.label, target.type === "view" ? "VIEW" : undefined);
      parts.push(ddl.trim());
    }
    structurePreviewSql.value = `${parts.filter(Boolean).join("\n\n")}\n`;
  } catch (e: any) {
    structurePreviewError.value = e?.message || String(e);
    console.error("Export structure failed:", e);
  } finally {
    isLoadingStructurePreview.value = false;
  }
}

function canExportStructureNode(node: TreeNode): node is TreeNode & { connectionId: string; database: string } {
  return (node.type === "table" || node.type === "view") && !!node.connectionId && !!node.database;
}

function selectedStructureNodes(): TreeNode[] {
  const selectedIds = new Set(connectionStore.selectedTreeNodeIds);
  if (!selectedIds.size) return [];
  const nodes: TreeNode[] = [];
  const visit = (items: TreeNode[]) => {
    for (const item of items) {
      if (selectedIds.has(item.id) && canExportStructureNode(item)) nodes.push(item);
      if (item.children) visit(item.children);
    }
  };
  visit(connectionStore.treeNodes);
  return nodes;
}

function structureExportTargets(): Array<TreeNode & { connectionId: string; database: string }> {
  if (!canExportStructureNode(props.node)) return [];
  const selected = selectedStructureNodes().filter((node): node is TreeNode & { connectionId: string; database: string } => canExportStructureNode(node) && node.connectionId === props.node.connectionId && node.database === props.node.database);
  return selected.some((node) => node.id === props.node.id) ? selected : [props.node];
}

function structureTargetName(target: TreeNode): string {
  return target.schema ? `${target.schema}.${target.label}` : target.label;
}

function columnDocValue(value: unknown): string {
  return value === null || value === undefined ? "" : String(value);
}

function tsvCell(value: unknown): string {
  return columnDocValue(value).replace(/\t/g, " ").replace(/\r?\n/g, " ").trim();
}

function markdownCell(value: unknown): string {
  return columnDocValue(value).replace(/\|/g, "\\|").replace(/\r?\n/g, "<br>").trim();
}

function columnDocHeaders(includeTable: boolean): string[] {
  const headers = [t("contextMenu.structureDocColumn"), t("contextMenu.structureDocType"), t("contextMenu.structureDocPrimaryKey"), t("contextMenu.structureDocNullable"), t("contextMenu.structureDocDefault"), t("contextMenu.structureDocComment")];
  return includeTable ? [t("contextMenu.structureDocTable"), ...headers] : headers;
}

function columnDocCells(target: TreeNode, column: ColumnInfo, includeTable: boolean): unknown[] {
  const cells = [column.name, column.data_type, column.is_primary_key ? t("contextMenu.structureDocYes") : t("contextMenu.structureDocNo"), column.is_nullable ? t("contextMenu.structureDocYes") : t("contextMenu.structureDocNo"), column.column_default, column.comment];
  return includeTable ? [structureTargetName(target), ...cells] : cells;
}

async function tableColumnsForStructureCopy(target: TreeNode & { connectionId: string; database: string }): Promise<ColumnInfo[]> {
  await connectionStore.ensureConnected(target.connectionId);
  return (await api.getColumns(target.connectionId, target.database, target.schema || target.database, target.label)) as ColumnInfo[];
}

async function buildStructureCopyText(format: StructureCopyFormat): Promise<string> {
  const targets = structureExportTargets();
  if (!targets.length) return "";
  const includeTable = targets.length > 1;
  const headers = columnDocHeaders(includeTable);

  if (format === "tsv") {
    const lines = [headers.map(tsvCell).join("\t")];
    for (const target of targets) {
      const columns = await tableColumnsForStructureCopy(target);
      for (const column of columns) {
        lines.push(columnDocCells(target, column, includeTable).map(tsvCell).join("\t"));
      }
    }
    return `${lines.join("\n")}\n`;
  }

  const tables: string[] = [];
  const markdownHeaders = columnDocHeaders(false);
  for (const target of targets) {
    const columns = await tableColumnsForStructureCopy(target);
    const tableLines = [`### ${markdownCell(structureTargetName(target))}`, "", `| ${markdownHeaders.map(markdownCell).join(" | ")} |`, `| ${markdownHeaders.map(() => "---").join(" | ")} |`, ...columns.map((column) => `| ${columnDocCells(target, column, false).map(markdownCell).join(" | ")} |`)];
    tables.push(tableLines.join("\n"));
  }
  return `${tables.join("\n\n")}\n`;
}

async function copyStructureAs(format: StructureCopyFormat) {
  let text = "";
  try {
    text = await buildStructureCopyText(format);
    if (!text) return;
    await copyToClipboard(text);
    toast(t("contextMenu.structureDocCopied"), 2000);
  } catch (e: any) {
    if (text) {
      structureDocCopyText.value = text;
      structureDocCopyTitle.value = format === "tsv" ? t("contextMenu.copyStructureAsTsv") : t("contextMenu.copyStructureAsMarkdown");
      showStructureDocCopyDialog.value = true;
      return;
    }
    toast(t("grid.copyFailed", { message: e?.message || String(e) }), 5000);
  }
}

async function copyStructureDocText() {
  if (!structureDocCopyText.value) return;
  try {
    await copyToClipboard(structureDocCopyText.value);
    toast(t("contextMenu.structureDocCopied"), 2000);
  } catch (e: any) {
    toast(t("grid.copyFailed", { message: e?.message || String(e) }), 5000);
  }
}

function selectTextareaContent(event: FocusEvent) {
  if (event.target instanceof HTMLTextAreaElement) event.target.select();
}

async function copyStructurePreview() {
  if (!structurePreviewSql.value) return;
  try {
    await copyToClipboard(structurePreviewSql.value);
    toast(t("contextMenu.exportStructureCopied"), 2000);
  } catch (e: any) {
    toast(t("grid.copyFailed", { message: e?.message || String(e) }), 5000);
  }
}

async function saveStructurePreview() {
  if (!structurePreviewSql.value) return;
  try {
    await saveFileContent(structurePreviewSql.value, structurePreviewDefaultFileName.value, "SQL", "sql");
    toast(t("grid.exported"));
  } catch (e: any) {
    toast(t("grid.exportFailed", { message: e?.message || String(e) }), 5000);
  }
}

async function exportDataLegacy(format: "csv" | "json" | "sql") {
  const node = props.node;
  if (!node.connectionId || !node.database) return;
  const connectionId = node.connectionId;
  const database = node.database;
  const config = connectionStore.getConfig(node.connectionId);
  if (!config) return;

  try {
    await connectionStore.ensureConnected(connectionId);
    const tableColumns = format === "sql" ? await api.getColumns(connectionId, database, node.schema || database, node.label) : undefined;
    const queryColumns = config.db_type === "neo4j" ? (tableColumns ?? (await api.getColumns(connectionId, database, node.schema || database, node.label))).map((column) => column.name) : undefined;
    const effectiveDbType = effectiveDatabaseTypeForConnection(config);
    const result = await fetchTableDataForExport({
      databaseType: effectiveDbType,
      schema: node.schema,
      tableName: node.label,
      columns: queryColumns,
      executePage: (sql) => api.executeQuery(connectionId, database, sql),
    });

    if (format === "csv") {
      let outputPath = `${node.label}.csv`;
      if (isTauriRuntime()) {
        const { save } = await import("@tauri-apps/plugin-dialog");
        const path = await save({
          defaultPath: outputPath,
          filters: [{ name: "CSV", extensions: ["csv"] }],
        });
        if (!path) return;
        outputPath = path as string;
      }
      await api.exportQueryResultCsv(outputPath, result.columns, result.rows);
      toast(t("grid.exported"));
      return;
    }

    if (format === "json") {
      let outputPath = `${node.label}.json`;
      if (isTauriRuntime()) {
        const { save } = await import("@tauri-apps/plugin-dialog");
        const path = await save({
          defaultPath: outputPath,
          filters: [{ name: "JSON", extensions: ["json"] }],
        });
        if (!path) return;
        outputPath = path as string;
      }
      await api.exportQueryResultJson(outputPath, result.columns, result.rows);
      toast(t("grid.exported"));
      return;
    }

    const content = await formatSqlInsert({
      databaseType: effectiveDbType,
      schema: node.schema,
      tableName: node.label,
      columns: result.columns,
      columnTypes: tableColumns ? columnTypesForResultColumns(result.columns, tableColumns) : undefined,
      rows: result.rows,
    });
    await saveFileContent(content, `${node.label}.sql`, "SQL", "sql");
    toast(t("grid.exported"));
  } catch (e: any) {
    toast(t("grid.exportFailed", { message: e?.message || String(e) }), 5000);
  }
}

function columnTypesForResultColumns(columns: string[], tableColumns: ColumnInfo[]): Array<string | undefined> {
  const typesByName = new Map(tableColumns.map((column) => [column.name.toLocaleLowerCase(), column.data_type]));
  return columns.map((column) => typesByName.get(column.toLocaleLowerCase()));
}

async function exportData(format: "csv" | "json" | "sql") {
  if (format !== "csv") {
    await exportDataLegacy(format);
    return;
  }
  await exportTableData("csv");
}

async function exportTableData(format: "csv" | "xlsx") {
  const node = props.node;
  if (!node.connectionId || !node.database) return;
  const connectionId = node.connectionId;
  const database = node.database;
  const config = connectionStore.getConfig(node.connectionId);
  if (!config) return;

  let task: ExportTask | null = null;
  try {
    await connectionStore.ensureConnected(connectionId);

    // Step 1: Open save dialog FIRST
    let outputPath = `${node.label}.${format}`;
    if (isTauriRuntime()) {
      const { save } = await import("@tauri-apps/plugin-dialog");
      const path = await save({
        defaultPath: outputPath,
        filters: [{ name: format === "csv" ? "CSV" : "Excel", extensions: [format] }],
      });
      if (!path) return;
      outputPath = path as string;
    }

    // Step 2: Register task in export tracker (background)
    task = addExportTask(node.label, format, outputPath);
    const currentTask = task;

    // Step 3: Get query columns for neo4j
    const queryColumns = config.db_type === "neo4j" ? (await api.getColumns(connectionId, database, node.schema || database, node.label)).map((c) => c.name) : undefined;

    // Step 4: Start streaming export (background, non-blocking)
    const request: api.TableExportRequest = {
      exportId: currentTask.exportId,
      connectionId,
      database,
      schema: node.schema || undefined,
      tableName: node.label,
      filePath: outputPath,
      format,
      columns: queryColumns,
      batchSize: settingsStore.editorSettings.exportBatchSize,
    };

    await api.startTableExport(request, (progress) => {
      currentTask.rowsExported = progress.rowsExported;
      currentTask.totalRows = progress.totalRows;
      currentTask.status = progress.status;
      currentTask.errorMessage = progress.errorMessage || null;
      if (progress.status === "Done") {
        toast(t("grid.exported"));
      } else if (progress.status === "Error") {
        toast(t("grid.exportFailed", { message: progress.errorMessage || "" }), 5000);
      }
    });
  } catch (e: any) {
    if (task) {
      task.status = "Error";
      task.errorMessage = e?.message || String(e);
    }
    toast(t("grid.exportFailed", { message: e?.message || String(e) }), 5000);
  }
}

async function exportDataXlsx() {
  await exportTableData("xlsx");
}

function editConnection() {
  if (props.node.connectionId) {
    connectionStore.startEditing(props.node.connectionId);
  }
}

const revealConnectionFilePath = computed<string | null>(() => {
  if (props.node.type !== "connection" || !props.node.connectionId) return null;
  const config = connectionStore.getConfig(props.node.connectionId);
  if (!config) return null;
  return connectionFilePath(config);
});

async function revealDatabaseFile() {
  const path = revealConnectionFilePath.value;
  if (!path) return;
  try {
    await revealPathInFileManager(path);
  } catch (e: any) {
    const message = typeof e === "string" ? e : e?.message || String(e);
    toast(message, 5000);
  }
}

const sqliteBackupSource = computed<string | null>(() => {
  if (props.node.type !== "connection" || !props.node.connectionId) return null;
  const config = connectionStore.getConfig(props.node.connectionId);
  if (!config) return null;
  return sqliteBackupSourcePath(config);
});

const canBackupSqliteDatabase = computed(() => {
  const source = sqliteBackupSource.value;
  if (!source || !props.node.connectionId) return false;
  return isTauriRuntime() && (!isMemorySqlitePath(source) || connectionStore.connectedIds.has(props.node.connectionId));
});

async function backupSqliteDatabase() {
  const connId = props.node.connectionId;
  const config = connId ? connectionStore.getConfig(connId) : undefined;
  const sourcePath = sqliteBackupSource.value;
  if (!connId || !config || !sourcePath) return;

  try {
    const { save } = await import("@tauri-apps/plugin-dialog");
    const destinationPath = await save({
      defaultPath: defaultSqliteBackupFileName(config),
      filters: [{ name: "SQLite", extensions: ["db", "sqlite", "sqlite3"] }],
    });
    if (!destinationPath) return;

    toast(t("contextMenu.backupSqliteDatabaseInProgress"), 2000);
    if (!isMemorySqlitePath(sourcePath)) {
      await connectionStore.ensureConnected(connId);
    }
    await api.backupSqliteDatabase(connId, destinationPath);
    toast(t("contextMenu.backupSqliteDatabaseSuccess"), 3000);
  } catch (e: any) {
    toast(t("contextMenu.backupSqliteDatabaseFailed", { message: e?.message || String(e) }), 5000);
  }
}

async function disconnectConnection() {
  if (props.node.connectionId) {
    try {
      await connectionStore.disconnect(props.node.connectionId);
      props.node.isExpanded = false;
      props.node.children = [];
      toast(t("connection.disconnected"), 2000);
    } catch (e: any) {
      toast(t("connection.saveFailed", { message: e?.message || String(e) }), 5000);
    }
  }
}

async function closeDatabaseConnection() {
  const node = props.node;
  if (node.type !== "database" || !node.connectionId || node.database == null) return;
  try {
    await connectionStore.closeDatabaseConnection(node.connectionId, node.database);
    toast(t("connection.databaseConnectionClosed", { name: node.label }), 2000);
  } catch (e: any) {
    toast(t("connection.saveFailed", { message: e?.message || String(e) }), 5000);
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
      schema: props.node.schema,
    };
  }
}

function openDataCompare() {
  if (props.node.connectionId) {
    connectionStore.dataCompareSource = {
      connectionId: props.node.connectionId,
      database: props.node.database ?? "",
      schema: props.node.schema,
      tableName: props.node.type === "table" ? props.node.label : undefined,
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

function openDatabaseExport() {
  const node = props.node;
  if (!node.connectionId || !node.database) return;
  connectionStore.databaseExportSource = {
    connectionId: node.connectionId,
    database: node.database,
    schema: node.type === "schema" || node.type === "table" || node.type === "view" || node.type === "materialized_view" ? node.schema : undefined,
    tableName: node.type === "table" || node.type === "view" || node.type === "materialized_view" ? node.label : undefined,
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
  queryStore.openTableStructure(node.connectionId, node.database, node.schema, node.label);
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

const canExpand = computed(() =>
  canTreeNodeShowExpander({
    type: activeNode.value.type,
    childCount: activeNode.value.children?.length ?? 0,
  }),
);

const isPinned = computed(() => activeNode.value.pinned || connectionStore.isTreeNodePinned(activeNode.value));

const isNodeDefaultDatabase = computed(
  () => (activeNode.value.type === "database" || activeNode.value.type === "redis-db" || activeNode.value.type === "mongo-db") && !!activeNode.value.connectionId && !!activeNode.value.database && connectionStore.isDefaultDatabase(activeNode.value.connectionId, activeNode.value.database),
);

const trailingComment = computed(() => {
  if (!settingsStore.editorSettings.sidebarObjectInfoMode.startsWith("comment-")) return null;
  return sidebarTreeNodeComment(activeNode.value);
});

function isRightAlignedComment(): boolean {
  return settingsStore.editorSettings.sidebarObjectInfoMode === "comment-right" && !!trailingComment.value;
}

function cancelTrailingCommentMeasure() {
  if (!trailingCommentMeasureFrame) return;
  window.cancelAnimationFrame(trailingCommentMeasureFrame);
  trailingCommentMeasureFrame = 0;
}

function measureTrailingCommentLayout() {
  const container = trailingCommentLayoutRef.value;
  const leading = trailingCommentLeadingRef.value;
  if (!isRightAlignedComment() || !container || !leading) {
    trailingCommentMaxWidth.value = 0;
    return;
  }
  trailingCommentMaxWidth.value = trailingCommentAvailableWidth(container.clientWidth, leading.scrollWidth);
}

function scheduleTrailingCommentMeasure() {
  if (typeof window === "undefined") {
    measureTrailingCommentLayout();
    return;
  }
  cancelTrailingCommentMeasure();
  trailingCommentMeasureFrame = window.requestAnimationFrame(() => {
    trailingCommentMeasureFrame = 0;
    measureTrailingCommentLayout();
  });
}

function refreshTrailingCommentMeasurement() {
  trailingCommentResizeObserver?.disconnect();
  trailingCommentResizeObserver = null;

  const container = trailingCommentLayoutRef.value;
  const leading = trailingCommentLeadingRef.value;
  if (!isRightAlignedComment() || !container || !leading) {
    trailingCommentMaxWidth.value = 0;
    return;
  }

  scheduleTrailingCommentMeasure();
  if (typeof ResizeObserver !== "undefined") {
    trailingCommentResizeObserver = new ResizeObserver(scheduleTrailingCommentMeasure);
    trailingCommentResizeObserver.observe(container);
    trailingCommentResizeObserver.observe(leading);
  }
}

function formattedObjectStorage(): string {
  if (settingsStore.editorSettings.sidebarObjectInfoMode !== "size" || (activeNode.value.type !== "database" && activeNode.value.type !== "table" && activeNode.value.type !== "materialized_view")) return "";
  return formatSidebarObjectStorage(activeNode.value.sizeBytes);
}

const alignedCommentLabelWidth = computed(() => (settingsStore.editorSettings.sidebarObjectInfoMode === "comment-aligned" ? props.commentLabelWidth : undefined));

function hasTrailingMetadata(): boolean {
  return !!trailingComment.value || !!formattedObjectStorage();
}

const usesFullWidthLabel = computed(() => usesFullWidthTreeLabel(activeNode.value.type, settingsStore.editorSettings.sidebarAllowHorizontalScroll, hasTrailingMetadata()));

const rowWidthClass = computed(() => (usesFullWidthLabel.value ? "w-max min-w-full" : "w-full min-w-0"));

const labelWidthClass = computed(() => treeLabelWidthClass({ fullWidth: usesFullWidthLabel.value, hasTrailingComment: hasTrailingMetadata() }));

watch(() => [isRightAlignedComment(), visibleLabel(activeNode.value), trailingComment.value, trailingCommentLayoutRef.value, trailingCommentLeadingRef.value], refreshTrailingCommentMeasurement, { flush: "post", immediate: true });

const paddingLeft = computed(() => treeItemPaddingLeft(props.depth));
const isConnected = computed(() => props.node.type === "connection" && !!props.node.connectionId && connectionStore.connectedIds.has(props.node.connectionId));
const isConnectionReadonly = computed(() => props.node.type === "connection" && !!props.node.connectionId && (connectionStore.getConfig(props.node.connectionId)?.read_only ?? false));
const canRefreshExternalSnapshot = computed(() => {
  const dbType = currentDatabaseType();
  return props.node.type === "connection" && !!dbType && EXTERNAL_TABULAR_TYPES.has(dbType);
});
const canCloseDatabaseConnection = computed(() => props.node.type === "database" && !!props.node.connectionId && props.node.database != null && connectionStore.connectedIds.has(props.node.connectionId));
const nodeIconClass = computed(() => {
  const infoClass = getIconInfo(props.node)?.colorClass;
  if (props.node.type !== "database") return infoClass;
  return canCloseDatabaseConnection.value ? infoClass : "text-muted-foreground/65";
});

const isConnecting = computed(() => activeNode.value.type === "connection" && !!activeNode.value.connectionId && connectionStore.connectingIds.has(activeNode.value.connectionId));

const isConnectionReadonly = computed(() => activeNode.value.type === "connection" && !!activeNode.value.connectionId && (connectionStore.getConfig(activeNode.value.connectionId)?.read_only ?? false));

const databaseOpenVisual = computed(() => {
  const opened = isSidebarDatabaseOpened(activeNode.value, connectionStore.isTreeNodeChildrenLoaded);
  const showsIndicator = activeNode.value.type === "database" && (opened || (!!activeNode.value.connectionId && activeNode.value.database != null && queryStore.openDatabaseKeys.has(`${activeNode.value.connectionId}\x00${activeNode.value.database}`)));
  const infoClass = getIconInfo(activeNode.value)?.colorClass;
  return {
    iconClass: activeNode.value.type !== "database" || opened ? infoClass : "text-muted-foreground/65",
    showsIndicator,
  };
});

function connectionIconType(connectionId?: string) {
  const config = connectionId ? connectionStore.getConfig(connectionId) : undefined;
  return config?.driver_profile || config?.db_type || "postgres";
}

const connectionColor = computed(() => {
  const connectionId = activeNode.value.connectionId;
  return connectionId ? connectionStore.getConfig(connectionId)?.color || "" : "";
});

const isActiveConnectionScope = computed(() => !!activeNode.value.connectionId && connectionStore.activeConnectionId === activeNode.value.connectionId);

const selectionVisual = computed(() => {
  const selected = connectionStore.selectedTreeNodeId === activeNode.value.id;
  const multiSelected = connectionStore.selectedTreeNodeIdsSet.has(activeNode.value.id);
  return {
    selected,
    multiSelected,
    rowSelected: selected || multiSelected,
    usesSelectionSetHighlight: connectionStore.connectionMultiSelectActive || connectionStore.selectedTreeNodeIds.length > 1,
  };
});

const rowStyle = computed(() => {
  const color = connectionColor.value;
  const backgroundColor = hexToRgba(color, isActiveConnectionScope.value ? 0.14 : 0.08);
  return {
    paddingLeft: paddingLeft.value,
    paddingRight: trailingComment.value ? "12px" : undefined,
    "--tree-connection-row-bg": backgroundColor,
    "--tree-connection-row-hover-bg": hexToRgba(color, isActiveConnectionScope.value ? 0.18 : 0.12),
    "--tree-connection-active-bg": hexToRgba(color, 0.18),
    "--tree-connection-active-focus-bg": hexToRgba(color, 0.22),
  };
});

const tableSearchStyle = computed(() => {
  const color = connectionColor.value;
  const rowBackgroundColor = color ? hexToRgba(color, isActiveConnectionScope.value ? 0.14 : 0.08) : "transparent";
  return {
    paddingLeft: paddingLeft.value,
    "--tree-table-search-row-bg": rowBackgroundColor,
    "--tree-table-search-input-bg": color ? hexToRgba(color, isActiveConnectionScope.value ? 0.05 : 0.03) : "hsl(var(--background) / 0.56)",
    "--tree-table-search-border": color ? hexToRgba(color, isActiveConnectionScope.value ? 0.12 : 0.08) : "hsl(var(--border) / 0.36)",
  };
});

function updateTableSearchQuery(value: string | number) {
  const parentId = tableSearchParentId.value;
  if (!parentId) return;
  const query = String(value);
  if (sidebarTreeContext?.setTableSearchQuery) {
    sidebarTreeContext.setTableSearchQuery(parentId, query);
    return;
  }
  connectionStore.setSidebarTableSearchQuery(parentId, query);
  void connectionStore.refreshSidebarTableSearch(parentId);
}

function clearTableSearchQuery() {
  updateTableSearchQuery("");
}

// --- Connection Group Management ---
const isRenamingGroup = ref(false);

const renameInput = ref("");

const renameInputRef = ref<HTMLInputElement>();

function startRenameGroup() {
  renameInput.value = activeNode.value.label;
  isRenamingGroup.value = true;
  emit("rename-started");
  focusSidebarRenameInput(() => (isRenamingGroup.value ? renameInputRef.value : undefined));
}

watch(
  () => props.pendingRename,
  (pending) => {
    if (pending && activeNode.value.type === "connection-group") startRenameGroup();
  },
  { immediate: true },
);

function shouldMeasureLabelOverflow(): boolean {
  return shouldMeasureSidebarLabelOverflow({
    hasDetailTooltip: !!detailTooltip.value?.rows.length,
    isRenaming: isRenamingGroup.value,
    usesFullWidthLabel: usesFullWidthLabel.value,
  });
}

function finishRenameGroup() {
  // Guard against double invocation: pressing Enter sets isRenamingGroup=false
  // and unmounts the input, which then fires @blur -> finishRenameGroup again.
  // The first call can rebuild the tree and recycle activeNode.value onto a different
  // group, so a second run would act on the wrong group and cascade across
  // groups (issue #681).
  if (!isRenamingGroup.value) return;
  isRenamingGroup.value = false;
  const trimmed = renameInput.value.trim();
  // An empty name cancels the rename and keeps the group as-is — never delete
  // here. Deleting a group is done explicitly via the context menu (issue #681).
  if (!trimmed || trimmed === activeNode.value.label) return;
  connectionStore.renameConnectionGroup(activeNode.value.id, trimmed);
}

const {
  state: dragState,
  startDrag,
  updateTarget,
  clearTarget,
} = useDragSort((draggedId, targetId, position) => {
  // If the grabbed row is part of a multi-selection, move all selected rows
  // together; otherwise just the grabbed one (issue #681).
  const selected = connectionStore.selectedTreeNodeIds;
  const draggedIds = selected.length > 1 && selected.includes(draggedId) ? [...selected] : [draggedId];
  connectionStore.reorderSidebarEntries(draggedIds, targetId, position);
});

const isDraggable = computed(() => {
  if (props.dragDisabled) return false;
  return activeNode.value.type === "connection" || activeNode.value.type === "connection-group";
});

const dragVisual = computed(() => ({
  isDropTarget: activeNode.value.type === "connection" || activeNode.value.type === "connection-group",
  showBefore: dragState.active && dragState.targetId === activeNode.value.id && dragState.dropPosition === "before",
  showAfter: dragState.active && dragState.targetId === activeNode.value.id && dragState.dropPosition === "after",
  showInside: dragState.active && dragState.targetId === activeNode.value.id && dragState.dropPosition === "inside",
  dragging: dragState.active && dragState.draggedId === activeNode.value.id,
}));

const TABLE_REFERENCE_DRAG_THRESHOLD = 5;

const TABLE_REFERENCE_DRAGGING_CLASS = "dbx-table-reference-dragging";

const canDragTableReference = computed(() => {
  if (props.dragDisabled || !activeNode.value.connectionId) return false;
  if (activeNode.value.type === "database") return typeof activeNode.value.database === "string" && activeNode.value.database.trim().length > 0;
  if (activeNode.value.database == null) return false;
  if (activeNode.value.type === "table" || activeNode.value.type === "view" || activeNode.value.type === "materialized_view") return true;
  return activeNode.value.type === "column" && !!activeNode.value.tableName;
});

let pendingTableReferenceDrag: {
  payload: QueryEditorTableReferencePayload;
  startX: number;
  startY: number;
} | null = null;

let draggingTableReferencePayload: QueryEditorTableReferencePayload | null = null;

let suppressNextTableReferenceClick = false;

function tableReferenceDragPayload(): QueryEditorTableReferencePayload | null {
  if (!canDragTableReference.value) return null;
  if (activeNode.value.type === "database") {
    return createTableReferencePayload({
      connectionId: activeNode.value.connectionId,
      database: activeNode.value.database,
      referenceType: "database",
      databaseType: currentDatabaseType(),
    });
  }
  if (activeNode.value.type === "column") {
    const columnName = columnNameForDrag(activeNode.value);
    if (!activeNode.value.tableName || !columnName) return null;
    return createTableReferencePayload({
      connectionId: activeNode.value.connectionId,
      database: activeNode.value.database,
      schema: activeNode.value.schema,
      tableName: activeNode.value.tableName,
      columnName,
      databaseType: currentDatabaseType(),
    });
  }
  const payload = createTableReferencePayload({
    connectionId: activeNode.value.connectionId,
    database: activeNode.value.database,
    schema: activeNode.value.schema,
    tableName: activeNode.value.label,
    databaseType: currentDatabaseType(),
  });
  return payload;
}

function columnNameForDrag(node: TreeNode): string {
  const column = node.meta as Partial<ColumnInfo> | undefined;
  if (typeof column?.name === "string" && column.name) return column.name;
  return node.label.replace(/\s+\([^()]*\)$/, "");
}

function startTableReferenceDrag(payload: QueryEditorTableReferencePayload) {
  draggingTableReferencePayload = payload;
  setActiveTableReferencePayload(payload);
  document.getSelection()?.removeAllRanges();
  document.body.style.cursor = "copy";
}

function finishTableReferenceDrag() {
  clearActiveTableReferencePayload(draggingTableReferencePayload);
  pendingTableReferenceDrag = null;
  draggingTableReferencePayload = null;
  document.body.classList.remove(TABLE_REFERENCE_DRAGGING_CLASS);
  document.body.style.cursor = "";
  document.removeEventListener("mousemove", onTableReferenceMouseMove, true);
  document.removeEventListener("mouseup", onTableReferenceMouseUp, true);
}

function onTableReferenceMouseMove(event: MouseEvent) {
  if (!pendingTableReferenceDrag && !draggingTableReferencePayload) return;
  if (pendingTableReferenceDrag && !draggingTableReferencePayload) {
    const dx = event.clientX - pendingTableReferenceDrag.startX;
    const dy = event.clientY - pendingTableReferenceDrag.startY;
    if (Math.abs(dx) < TABLE_REFERENCE_DRAG_THRESHOLD && Math.abs(dy) < TABLE_REFERENCE_DRAG_THRESHOLD) return;
    startTableReferenceDrag(pendingTableReferenceDrag.payload);
  }
  if (draggingTableReferencePayload) {
    event.preventDefault();
    document.getSelection()?.removeAllRanges();
  }
}

function onTableReferenceMouseUp(event: MouseEvent) {
  const payload = draggingTableReferencePayload;
  if (payload) {
    suppressNextTableReferenceClick = true;
    const target = document.elementFromPoint(event.clientX, event.clientY);
    if (target instanceof Element && target.closest("[data-query-editor-root]")) {
      window.dispatchEvent(
        createTableReferenceDropEvent({
          payload,
          clientX: event.clientX,
          clientY: event.clientY,
        }),
      );
    }
  }
  finishTableReferenceDrag();
}

function startTableReferenceMouseDrag(event: MouseEvent) {
  if (event.button !== 0) return;
  const payload = tableReferenceDragPayload();
  if (!payload) return;
  event.preventDefault();
  document.getSelection()?.removeAllRanges();
  document.body.classList.add(TABLE_REFERENCE_DRAGGING_CLASS);
  pendingTableReferenceDrag = { payload, startX: event.clientX, startY: event.clientY };
  document.addEventListener("mousemove", onTableReferenceMouseMove, true);
  document.addEventListener("mouseup", onTableReferenceMouseUp, true);
}

function onRowMouseDown(event: MouseEvent) {
  if (isDraggable.value) {
    startDrag(event, activeNode.value.id, activeNode.value.type);
  } else if (canDragTableReference.value) {
    startTableReferenceMouseDrag(event);
  }
}

watch(
  () => props.node,
  (node, previousNode) => {
    activeNode.value = node;
    if (node.id === previousNode.id) return;
    // Virtual rows are recycled; transient DOM and pointer state must not leak
    // from the previously rendered node into the new row.
    isRenamingGroup.value = false;
    renameInput.value = "";
    labelOverflowing.value = false;
    suppressNextTableReferenceClick = false;
    handleMouseLeave();
    finishTableReferenceDrag();
  },
  { flush: "sync" },
);

onBeforeUnmount(() => {
  stopPasteHandlerRegistration();
  handleMouseLeave();
  trailingCommentResizeObserver?.disconnect();
  cancelTrailingCommentMeasure();
  finishTableReferenceDrag();
});

function onToggleClick() {
  selectSingleTreeNode(props.node);
  rowRef.value?.focus({ preventScroll: true });
  treeRuntime.toggleNode(props.node);
}

function onToggleMouseDown(event: MouseEvent) {
  if (event.button !== 0) return;
  selectSingleTreeNode(props.node);
  rowRef.value?.focus({ preventScroll: true });
}

function onClick(event: MouseEvent) {
  if (suppressNextTableReferenceClick) {
    suppressNextTableReferenceClick = false;
    event.preventDefault();
    event.stopPropagation();
    return;
  }
  // The tree container clears selection on blank-area clicks, so row clicks
  // must remain isolated while the tree-level runtime performs the action.
  event.stopPropagation();
  const openMode = dataTabOpenModeFromTreeClick(props.node.type, event, settingsStore.editorSettings.shortcuts.openDataInNewTab);
  if (openMode === "new-tab") {
    event.preventDefault();
    if (event.detail > 1) return;
    selectSingleTreeNode(props.node);
    rowRef.value?.focus({ preventScroll: true });
    treeRuntime.openDataInNewTab(props.node);
    return;
  }
  if (event.shiftKey) {
    selectTreeNodeRange(props.node);
    rowRef.value?.focus({ preventScroll: true });
    return;
  }
  if (event.metaKey || event.ctrlKey) {
    toggleTreeNodeSelection(props.node);
    rowRef.value?.focus({ preventScroll: true });
    return;
  }
  selectSingleTreeNode(props.node);
  rowRef.value?.focus({ preventScroll: true });
  if (settingsStore.editorSettings.sidebarActivation === "double") return;
  treeRuntime.handleRowClick(props.node, event.detail);
}

function onDoubleClick(event: MouseEvent) {
  treeRuntime.handleRowDoubleClick(props.node, event);
}

function onTreeItemContextMenu(event: MouseEvent) {
  if (!connectionStore.selectedTreeNodeIds.includes(props.node.id)) selectSingleTreeNode(props.node);
  else connectionStore.selectedTreeNodeId = props.node.id;
  rowRef.value?.focus({ preventScroll: true });
  emit("context-menu", event, props.node);
}

function treeItemMenuItems(): ContextMenuItem[] {
  const node = props.node;
  const items: ContextMenuItem[] = [];
  const batchDropCount = selectedBatchDropTargets().length;
  const deleteMenuLabel = (singleLabel: string) => (batchDropCount > 1 ? batchDropMenuLabel() : singleLabel);
  const deleteMenuAction = (singleAction: () => void) => (batchDropCount > 1 ? requestBatchDrop : singleAction);

  // 1. Pin toggle
  if (canPin.value) {
    items.push({
      label: isPinned.value ? t("contextMenu.unpin") : t("contextMenu.pin"),
      action: togglePin,
      icon: Pin,
    });
    if (hasTypeMenu.value) items.push({ label: "", separator: true });
  }

  // 2. Connection
  if (node.type === "connection") {
    if (!isConnected.value) {
      items.push({ label: t("contextMenu.openConnection"), action: toggle, icon: Plug });
    } else {
      items.push({ label: t("contextMenu.closeConnection"), action: disconnectConnection, icon: Unplug });
    }
    items.push({ label: t("contextMenu.newQuery"), action: newQuery, icon: TerminalSquare });
    const sqlHistoryMenu = savedSqlHistorySubmenu();
    if (sqlHistoryMenu) items.push(sqlHistoryMenu);
    if (supportsDatabaseUserAdmin(currentDatabaseType())) {
      items.push({ label: t("contextMenu.userAdmin"), action: openUserAdmin, icon: UsersRound });
    }
    if (canCopyFinalProxyPort.value) {
      items.push({ label: t("contextMenu.copyFinalProxyPort"), action: copyFinalProxyPort, icon: Network });
    }
    if (canOpenSqlFileExecution.value) {
      items.push({ label: t("sqlFile.title"), action: openSqlFileExecution, icon: FileCode });
    }
    if (canCreateDatabase.value) {
      items.push({
        label: isDuckDbConnection.value ? t("contextMenu.createDuckDbFile") : t("contextMenu.createDatabase"),
        action: openCreateDatabase,
        icon: Plus,
      });
    }
    items.push({ label: "", separator: true });
    if (availableGroups.value.length > 0 || currentGroupId.value) {
      const groupChildren: ContextMenuItem[] = availableGroups.value.map((group: { id: string; name: string }) => ({
        label: group.name,
        action: () => moveToGroup(group.id),
        icon: FolderOpen,
        disabled: group.id === currentGroupId.value,
      }));
      if (currentGroupId.value) {
        groupChildren.push({ label: "", separator: true });
        groupChildren.push({ label: t("connectionGroup.ungrouped"), action: () => moveToGroup(null) });
      }
      groupChildren.push({ label: "", separator: true });
      groupChildren.push({ label: t("connectionGroup.newGroup"), action: moveToNewGroup, icon: FolderPlus });
      items.push({ label: t("connectionGroup.moveToGroup"), icon: FolderInput, children: groupChildren });
    } else {
      items.push({ label: t("connectionGroup.moveToNewGroup"), action: moveToNewGroup, icon: FolderPlus });
    }
    items.push({
      label: t("contextMenu.refreshChildren"),
      action: refresh,
      icon: RefreshCw,
      shortcut: shortcutRefresh,
    });
    if (canRefreshExternalSnapshot.value) {
      items.push({
        label: t("contextMenu.refreshExternalSnapshot"),
        action: refreshExternalSnapshot,
        icon: RefreshCw,
      });
    }
    if (canConfigureVisibleDatabases.value) {
      items.push({
        label: t("contextMenu.selectVisibleDatabases"),
        action: openVisibleDatabasesDialog,
        icon: ListFilter,
      });
    }
    items.push({ label: t("contextMenu.editConnection"), action: editConnection, icon: Pencil });
    if (revealConnectionFilePath.value) {
      items.push({
        label: t("contextMenu.revealDatabaseFile"),
        action: revealDatabaseFile,
        icon: FolderOpen,
      });
    }
    if (canBackupSqliteDatabase.value) {
      items.push({
        label: t("contextMenu.backupSqliteDatabase"),
        action: backupSqliteDatabase,
        icon: HardDriveDownload,
      });
    }
    items.push({ label: t("contextMenu.duplicateConnection"), action: duplicateConnection, icon: CopyPlus });
    items.push({ label: "", separator: true });
    items.push({
      label: connectionDeleteMenuLabel(),
      action: deleteConnection,
      icon: Trash2,
      shortcut: shortcutDelete,
      variant: "destructive" as const,
    });
    return items;
  }

  // 3. Connection Group
  if (node.type === "connection-group") {
    items.push({ label: t("contextMenu.copyName"), action: copyName, icon: Copy, shortcut: shortcutCopyName.value });
    items.push({ label: "", separator: true });
    items.push({ label: t("toolbar.newConnection"), action: newConnectionInGroup, icon: Plus });
    items.push({ label: t("connectionGroup.newGroup"), action: newSubgroup, icon: FolderPlus });
    items.push({ label: "", separator: true });
    items.push({
      label: t("connectionGroup.renameGroup"),
      action: startRenameGroup,
      icon: Pencil,
      shortcut: shortcutRename,
    });
    items.push({ label: "", separator: true });
    items.push({
      label: t("connectionGroup.deleteGroup"),
      action: deleteConnectionGroup,
      icon: Trash2,
      shortcut: shortcutDelete,
      variant: "destructive" as const,
    });
    return items;
  }

  // 4. Database / Schema
  if (node.type === "database" || node.type === "schema") {
    items.push({ label: t("contextMenu.copyName"), action: copyName, icon: Copy, shortcut: shortcutCopyName.value });
    items.push({ label: "", separator: true });
    if (canOpenObjectBrowser.value) {
      items.push({ label: t("contextMenu.openObjectBrowser"), action: openObjectBrowser, icon: TableProperties });
    }
    items.push({ label: t("contextMenu.newQuery"), action: newQuery, icon: TerminalSquare });
    const sqlHistoryMenu = savedSqlHistorySubmenu();
    if (sqlHistoryMenu) items.push(sqlHistoryMenu);
    if (node.type === "database") {
      if (!isNodeDefaultDatabase.value) {
        items.push({ label: t("contextMenu.setDefaultDatabase"), action: setNodeAsDefaultDatabase, icon: Database });
      } else {
        items.push({ label: t("contextMenu.clearDefaultDatabase"), action: clearNodeDefaultDatabase, icon: Database });
      }
    }
    if (canCreateTable.value) {
      items.push({ label: t("contextMenu.createTable"), action: createTable, icon: Plus });
    }
    if (canCreateSchema.value) {
      items.push({ label: t("contextMenu.createSchema"), action: openCreateSchemaDialog, icon: Plus });
    }
    if (canOpenSqlFileExecution.value) {
      items.push({ label: t("sqlFile.title"), action: openSqlFileExecution, icon: FileCode });
    }
    if (canOpenDiagram.value) {
      items.push({ label: t("diagram.open"), action: openDiagram, icon: Network });
    }
    if (canOpenDatabaseSearch.value) {
      items.push({ label: t("databaseSearch.open"), action: openDatabaseSearch, icon: Search });
    }
    items.push({
      label: t("contextMenu.refreshChildren"),
      action: refresh,
      icon: RefreshCw,
      shortcut: shortcutRefresh,
    });
    items.push({ label: "", separator: true });
    items.push({ label: t("transfer.dataTransfer"), action: openTransfer, icon: ArrowRightLeft });
    items.push({ label: t("diff.title"), action: openSchemaDiff, icon: ArrowRightLeft });
    items.push({ label: t("dataCompare.title"), action: openDataCompare, icon: ArrowRightLeft });
    items.push({ label: t("contextMenu.exportDatabase"), action: openDatabaseExport, icon: Upload });
    if (canCloseDatabaseConnection.value) {
      items.push({ label: "", separator: true });
      items.push({ label: t("contextMenu.closeDatabaseConnection"), action: closeDatabaseConnection, icon: Unplug });
    }
    if (canDropDatabase.value || canDropSchema.value) {
      items.push({ label: "", separator: true });
    }
    if (canDropDatabase.value) {
      items.push({
        label: t("contextMenu.dropDatabase"),
        action: dropDatabase,
        icon: Trash2,
        shortcut: shortcutDelete,
        variant: "destructive" as const,
      });
    }
    if (canDropSchema.value) {
      items.push({
        label: t("contextMenu.dropSchema"),
        action: dropSchema,
        icon: Trash2,
        shortcut: shortcutDelete,
        variant: "destructive" as const,
      });
    }
    return items;
  }

  // 5. Redis DB / Mongo DB
  if (node.type === "etcd-root") {
    items.push({ label: t("contextMenu.openConnection"), action: toggle, icon: Database });
    return items;
  }

  if (node.type === "user-admin") {
    items.push({ label: t("contextMenu.openUserAdmin"), action: openUserAdmin, icon: UsersRound });
    return items;
  }

  if (node.type === "redis-db" || node.type === "mongo-db") {
    items.push({ label: t("contextMenu.newQuery"), action: newQuery, icon: TerminalSquare });
    if (!isNodeDefaultDatabase.value) {
      items.push({ label: t("contextMenu.setDefaultDatabase"), action: setNodeAsDefaultDatabase, icon: Database });
    } else {
      items.push({ label: t("contextMenu.clearDefaultDatabase"), action: clearNodeDefaultDatabase, icon: Database });
    }
    if (node.type === "redis-db") {
      items.push({ label: "", separator: true });
      items.push({ label: t("redis.flushDb"), action: flushRedisDb, icon: Eraser, variant: "destructive" as const });
    }
    return items;
  }

  if (node.type === "elasticsearch-index" || node.type === "vector-collection") {
    items.push({ label: t("contextMenu.copyName"), action: copyName, icon: Copy, shortcut: shortcutCopyName.value });
    items.push({ label: "", separator: true });
    items.push({ label: t("contextMenu.viewData"), action: toggle, icon: TableProperties });
    items.push({ label: t("contextMenu.newQuery"), action: newQuery, icon: TerminalSquare });
    return items;
  }

  // 6. Table / View / Materialized View
  if (node.type === "table" || node.type === "view" || node.type === "materialized_view") {
    items.push({ label: t("contextMenu.copyName"), action: copyName, icon: Copy, shortcut: shortcutCopyName.value });
    items.push({ label: "", separator: true });
    items.push({ label: t("contextMenu.viewData"), action: openData, icon: TableProperties });
    if (node.type === "table") {
      items.push({
        label: t("contextMenu.viewDdl"),
        action: () => {
          ddlTarget.value = node;
          showDdlDialog.value = true;
        },
        icon: FileCode,
      });
    }
    if (node.type === "view" || node.type === "materialized_view") {
      items.push({ label: t("contextMenu.editView"), action: viewObjectSource, icon: Pencil });
      items.push({ label: t("contextMenu.viewSource"), action: viewObjectSource, icon: Code2 });
      items.push({ label: t("contextMenu.viewDdl"), action: viewObjectDdl, icon: FileCode });
    }
    if (canOpenStructureEditor.value) {
      items.push({ label: t("contextMenu.editStructure"), action: openStructureEditor, icon: PencilRuler });
    }
    if (canRenameObject.value) {
      items.push({
        label: t("contextMenu.renameObject"),
        action: openRenameObjectDialog,
        icon: Pencil,
        shortcut: shortcutRename,
      });
    }
    if (node.type === "view" || node.type === "materialized_view") {
      items.push({
        label: deleteMenuLabel(t("contextMenu.dropView")),
        action: deleteMenuAction(requestDropObject),
        icon: Trash2,
        shortcut: shortcutDelete,
        variant: "destructive" as const,
      });
    }
    items.push({
      label: t("contextMenu.generateSql"),
      icon: FilePlus,
      children: isTableNotView.value
        ? [
            { label: "SELECT", action: newSelectTemplate, icon: TerminalSquare },
            { label: "INSERT", action: newInsertTemplate, icon: FilePlus },
            { label: "UPDATE", action: newUpdateTemplate, icon: SquarePen },
            { label: "DELETE", action: newDeleteTemplate, icon: ListX },
            { label: "DDL", action: generateDdlTemplate, icon: FileCode },
          ]
        : [
            { label: "SELECT", action: newSelectTemplate, icon: TerminalSquare },
            { label: "DDL", action: generateDdlTemplate, icon: FileCode },
          ],
    });
    const sqlHistoryMenu = savedSqlHistorySubmenu();
    if (sqlHistoryMenu) items.push(sqlHistoryMenu);
    if (canOpenDiagram.value) {
      items.push({ label: t("diagram.open"), action: openDiagram, icon: Network });
    }
    if (canOpenTableImport.value) {
      items.push({ label: t("contextMenu.importData"), action: openTableImport, icon: Download });
    }
    if (isTableNotView.value) {
      items.push({ label: t("dataCompare.title"), action: openDataCompare, icon: ArrowRightLeft });
    }
    items.push({ label: "", separator: true });
    items.push(exportDataSubmenu());
    items.push({ label: t("contextMenu.exportDatabase"), action: openDatabaseExport, icon: Upload });
    items.push({ label: t("contextMenu.exportStructure"), action: exportStructure, icon: FileCode });
    items.push(copyStructureAsSubmenu());
    if (isTableNotView.value) {
      items.push({ label: "", separator: true });
      items.push({ label: t("contextMenu.duplicateStructure"), action: duplicateStructure, icon: CopyPlus });
      items.push({ label: "", separator: true });
      if (supportsTruncate.value) {
        items.push({
          label: t("contextMenu.truncateTable"),
          action: truncateTable,
          icon: Scissors,
          variant: "destructive" as const,
        });
      }
      items.push({
        label: t("contextMenu.emptyTable"),
        action: emptyTable,
        icon: Eraser,
        variant: "destructive" as const,
      });
      items.push({
        label: deleteMenuLabel(t("contextMenu.dropTable")),
        action: deleteMenuAction(dropTable),
        icon: Trash2,
        shortcut: shortcutDelete,
        variant: "destructive" as const,
      });
    }
    items.push({ label: "", separator: true });
    items.push({
      label: t("contextMenu.refreshChildren"),
      action: refresh,
      icon: RefreshCw,
      shortcut: shortcutRefresh,
    });
    return items;
  }

  // 7. Column
  if (node.type === "column") {
    items.push({ label: t("contextMenu.copyName"), action: copyName, icon: Copy, shortcut: shortcutCopyName.value });
    if (canOpenFieldLineage.value) {
      items.push({ label: "", separator: true });
      items.push({ label: t("lineage.open"), action: openFieldLineage, icon: Network });
    }
    if (canDropTableChildObject.value) {
      items.push({ label: "", separator: true });
      items.push({
        label: deleteMenuLabel(dropTableChildObjectMenuLabel()),
        action: deleteMenuAction(requestDropTableChildObject),
        icon: Trash2,
        shortcut: shortcutDelete,
        variant: "destructive" as const,
      });
    }
    return items;
  }

  if (node.type === "index" || node.type === "fkey" || node.type === "trigger") {
    items.push({ label: t("contextMenu.copyName"), action: copyName, icon: Copy, shortcut: shortcutCopyName.value });
    if (canDropTableChildObject.value) {
      items.push({ label: "", separator: true });
      items.push({
        label: deleteMenuLabel(dropTableChildObjectMenuLabel()),
        action: deleteMenuAction(requestDropTableChildObject),
        icon: Trash2,
        shortcut: shortcutDelete,
        variant: "destructive" as const,
      });
    }
    return items;
  }

  // 8. Procedure / Function / Package
  if (node.type === "procedure" || node.type === "function") {
    if (node.type === "procedure") {
      items.push({ label: t("contextMenu.executeProcedure"), action: openProcedureExecution, icon: Play });
    }
    items.push({ label: t("contextMenu.viewSource"), action: viewObjectSource, icon: Code2 });
    if (canRenameObject.value) {
      items.push({
        label: t("contextMenu.renameObject"),
        action: openRenameObjectDialog,
        icon: Pencil,
        shortcut: shortcutRename,
      });
    }
    items.push({ label: "", separator: true });
    items.push({
      label: deleteMenuLabel(node.type === "procedure" ? t("contextMenu.dropProcedure") : t("contextMenu.dropFunction")),
      action: deleteMenuAction(requestDropObject),
      icon: Trash2,
      shortcut: shortcutDelete,
      variant: "destructive" as const,
    });
    return items;
  }

  if (node.type === "sequence") {
    items.push({ label: t("contextMenu.viewSource"), action: viewObjectSource, icon: Code2 });
    items.push({ label: "", separator: true });
    items.push({ label: t("contextMenu.copyName"), action: copyName, icon: Copy, shortcut: shortcutCopyName.value });
    return items;
  }

  if (node.type === "package" || node.type === "package-body") {
    items.push({ label: t("contextMenu.viewSource"), action: viewObjectSource, icon: Code2 });
    items.push({ label: "", separator: true });
    items.push({ label: t("contextMenu.copyName"), action: copyName, icon: Copy, shortcut: shortcutCopyName.value });
    return items;
  }

  // 9. Group Labels (group-columns, group-tables, etc.)
  if (isGroupLabel(node)) {
    const hasGroupCreateAction = (node.type === "group-tables" && canCreateTable.value) || (node.type === "group-views" && !!node.connectionId && !!node.database);
    if (node.type === "group-tables" && canCreateTable.value) {
      items.push({ label: t("contextMenu.createTable"), action: createTable, icon: Plus });
    }
    if (node.type === "group-views" && node.connectionId && node.database) {
      items.push({ label: t("contextMenu.createView"), action: createView, icon: Plus });
    }
    if (hasGroupCreateAction) {
      items.push({ label: "", separator: true });
    }
    if (node.type !== "group-partitions") {
      items.push({
        label: t("contextMenu.refreshChildren"),
        action: refresh,
        icon: RefreshCw,
        shortcut: shortcutRefresh,
      });
    }
    return items;
  }

  // 10. Universal Copy Name (for all types except connection)
  if (hasTypeMenu.value) {
    items.push({ label: "", separator: true });
    items.push({ label: t("contextMenu.copyName"), action: copyName, icon: Copy, shortcut: shortcutCopyName.value });
  }

  return items;
}
</script>

<template>
  <div v-if="node.type === 'table-search-control'" class="tree-table-search-control flex h-7 items-center py-0.5 pr-2" :style="tableSearchStyle" @click.stop @dblclick.stop @mousedown.stop @keydown.stop>
    <div class="relative w-full min-w-0">
      <Search class="pointer-events-none absolute left-2 top-1/2 h-3 w-3 -translate-y-1/2 text-muted-foreground" />
      <Input
        :model-value="tableSearchValue"
        autocapitalize="off"
        autocorrect="off"
        spellcheck="false"
        class="h-6 w-full rounded border pl-7 pr-6 text-xs shadow-none focus-visible:ring-1"
        :style="{ backgroundColor: 'var(--tree-table-search-input-bg)', borderColor: 'var(--tree-table-search-border)' }"
        :placeholder="t(node.label)"
        :aria-label="t(node.label)"
        :data-sidebar-table-search-parent-id="tableSearchParentId"
        @update:model-value="updateTableSearchQuery"
      />
      <button v-if="tableSearchValue" type="button" class="absolute right-1.5 top-1/2 flex h-4 w-4 -translate-y-1/2 items-center justify-center rounded text-muted-foreground hover:bg-muted hover:text-foreground" :aria-label="t('sidebar.clearTableSearch')" @click.stop="clearTableSearchQuery">
        <X class="h-3 w-3" />
      </button>
    </div>
  </div>

  <div v-else @contextmenu="onTreeItemContextMenu">
    <LightTooltip :text="displayLabel(node)" :disabled="isTooltipDisabled()" side="right" :side-offset="8" :delay="0" :close-delay="0" :surface="detailTooltip ? 'popover' : 'foreground'">
      <div
        ref="rowRef"
        class="group flex items-center gap-2 py-1 px-2 cursor-pointer relative outline-none"
        style="contain: layout style"
        :class="[
          rowWidthClass,
          {
            'group/sidebar-row': true,
            'ring-1 ring-primary/50 bg-primary/5': dragVisual.showInside,
            'opacity-50': dragVisual.dragging,
            'tree-item-connection-tint': connectionColor,
            'hover:bg-accent': node.type !== 'connection',
            'hover:bg-secondary/60': node.type === 'connection',
            rounded: !selectionVisual.rowSelected,
            'tree-item-active': selectionVisual.rowSelected,
            'tree-item-active--selection-set': selectionVisual.usesSelectionSetHighlight && selectionVisual.rowSelected,
            'tree-item-highlight': highlighted,
          },
        ]"
        :tabindex="selectionVisual.selected || selectionVisual.multiSelected ? 0 : -1"
        :style="rowStyle"
        @click="onClick"
        @dblclick="onDoubleClick"
        @keydown="onKeydown"
        @mousedown="onRowMouseDown"
        @mousemove="dragVisual.isDropTarget ? updateTarget($event, node.id, node.type) : undefined"
        @mouseenter="handleMouseEnter"
        @mouseleave="
          clearTarget(node.id);
          handleMouseLeave();
        "
      >
        <div v-if="dragVisual.showBefore" class="absolute right-2 top-0 h-0.5 bg-primary rounded-full pointer-events-none" :style="{ left: paddingLeft }" />
        <div v-if="dragVisual.showAfter" class="absolute right-2 bottom-0 h-0.5 bg-primary rounded-full pointer-events-none" :style="{ left: paddingLeft }" />
        <template v-if="canExpand">
          <button type="button" class="-m-0.5 flex h-4 w-4 shrink-0 items-center justify-center rounded-sm text-muted-foreground hover:bg-muted hover:text-foreground" @mousedown.stop="onToggleMouseDown" @click.stop="onToggleClick">
            <Loader2 v-if="node.isLoading" class="w-3.5 h-3.5 animate-spin" />
            <ChevronDown v-else-if="node.isExpanded" class="w-3.5 h-3.5" />
            <ChevronRight v-else class="w-3.5 h-3.5" />
          </button>
        </template>
        <span v-else class="w-3.5 h-3.5 shrink-0" />
        <DatabaseIcon v-if="node.type === 'connection'" :db-type="connectionIconType(node.connectionId)" class="h-3.5 w-3.5 shrink-0" />
        <Loader2 v-else-if="node.type === 'load-more' && node.isLoading" class="w-3.5 h-3.5 shrink-0 animate-spin text-primary" />
        <component v-else :is="getIconInfo(node)?.icon || Database" class="w-3.5 h-3.5 shrink-0" :class="databaseOpenVisual.iconClass" />
        <div ref="trailingCommentLayoutRef" :class="hasTrailingMetadata() ? 'flex flex-1 min-w-0 items-center' : 'contents'">
          <div
            ref="trailingCommentLeadingRef"
            :class="trailingComment ? 'flex max-w-full min-w-0 shrink-0 items-center gap-2' : formattedObjectStorage() ? 'flex min-w-0 flex-1 items-center gap-2' : 'contents'"
            :style="alignedCommentLabelWidth ? { width: `${alignedCommentLabelWidth}px` } : undefined"
          >
            <input
              v-if="isRenamingGroup"
              ref="renameInputRef"
              v-model="renameInput"
              class="min-w-0 flex-1 truncate bg-transparent border border-primary/50 rounded px-1 outline-none"
              @blur="finishRenameGroup"
              @keydown.enter.prevent="finishRenameGroup"
              @keydown.escape.prevent="isRenamingGroup = false"
              @click.stop
            />
            <span v-else ref="labelRef" :class="labelWidthClass">{{ visibleLabel(node) }}</span>
            <ProductionContextBadge v-if="showProductionBadge" compact />
            <span
              v-if="
                (node.type === 'group-tables' || node.type === 'group-views' || node.type === 'group-materialized-views' || node.type === 'group-procedures' || node.type === 'group-functions' || node.type === 'group-sequences' || node.type === 'group-packages' || node.type === 'group-partitions') &&
                node.objectCount != null
              "
              class="text-muted-foreground text-[10px] shrink-0"
              >{{ node.objectCount }}</span
            >
            <Badge v-if="isNodeDefaultDatabase" variant="secondary" class="h-4 px-1.5 text-[10px]">
              {{ t("editor.defaultDatabase") }}
            </Badge>
          </div>
          <span v-if="trailingComment && !isRightAlignedComment()" class="sidebar-object-comment ml-2 min-w-0 flex-1 truncate text-left" :class="{ 'sidebar-object-comment--windows': useWindowsSidebarCommentFont }">{{ trailingComment }}</span>
          <span v-if="isRightAlignedComment() && trailingCommentMaxWidth > 0" class="min-w-0 flex-1" aria-hidden="true" />
          <span
            v-if="isRightAlignedComment() && trailingCommentMaxWidth > 0"
            class="sidebar-object-comment sidebar-object-comment--right min-w-0 shrink-0 truncate text-left"
            :class="{ 'sidebar-object-comment--windows': useWindowsSidebarCommentFont }"
            :style="{ marginLeft: `${trailingCommentGapPx}px`, maxWidth: `${trailingCommentMaxWidth}px` }"
            >{{ trailingComment }}</span
          >
        </div>
        <span v-if="node.type === 'connection' && node.connectionId && connectionStore.connectedIds.has(node.connectionId)" class="w-1.5 h-1.5 rounded-full bg-green-500 shrink-0" />
        <span v-if="databaseOpenVisual.showsIndicator" class="w-1.5 h-1.5 rounded-full bg-green-500 shrink-0" />
        <Badge v-if="isConnectionReadonly" variant="secondary" class="h-4 px-1.5 text-[10px] gap-0.5"><Lock class="w-2.5 h-2.5" />{{ t("connection.readOnlyBadge") }}</Badge>
        <ConnectionErrorIndicator v-if="node.type === 'connection'" :connection-id="node.connectionId" trigger-class="h-4 w-4" />
        <Pin v-if="isPinned" class="w-3 h-3 shrink-0 text-primary fill-current" aria-hidden="true" />
        <span v-if="formattedObjectStorage()" class="ml-auto shrink-0 text-right text-xs tabular-nums text-muted-foreground">{{ formattedObjectStorage() }}</span>
        <button
          v-if="isConnecting"
          type="button"
          class="ml-auto flex h-4 w-4 shrink-0 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-secondary/45 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          :aria-label="t('connection.cancelConnecting')"
          :title="t('connection.cancelConnecting')"
          @mousedown.stop
          @click.stop="cancelConnectionAttempt"
        >
          <X class="h-3 w-3" />
        </button>
        <button
          v-if="node.type === 'connection'"
          type="button"
          class="flex h-4 w-4 shrink-0 items-center justify-center rounded text-muted-foreground/55 opacity-0 transition-colors transition-opacity hover:bg-secondary/45 hover:text-foreground focus-visible:opacity-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring group-hover/sidebar-row:opacity-100"
          :class="[{ 'opacity-100': isConnectionSelectionChecked || connectionStore.connectionMultiSelectActive }, isConnecting ? '' : 'ml-auto']"
          :aria-label="isConnectionSelectionChecked ? t('connectionGroup.deselectConnection') : t('connectionGroup.selectConnection')"
          @mousedown.stop
          @click="toggleConnectionMultiSelection"
        >
          <Check v-if="isConnectionSelectionChecked" class="h-3 w-3 text-primary" />
          <Square v-else class="h-3 w-3 stroke-[1.7]" />
        </button>
      </div>
      <template v-if="detailTooltip" #content>
        <div class="w-max min-w-40 max-w-[min(28rem,calc(100vw-24px))] rounded-md border border-border bg-popover p-2 text-popover-foreground shadow-lg">
          <div class="space-y-1">
            <div v-for="row in detailTooltip.rows" :key="row.label" class="grid grid-cols-[max-content_minmax(0,1fr)] gap-2 text-xs leading-5">
              <span class="text-muted-foreground">{{ row.label }}</span>
              <span v-if="row.multiline" class="max-h-20 overflow-hidden whitespace-pre-wrap break-words text-foreground/90">
                {{ row.value }}
              </span>
              <span v-else class="truncate font-mono text-foreground/90" :title="row.value">{{ row.value }}</span>
            </div>
          </div>
        </div>
      </template>
    </LightTooltip>
  </div>
</template>

<style>
.sidebar-object-comment {
  color: var(--muted-foreground);
  font-size: 12px;
  line-height: 1rem;
  opacity: 0.6;
  /* Sidebar rows repaint on hover; avoid heavier font shaping and fallback here. */
  text-rendering: auto;
}

.sidebar-object-comment--right {
  width: max-content;
  max-width: 100%;
  flex-shrink: 999;
}

.sidebar-object-comment--windows {
  font-family: "Microsoft YaHei UI", "Microsoft YaHei", "Segoe UI", system-ui, sans-serif;
  font-size: 14px;
  font-weight: 500;
  opacity: 1;
}

.tree-item-connection-tint {
  isolation: isolate;
  background-color: transparent !important;
}

.tree-item-connection-tint::before {
  content: "";
  position: absolute;
  inset: 0 -9999px;
  z-index: 0;
  background-color: var(--tree-connection-row-bg);
  border-radius: inherit;
  pointer-events: none;
}

.tree-item-connection-tint > * {
  position: relative;
  z-index: 1;
}

.tree-item-connection-tint:hover,
.tree-item-connection-tint.tree-item-active,
.tree-item-connection-tint.tree-item-active:focus {
  background-color: transparent !important;
}

.tree-item-connection-tint:hover::before {
  background-color: var(--tree-connection-row-hover-bg, var(--tree-connection-row-bg));
}

.tree-item-connection-tint.tree-item-active::before {
  background-color: var(--tree-connection-active-bg, var(--tree-connection-row-bg));
}

.tree-item-connection-tint.tree-item-active:focus::before {
  background-color: var(--tree-connection-active-focus-bg, var(--tree-connection-active-bg));
}

.tree-item-connection-tint.tree-item-active--selection-set:focus::before {
  background-color: var(--tree-connection-active-bg, var(--tree-connection-row-bg));
}

.tree-table-search-control {
  position: relative;
  isolation: isolate;
  background-color: transparent;
}

.tree-table-search-control::before {
  content: "";
  position: absolute;
  inset: 0 -9999px;
  z-index: 0;
  background-color: var(--tree-table-search-row-bg);
  pointer-events: none;
}

.tree-table-search-control > * {
  position: relative;
  z-index: 1;
}

/* Unfocused: subtle gray */
.tree-item-active {
  background-color: var(--tree-connection-active-bg, rgb(235 235 235)) !important;
}
:root.dark .tree-item-active {
  background-color: var(--tree-connection-active-bg, rgb(36 36 36)) !important;
}

/* Focused: soft blue */
.tree-item-active:focus {
  background-color: var(--tree-connection-active-focus-bg, rgb(211 227 245)) !important;
}
:root.dark .tree-item-active:focus {
  background-color: var(--tree-connection-active-focus-bg, rgb(33 60 89)) !important;
}

/* Multi-selection treats every selected row as equal; keep focus neutral. */
.tree-item-active--selection-set:focus {
  background-color: var(--tree-connection-active-bg, rgb(235 235 235)) !important;
  box-shadow: inset 0 0 0 1px hsl(var(--foreground) / 0.14);
}
:root.dark .tree-item-active--selection-set:focus {
  background-color: var(--tree-connection-active-bg, rgb(36 36 36)) !important;
  box-shadow: inset 0 0 0 1px hsl(var(--foreground) / 0.18);
}

/* Locate highlight: instant amber, then fade on removal */
.tree-item-highlight {
  background-color: rgb(253 225 167) !important;
  background-color: oklch(0.92 0.08 85) !important;
  transition: background-color 0.28s ease-out;
}

:root.dark .tree-item-highlight {
  background-color: rgb(110 67 0) !important;
  background-color: oklch(0.42 0.12 80) !important;
  transition: background-color 0.28s ease-out;
}

.tree-item-connection-tint.tree-item-highlight::before {
  background-color: rgb(253 225 167) !important;
  background-color: oklch(0.92 0.08 85) !important;
}

:root.dark .tree-item-connection-tint.tree-item-highlight::before {
  background-color: rgb(110 67 0) !important;
  background-color: oklch(0.42 0.12 80) !important;
}
</style>
