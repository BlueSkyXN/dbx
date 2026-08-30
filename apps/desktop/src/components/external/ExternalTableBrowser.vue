<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { ChevronLeft, ChevronRight, Loader2, RefreshCw, ShieldAlert } from "@lucide/vue";
import { useI18n } from "vue-i18n";

import DataGrid from "@/components/grid/DataGrid.vue";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import type { CustomSaveHandler, CustomSaveResult } from "@/composables/useDataGridEditor";
import * as api from "@/lib/backend/api";
import { isTauriRuntime } from "@/lib/backend/tauriRuntime";
import { buildExternalSavePlan, customSaveResultFromExternal, externalSavePreview, externalValueForGrid, gridValueForExternal } from "@/lib/externalTable/externalTableEditing";
import { useConnectionStore } from "@/stores/connectionStore";
import type { QueryResult } from "@/types/database";
import type { ExternalTableRef, ExternalTableSchema, PageSnapshot } from "@/types/externalTable";

const props = defineProps<{
  connectionId: string;
  pendingStateKey: string;
}>();

type DataGridHandle = {
  hasPendingChanges: () => boolean;
  isCustomSaveBlocked: () => boolean;
  discardPendingChanges: () => void;
};

const { t } = useI18n();
const connectionStore = useConnectionStore();
const gridRef = ref<DataGridHandle>();
const tables = ref<ExternalTableRef[]>([]);
const selectedTableKey = ref("");
const schema = ref<ExternalTableSchema>();
const page = ref<PageSnapshot>();
const loading = ref(false);
const error = ref("");
const saveStatus = ref("");
const saveBlocked = ref(false);
const currentCursor = ref<string | null>(null);
const previousCursors = ref<Array<string | null>>([]);
let loadGeneration = 0;
const pageNumber = computed(() => previousCursors.value.length + 1);
const connection = computed(() => connectionStore.getConfig(props.connectionId));
const selectedTable = computed(() => tables.value.find((table) => table.tableKey === selectedTableKey.value));
const isDesktop = isTauriRuntime();

function queryColumnType(valueType: string) {
  if (valueType === "number") return "number";
  if (valueType === "boolean") return "boolean";
  if (valueType === "date_time") return "datetime";
  if (valueType === "json") return "json";
  return "text";
}

const gridResult = computed<QueryResult>(() => ({
  columns: (page.value?.columns ?? schema.value?.columns ?? []).map((column) => column.displayName),
  column_types: (page.value?.columns ?? schema.value?.columns ?? []).map((column) => queryColumnType(column.valueType)),
  rows: (page.value?.rows ?? []).map((row) => row.values.map(externalValueForGrid)),
  affected_rows: page.value?.rows.length ?? 0,
  execution_time_ms: 0,
  total_is_exact: !page.value?.nextCursor,
  has_more: !!page.value?.nextCursor,
}));

const editable = computed(() => !!schema.value?.writable && page.value?.readState === "complete" && schema.value.capabilities.canUpdate && !saveBlocked.value);
const canInsert = computed(() => schema.value?.capabilities.insertMode === "append");
const canDelete = computed(() => schema.value?.capabilities.deleteMode !== "unsupported");

function isCellReadonly(sourceRowIndex: number, columnIndex: number) {
  const column = schema.value?.columns[columnIndex];
  const row = page.value?.rows[sourceRowIndex];
  if (!column || !row || !column.writable) return true;
  return (row.readonlyColumnKeys ?? []).includes(column.columnKey);
}

function saveSummary(result: CustomSaveResult) {
  if (result.unknown.length) return t("externalTable.saveUnknown");
  if (result.conflicts.length) return t("externalTable.saveConflict", { count: result.conflicts.length });
  if (result.saveBlocked) return t("externalTable.saveUnknown");
  if (result.rejected.length) return t("externalTable.saveRejected", { count: result.rejected.length });
  return t("externalTable.saveApplied");
}

function tableMatches(left: ExternalTableRef | undefined, right: ExternalTableRef | undefined) {
  return !!left && !!right && left.tableKey === right.tableKey;
}

function columnsMatch(left: ExternalTableSchema["columns"], right: PageSnapshot["columns"]) {
  return left.length === right.length && left.every((column, index) => column.columnKey === right[index]?.columnKey);
}

const customSaveHandler = computed<CustomSaveHandler | undefined>(() => {
  if (!schema.value || !page.value) return undefined;
  return {
    canInsert: canInsert.value,
    supportsInsert: canInsert.value,
    canDelete: canDelete.value,
    targetLabel: page.value.table.displayName,
    confirmDiscardPending: () => window.confirm(t("externalTable.discardPendingConfirm")),
    readonlyColumns: schema.value.columns.filter((column) => !column.writable).map((column) => column.displayName),
    preview: async (changes) => externalSavePreview(buildExternalSavePlan(changes, page.value!, schema.value!), schema.value!.capabilities.deleteMode),
    save: async (changes) => {
      const currentPage = page.value;
      const currentSchema = schema.value;
      if (!currentPage || !currentSchema || !tableMatches(currentPage.table, currentSchema.table) || !tableMatches(currentPage.table, selectedTable.value)) {
        throw new Error(t("externalTable.notLoaded"));
      }
      const plan = buildExternalSavePlan(changes, currentPage, currentSchema);
      if (!plan.operations.length) throw new Error(t("externalTable.noValidChanges"));
      const result = await api.externalTableApplyChanges(props.connectionId, {
        table: currentPage.table,
        snapshotToken: currentPage.snapshotToken,
        operations: plan.operations,
      });
      if (result.newSnapshotToken) currentPage.snapshotToken = result.newSnapshotToken;
      const custom = customSaveResultFromExternal(plan, result);
      saveBlocked.value = custom.saveBlocked;
      saveStatus.value = saveSummary(custom);
      return custom;
    },
    applySavedChanges: ({ dirtyRows }) => {
      const currentPage = page.value;
      const currentSchema = schema.value;
      if (!currentPage || !currentSchema) return;
      for (const [sourceRowIndex, changes] of dirtyRows) {
        const row = currentPage.rows[sourceRowIndex];
        if (!row) continue;
        for (const [columnIndex, value] of changes) {
          const column = currentSchema.columns[columnIndex];
          if (column) row.values[columnIndex] = gridValueForExternal(value, column);
        }
      }
    },
  };
});

async function loadPage(cursor: string | null, refreshSchema: boolean) {
  const table = selectedTable.value;
  if (!table) return false;
  const generation = ++loadGeneration;
  const connectionId = props.connectionId;
  loading.value = true;
  error.value = "";
  try {
    const nextSchema = refreshSchema || !schema.value ? await api.externalTableDescribe(connectionId, table) : schema.value;
    const nextPage = await api.externalTableReadPage(connectionId, { table, cursor, limit: 200 });
    if (generation !== loadGeneration || props.connectionId !== connectionId || selectedTable.value?.tableKey !== table.tableKey) {
      return false;
    }
    if (!tableMatches(nextSchema.table, nextPage.table) || !columnsMatch(nextSchema.columns, nextPage.columns)) {
      throw new Error(t("externalTable.schemaChangedDuringLoad"));
    }
    schema.value = nextSchema;
    page.value = nextPage;
    currentCursor.value = cursor;
    saveBlocked.value = false;
    saveStatus.value = nextPage.readState === "incomplete" ? t("externalTable.incompleteRead") : "";
    return true;
  } catch (cause) {
    if (generation === loadGeneration) error.value = cause instanceof Error ? cause.message : String(cause);
    return false;
  } finally {
    if (generation === loadGeneration) loading.value = false;
  }
}

async function loadTables() {
  const generation = ++loadGeneration;
  const connectionId = props.connectionId;
  loading.value = true;
  error.value = "";
  try {
    if (!isDesktop) throw new Error(t("externalTable.desktopOnly"));
    await connectionStore.ensureConnected(connectionId);
    const nextTables = await api.externalTableList(connectionId);
    if (generation !== loadGeneration || props.connectionId !== connectionId) return;
    tables.value = nextTables;
    selectedTableKey.value = tables.value[0]?.tableKey ?? "";
    schema.value = undefined;
    page.value = undefined;
    previousCursors.value = [];
    currentCursor.value = null;
    if (selectedTableKey.value) await loadPage(null, true);
  } catch (cause) {
    if (generation === loadGeneration) error.value = cause instanceof Error ? cause.message : String(cause);
  } finally {
    if (generation === loadGeneration) loading.value = false;
  }
}

function confirmDiscardPending() {
  if (!gridRef.value?.hasPendingChanges()) return true;
  if (!window.confirm(t("externalTable.discardPendingConfirm"))) return false;
  gridRef.value.discardPendingChanges();
  saveBlocked.value = false;
  return true;
}

async function refresh() {
  if (!confirmDiscardPending()) return false;
  return loadPage(currentCursor.value, true);
}

async function selectTable(tableKey: string) {
  if (tableKey === selectedTableKey.value) return;
  if (!confirmDiscardPending()) return;
  selectedTableKey.value = tableKey;
  previousCursors.value = [];
  currentCursor.value = null;
  schema.value = undefined;
  await loadPage(null, true);
}

async function nextPage() {
  const cursor = page.value?.nextCursor ?? null;
  if (!cursor || !confirmDiscardPending()) return;
  const previousCursor = currentCursor.value;
  if (await loadPage(cursor, false)) previousCursors.value.push(previousCursor);
}

async function previousPage() {
  if (!previousCursors.value.length || !confirmDiscardPending()) return;
  const cursor = previousCursors.value[previousCursors.value.length - 1] ?? null;
  if (await loadPage(cursor, false)) previousCursors.value.pop();
}

async function handleGridReload() {
  await loadPage(currentCursor.value, true);
}

watch(
  () => props.connectionId,
  () => void loadTables(),
);

onMounted(() => void loadTables());

defineExpose({ refresh });
</script>

<template>
  <div class="flex h-full min-h-0 flex-col bg-background">
    <div class="flex min-h-11 shrink-0 flex-wrap items-center gap-2 border-b px-3 py-2">
      <Select v-if="tables.length > 1" :model-value="selectedTableKey" @update:model-value="(value) => selectTable(String(value))">
        <SelectTrigger class="h-8 w-64 max-w-full" :disabled="loading">
          <SelectValue :placeholder="t('externalTable.selectTable')" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem v-for="table in tables" :key="table.tableKey" :value="table.tableKey">{{ table.displayName }}</SelectItem>
        </SelectContent>
      </Select>
      <span v-else-if="selectedTable" class="truncate text-sm font-medium">{{ selectedTable.displayName }}</span>
      <Badge v-if="page?.readState === 'incomplete'" variant="outline" class="border-amber-500/40 text-amber-700 dark:text-amber-400">{{ t("externalTable.incomplete") }}</Badge>
      <Badge v-if="schema && !schema.writable" variant="outline">{{ t("externalTable.readOnly") }}</Badge>
      <span v-if="schema?.readonlyReason" class="max-w-xl truncate text-xs text-muted-foreground" :title="schema.readonlyReason">{{ schema.readonlyReason }}</span>
      <span class="flex-1" />
      <span class="text-xs text-muted-foreground">{{ t("externalTable.page", { page: pageNumber }) }}</span>
      <Button size="icon" variant="outline" class="h-8 w-8" :disabled="loading || previousCursors.length === 0" :title="t('externalTable.previousPage')" @click="previousPage">
        <ChevronLeft class="h-4 w-4" />
      </Button>
      <Button size="icon" variant="outline" class="h-8 w-8" :disabled="loading || !page?.nextCursor" :title="t('externalTable.nextPage')" @click="nextPage">
        <ChevronRight class="h-4 w-4" />
      </Button>
      <Button size="sm" variant="outline" class="h-8 gap-1.5" :disabled="loading" @click="refresh">
        <RefreshCw class="h-3.5 w-3.5" />
        {{ t("common.refresh") }}
      </Button>
    </div>

    <div v-if="saveBlocked" class="flex shrink-0 items-center gap-2 border-b border-amber-500/30 bg-amber-500/5 px-3 py-2 text-sm text-amber-800 dark:text-amber-300">
      <ShieldAlert class="h-4 w-4 shrink-0" />
      <span class="min-w-0 flex-1">{{ saveStatus || t("externalTable.saveUnknown") }}</span>
      <Button size="sm" variant="outline" class="h-7" @click="refresh">{{ t("externalTable.reload") }}</Button>
    </div>
    <div v-else-if="saveStatus" class="shrink-0 border-b bg-muted/30 px-3 py-1.5 text-xs text-muted-foreground">{{ saveStatus }}</div>

    <div v-if="error" class="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 p-8 text-center">
      <ShieldAlert class="h-8 w-8 text-destructive" />
      <p class="max-w-2xl text-sm text-destructive">{{ error }}</p>
      <Button variant="outline" size="sm" @click="loadTables">{{ t("common.retry") }}</Button>
    </div>
    <div v-else-if="!selectedTable && !loading" class="flex min-h-0 flex-1 items-center justify-center text-sm text-muted-foreground">{{ t("externalTable.noTables") }}</div>
    <div v-else class="relative min-h-0 flex-1">
      <DataGrid
        v-if="schema && page"
        ref="gridRef"
        :result="gridResult"
        :editable="editable"
        :database-type="connection?.db_type"
        :connection-id="connectionId"
        database=""
        context="table-data"
        :pagination-enabled="false"
        :custom-save-handler="customSaveHandler"
        :allow-insert-rows="canInsert"
        :allow-delete-rows="canDelete"
        :allow-auto-refresh="false"
        :is-cell-readonly="isCellReadonly"
        :cache-key="`${pendingStateKey}:${selectedTableKey}:${currentCursor ?? 'first'}`"
        :pending-state-key="`${pendingStateKey}:${selectedTableKey}:${currentCursor ?? 'first'}`"
        :column-width-cache-key="`external:${connectionId}:${selectedTableKey}`"
        @reload="handleGridReload"
      />
      <div v-if="loading" class="absolute inset-0 z-20 flex items-center justify-center bg-background/70 backdrop-blur-[1px]">
        <Loader2 class="h-6 w-6 animate-spin text-muted-foreground" />
      </div>
    </div>
  </div>
</template>
