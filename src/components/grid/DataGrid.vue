<script lang="ts">
import { ref } from "vue";
const globalDdlOpen = ref(false);
</script>

<script setup lang="ts">
import { computed, nextTick, onUnmounted, watch } from "vue";
import { useElementSize } from "@vueuse/core";
import { useI18n } from "vue-i18n";
import {
  ArrowUp,
  ArrowDown,
  ArrowUpDown,
  Download,
  Plus,
  Trash2,
  Save,
  ChevronLeft,
  ChevronRight,
  Search,
  Inbox,
  SearchX,
  Code2,
  Copy,
  Loader2,
  X,
  Undo2,
  WrapText,
  Info,
  Rows3,
  TriangleAlert,
  RefreshCcw,
  RotateCcw,
  Pencil,
  Filter,
  FileDown,
  SquareDashed,
} from "lucide-vue-next";
import { Button } from "@/components/ui/button";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import DangerConfirmDialog from "@/components/editor/DangerConfirmDialog.vue";
import type { QueryResult, ColumnInfo, DatabaseType, ExternalRowUpdate } from "@/types/database";
import { isTauriRuntime } from "@/lib/tauriRuntime";
import * as api from "@/lib/api";
import {
  extractSelection,
  formatSelectionAsCsv,
  formatSelectionAsJson,
  formatSelectionAsSqlInList,
  formatSelectionAsTsv,
  isCellInSelection,
  normalizeSelectionRange,
  type CellPosition,
  type CellSelectionRange,
} from "@/lib/gridSelection";
import { buildTableSelectSql, quoteTableIdentifier } from "@/lib/tableSelectSql";
import { buildDataGridSaveStatements, formatGridSqlLiteral } from "@/lib/dataGridSql";
import { formatMarkdownTable } from "@/lib/markdownTable";
import { isExternalTabular } from "@/lib/databaseCapabilities";
import { externalRecordIdColumn, isFeishuBitableTableEditable } from "@/lib/externalTableEditing";
import {
  matchesRowStatusFilter,
  rowStatusFilterAfterAddingRow,
  type RowStatus,
  type RowStatusFilter,
} from "@/lib/gridRowStatus";

import { useToast } from "@/composables/useToast";

const { t } = useI18n();
const { toast } = useToast();

const props = defineProps<{
  result: QueryResult;
  sql?: string;
  editable?: boolean;
  databaseType?: DatabaseType;
  connectionId?: string;
  database?: string;
  context?: "results" | "table-data";
  tableMeta?: {
    schema?: string;
    tableName: string;
    columns: ColumnInfo[];
    primaryKeys: string[];
  };
  loading?: boolean;
  onExecuteSql?: (sql: string) => Promise<void>;
}>();

const emit = defineEmits<{
  reload: [sql?: string, searchText?: string, whereInput?: string, orderBy?: string];
  paginate: [offset: number, limit: number, whereInput?: string, orderBy?: string];
  sort: [column: string, columnIndex: number, direction: "asc" | "desc" | null, whereInput?: string];
}>();

const hasData = computed(() => props.result.columns.length > 0);

const columnTypeMap = computed(() => {
  const map = new Map<string, string>();
  if (props.tableMeta?.columns) {
    for (const col of props.tableMeta.columns) {
      const typeName = shortTypeName(col.data_type);
      // Add precision for numeric/decimal types
      if (col.numeric_precision != null && ["numeric", "decimal"].includes(col.data_type.toLowerCase())) {
        const scale = col.numeric_scale ?? 0;
        map.set(col.name, `${typeName}(${col.numeric_precision},${scale})`);
      } else {
        map.set(col.name, typeName);
      }
    }
  }
  return map;
});

const columnCommentMap = computed(() => {
  const map = new Map<string, string>();
  if (props.tableMeta?.columns) {
    for (const col of props.tableMeta.columns) {
      if (col.comment) map.set(col.name, col.comment);
    }
  }
  return map;
});

function shortTypeName(t: string): string {
  const s = t.toLowerCase();
  if (s === "character varying") return "varchar";
  if (s === "character") return "char";
  if (s === "double precision") return "double";
  if (s === "timestamp without time zone") return "timestamp";
  if (s === "timestamp with time zone") return "timestamptz";
  if (s === "time without time zone") return "time";
  if (s === "time with time zone") return "timetz";
  if (s === "boolean") return "bool";
  if (s === "integer") return "int";
  if (s === "smallint") return "int2";
  if (s === "bigint") return "int8";
  if (s === "real") return "float4";
  return t;
}

function typeColorClass(t: string): string {
  // Strip precision/scale suffix like (20,6)
  const base = t.replace(/\(.*\)$/, "").toLowerCase();
  if (
    [
      "int",
      "int2",
      "int4",
      "int8",
      "smallint",
      "bigint",
      "integer",
      "serial",
      "bigserial",
      "tinyint",
      "mediumint",
    ].includes(base)
  )
    return "text-blue-500";
  if (["float4", "float8", "double", "decimal", "numeric", "real", "float", "money"].includes(base))
    return "text-cyan-500";
  if (
    [
      "varchar",
      "text",
      "char",
      "character varying",
      "character",
      "string",
      "nvarchar",
      "nchar",
      "ntext",
      "longtext",
      "mediumtext",
      "tinytext",
      "clob",
    ].includes(base)
  )
    return "text-green-500";
  if (["bool", "boolean", "bit"].includes(base)) return "text-orange-500";
  if (["timestamp", "timestamptz", "datetime", "date", "time", "timetz", "datetime2", "smalldatetime"].includes(base))
    return "text-purple-500";
  if (["json", "jsonb", "xml", "array"].includes(base)) return "text-pink-500";
  if (["uuid", "uniqueidentifier"].includes(base)) return "text-amber-500";
  if (["bytea", "blob", "binary", "varbinary", "image"].includes(base)) return "text-red-400";
  return "text-muted-foreground";
}
const contextCell = ref<{ rowId: number; rowIndex: number; col: number } | null>(null);
const selectionAnchor = ref<CellPosition | null>(null);
const selectionFocus = ref<CellPosition | null>(null);
const isSelectingCells = ref(false);
const detailCell = ref<{ rowIndex: number; col: number } | null>(null);
const showCellDetail = ref(false);
const transposeRowIndex = ref<number | null>(null);
const showTranspose = ref(false);
const sortCol = ref<string | null>(null);
const sortColIndex = ref<number | null>(null);
const sortDir = ref<"asc" | "desc">("asc");
const searchText = ref("");
const searchSuggestions = ref<string[]>([]);
const suggestionIndex = ref(-1);
const searchInputRef = ref<HTMLInputElement>();
const measureRef = ref<HTMLSpanElement>();
const suggestionLeft = ref(0);

const whereSuggestions = ref<string[]>([]);
const whereSuggestionIndex = ref(-1);
const whereFilterInputRef = ref<HTMLInputElement>();
const whereMeasureRef = ref<HTMLSpanElement>();
const whereSuggestionLeft = ref(0);

const orderBySuggestions = ref<string[]>([]);
const orderBySuggestionIndex = ref(-1);
const orderByInputRef = ref<HTMLInputElement>();
const orderByMeasureRef = ref<HTMLSpanElement>();
const orderBySuggestionLeft = ref(0);

const orderByInput = ref("");
const hasOrderByInput = computed(() => orderByInput.value.trim().length > 0);
const whereFilterInput = ref("");
const hasWhereFilterInput = computed(() => whereFilterInput.value.trim().length > 0);

function updateSuggestionPosition() {
  nextTick(() => {
    const input = searchInputRef.value;
    const measure = measureRef.value;
    if (!input || !measure) return;
    const cursorPos = input.selectionStart ?? 0;
    measure.textContent = searchText.value.slice(0, cursorPos);
    suggestionLeft.value = measure.getBoundingClientRect().width;
  });
}

watch(searchText, (val) => {
  searchSuggestions.value = [];
  if (!props.tableMeta?.columns?.length) return;

  const trimmed = val.trim();
  if (trimmed.length === 0) return;

  const lastToken = trimmed.split(/[\s,()><=!&|]+/).pop() || "";
  if (lastToken.length > 0) {
    const tl = lastToken.toLowerCase();
    searchSuggestions.value = props.tableMeta.columns
      .map((c) => c.name)
      .filter((n) => n.toLowerCase().startsWith(tl) && n.toLowerCase() !== tl)
      .slice(0, 8);
    suggestionIndex.value = 0;
    updateSuggestionPosition();
  }
});

function acceptSuggestion() {
  const idx = suggestionIndex.value;
  if (idx < 0 || idx >= searchSuggestions.value.length) return;
  const sug = searchSuggestions.value[idx];

  const lastWordMatch = searchText.value.match(/([^\s,()><=!&|]+)$/);
  if (lastWordMatch) {
    const lastWord = lastWordMatch[1];
    const prefix = searchText.value.slice(0, -lastWord.length);
    searchText.value = prefix + sug;
  }
  searchSuggestions.value = [];
  suggestionIndex.value = -1;
  searchInputRef.value?.focus();
}

function dismissSuggestions() {
  searchSuggestions.value = [];
  suggestionIndex.value = -1;
}

function navigateSuggestion(delta: number) {
  if (searchSuggestions.value.length === 0) return;
  suggestionIndex.value = Math.min(Math.max(suggestionIndex.value + delta, 0), searchSuggestions.value.length - 1);
}

const PAIRS: Record<string, string> = { "'": "'", '"': '"', "(": ")" };

function onSearchKeydown(e: KeyboardEvent) {
  if (e.key in PAIRS && !e.ctrlKey && !e.metaKey) {
    const input = e.target as HTMLInputElement;
    const start = input.selectionStart ?? 0;
    const end = input.selectionEnd ?? 0;
    const close = PAIRS[e.key];

    if (start !== end) {
      // Wrap selection: 'text' → 'text'
      e.preventDefault();
      const selected = searchText.value.slice(start, end);
      searchText.value = searchText.value.slice(0, start) + e.key + selected + close + searchText.value.slice(end);
      nextTick(() => {
        input.setSelectionRange(start + 1 + selected.length, start + 1 + selected.length);
      });
      suggestionIndex.value = -1;
      return;
    }

    if (e.key === close && searchText.value[start] === close) {
      // Cursor before matching close char → skip over it (only for quotes)
      e.preventDefault();
      input.setSelectionRange(start + 1, start + 1);
      return;
    }

    e.preventDefault();
    searchText.value = searchText.value.slice(0, start) + e.key + close + searchText.value.slice(end);
    nextTick(() => {
      input.setSelectionRange(start + 1, start + 1);
    });
    suggestionIndex.value = -1;
    return;
  }

  if (searchSuggestions.value.length > 0) {
    if (e.key === "Tab") {
      e.preventDefault();
      acceptSuggestion();
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      dismissSuggestions();
      return;
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      navigateSuggestion(1);
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      navigateSuggestion(-1);
      return;
    }
  }
  if (e.key === "Escape") {
    searchText.value = "";
  }
}

// --- WHERE filter input suggestions ---
function updateWhereSuggestionPosition() {
  nextTick(() => {
    const input = whereFilterInputRef.value;
    const measure = whereMeasureRef.value;
    if (!input || !measure) return;
    const cursorPos = input.selectionStart ?? 0;
    measure.textContent = whereFilterInput.value.slice(0, cursorPos);
    whereSuggestionLeft.value = measure.getBoundingClientRect().width;
  });
}

function acceptWhereSuggestion() {
  const idx = whereSuggestionIndex.value;
  if (idx < 0 || idx >= whereSuggestions.value.length) return;
  const sug = whereSuggestions.value[idx];
  const lastWordMatch = whereFilterInput.value.match(/([^\s,()><=!&|]+)$/);
  if (lastWordMatch) {
    const lastWord = lastWordMatch[1];
    const prefix = whereFilterInput.value.slice(0, -lastWord.length);
    whereFilterInput.value = prefix + sug;
  }
  whereSuggestions.value = [];
  whereSuggestionIndex.value = -1;
  whereFilterInputRef.value?.focus();
}

function dismissWhereSuggestions() {
  whereSuggestions.value = [];
  whereSuggestionIndex.value = -1;
}

function navigateWhereSuggestion(delta: number) {
  if (whereSuggestions.value.length === 0) return;
  whereSuggestionIndex.value = Math.min(
    Math.max(whereSuggestionIndex.value + delta, 0),
    whereSuggestions.value.length - 1,
  );
}

watch(whereFilterInput, (val) => {
  whereSuggestions.value = [];
  if (!props.tableMeta?.columns?.length) return;
  const trimmed = val.trim();
  if (trimmed.length === 0) return;
  const lastToken = trimmed.split(/[\s,()><=!&|]+/).pop() || "";
  if (lastToken.length > 0) {
    const tl = lastToken.toLowerCase();
    whereSuggestions.value = props.tableMeta.columns
      .map((c) => c.name)
      .filter((n) => n.toLowerCase().startsWith(tl) && n.toLowerCase() !== tl)
      .slice(0, 8);
    whereSuggestionIndex.value = 0;
    updateWhereSuggestionPosition();
  }
});

function onWhereFilterKeydown(e: KeyboardEvent) {
  if (e.key in PAIRS && !e.ctrlKey && !e.metaKey) {
    const input = e.target as HTMLInputElement;
    const start = input.selectionStart ?? 0;
    const end = input.selectionEnd ?? 0;
    const close = PAIRS[e.key];
    if (start !== end) {
      e.preventDefault();
      const selected = whereFilterInput.value.slice(start, end);
      whereFilterInput.value =
        whereFilterInput.value.slice(0, start) + e.key + selected + close + whereFilterInput.value.slice(end);
      nextTick(() => {
        input.setSelectionRange(start + 1 + selected.length, start + 1 + selected.length);
      });
      whereSuggestionIndex.value = -1;
      return;
    }
    if (e.key === close && whereFilterInput.value[start] === close) {
      e.preventDefault();
      input.setSelectionRange(start + 1, start + 1);
      return;
    }
    e.preventDefault();
    whereFilterInput.value = whereFilterInput.value.slice(0, start) + e.key + close + whereFilterInput.value.slice(end);
    nextTick(() => {
      input.setSelectionRange(start + 1, start + 1);
    });
    whereSuggestionIndex.value = -1;
    return;
  }
  if (whereSuggestions.value.length > 0) {
    if (e.key === "Tab") {
      e.preventDefault();
      acceptWhereSuggestion();
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      dismissWhereSuggestions();
      return;
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      navigateWhereSuggestion(1);
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      navigateWhereSuggestion(-1);
      return;
    }
  }
  if (e.key === "Enter") {
    e.preventDefault();
    applyWhereFilter();
  }
}

// --- ORDER BY input suggestions ---
function updateOrderBySuggestionPosition() {
  nextTick(() => {
    const input = orderByInputRef.value;
    const measure = orderByMeasureRef.value;
    if (!input || !measure) return;
    const cursorPos = input.selectionStart ?? 0;
    measure.textContent = orderByInput.value.slice(0, cursorPos);
    orderBySuggestionLeft.value = measure.getBoundingClientRect().width;
  });
}

function acceptOrderBySuggestion() {
  const idx = orderBySuggestionIndex.value;
  if (idx < 0 || idx >= orderBySuggestions.value.length) return;
  const sug = orderBySuggestions.value[idx];
  const lastWordMatch = orderByInput.value.match(/([^\s,()]+)$/);
  if (lastWordMatch) {
    const lastWord = lastWordMatch[1];
    const prefix = orderByInput.value.slice(0, -lastWord.length);
    orderByInput.value = prefix + sug;
  }
  orderBySuggestions.value = [];
  orderBySuggestionIndex.value = -1;
  orderByInputRef.value?.focus();
}

function dismissOrderBySuggestions() {
  orderBySuggestions.value = [];
  orderBySuggestionIndex.value = -1;
}

function navigateOrderBySuggestion(delta: number) {
  if (orderBySuggestions.value.length === 0) return;
  orderBySuggestionIndex.value = Math.min(
    Math.max(orderBySuggestionIndex.value + delta, 0),
    orderBySuggestions.value.length - 1,
  );
}

watch(orderByInput, (val) => {
  orderBySuggestions.value = [];
  if (!props.tableMeta?.columns?.length) return;
  const trimmed = val.trim();
  if (trimmed.length === 0) return;
  const lastToken = trimmed.split(/[\s,()]+/).pop() || "";
  if (lastToken.length > 0 && !["asc", "desc"].includes(lastToken.toLowerCase())) {
    const tl = lastToken.toLowerCase();
    orderBySuggestions.value = props.tableMeta.columns
      .map((c) => c.name)
      .filter((n) => n.toLowerCase().startsWith(tl) && n.toLowerCase() !== tl)
      .slice(0, 8);
    orderBySuggestionIndex.value = 0;
    updateOrderBySuggestionPosition();
  }
});

function onOrderByKeydown(e: KeyboardEvent) {
  if (orderBySuggestions.value.length > 0) {
    if (e.key === "Tab") {
      e.preventDefault();
      acceptOrderBySuggestion();
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      dismissOrderBySuggestions();
      return;
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      navigateOrderBySuggestion(1);
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      navigateOrderBySuggestion(-1);
      return;
    }
  }
  if (e.key === "Enter") {
    e.preventDefault();
    applyOrderBySearch();
  }
}

const saveError = ref("");
const isApplyingWhere = ref(false);
const rowStatusFilter = ref<RowStatusFilter>("all");
const columnWidths = ref<number[]>([]);
const gridRef = ref<HTMLDivElement>();
const headerRef = ref<HTMLDivElement>();
const { width: gridWidth } = useElementSize(gridRef);

const COL_MIN_WIDTH = 60;
const COL_MAX_WIDTH = 400;
const COL_CHAR_WIDTH = 8;
const COL_HEADER_PADDING = 48;
const COL_CELL_PADDING = 28;
const COL_SAMPLE_ROWS = 50;

function estimateTextWidth(text: string, padding: number): number {
  return text.length * COL_CHAR_WIDTH + padding;
}

function initColumnWidths() {
  if (columnWidths.value.length !== props.result.columns.length) {
    const rows = props.result.rows;
    const sampleCount = Math.min(rows.length, COL_SAMPLE_ROWS);
    columnWidths.value = props.result.columns.map((colName, colIdx) => {
      let maxWidth = estimateTextWidth(colName, COL_HEADER_PADDING);
      for (let i = 0; i < sampleCount; i++) {
        const val = rows[i]?.[colIdx];
        if (val == null) continue;
        const text = typeof val === "object" ? JSON.stringify(val) : String(val);
        const displayLen = Math.min(text.length, 60);
        const w = displayLen * COL_CHAR_WIDTH + COL_CELL_PADDING;
        if (w > maxWidth) maxWidth = w;
      }
      return Math.max(COL_MIN_WIDTH, Math.min(COL_MAX_WIDTH, Math.round(maxWidth)));
    });
  }
}

function syncHeaderScroll(e: Event) {
  if (headerRef.value) {
    headerRef.value.scrollLeft = (e.target as HTMLElement).scrollLeft;
  }
}

let isResizing = false;

function onResizeStart(colIdx: number, event: MouseEvent) {
  event.preventDefault();
  isResizing = true;
  const startX = event.clientX;
  const startWidth = columnWidths.value[colIdx];
  const onMove = (e: MouseEvent) => {
    columnWidths.value[colIdx] = Math.max(60, startWidth + e.clientX - startX);
  };
  const onUp = () => {
    document.removeEventListener("mousemove", onMove);
    document.removeEventListener("mouseup", onUp);
    requestAnimationFrame(() => {
      isResizing = false;
    });
  };
  document.addEventListener("mousemove", onMove);
  document.addEventListener("mouseup", onUp);
}

function autoFitColumn(colIdx: number) {
  const colName = props.result.columns[colIdx];
  if (!colName) return;
  const rows = props.result.rows;
  const sampleCount = Math.min(rows.length, COL_SAMPLE_ROWS);
  let maxWidth = estimateTextWidth(colName, COL_HEADER_PADDING);
  for (let i = 0; i < sampleCount; i++) {
    const val = rows[i]?.[colIdx];
    if (val == null) continue;
    const text = typeof val === "object" ? JSON.stringify(val) : String(val);
    const displayLen = Math.min(text.length, 60);
    const w = displayLen * COL_CHAR_WIDTH + COL_CELL_PADDING;
    if (w > maxWidth) maxWidth = w;
  }
  columnWidths.value[colIdx] = Math.max(COL_MIN_WIDTH, Math.min(COL_MAX_WIDTH, Math.round(maxWidth)));
}

const ROW_NUM_WIDTH = 48;
const baseTotalWidth = computed(() => columnWidths.value.reduce((a, b) => a + b, 0));
const renderedColumnWidths = computed(() => {
  const widths = columnWidths.value;
  if (widths.length === 0) return widths;

  const extraWidth = Math.max(0, gridWidth.value - ROW_NUM_WIDTH - baseTotalWidth.value);
  if (extraWidth === 0) return widths;

  const extraPerColumn = extraWidth / widths.length;
  return widths.map((width) => width + extraPerColumn);
});
const totalWidth = computed(() => renderedColumnWidths.value.reduce((a, b) => a + b, 0) + ROW_NUM_WIDTH);

const columnVars = computed(() => {
  const vars: Record<string, string> = {};
  renderedColumnWidths.value.forEach((w, i) => {
    vars[`--col-w-${i}`] = `${w}px`;
  });
  vars["--row-num-w"] = `${ROW_NUM_WIDTH}px`;
  vars["--total-w"] = `${totalWidth.value}px`;
  return vars;
});

initColumnWidths();
watch(() => props.result.columns.length, initColumnWidths);

// --- Pagination ---
const pageSize = ref(100);
const currentPage = ref(1);
const isFullPage = computed(() => props.result.rows.length >= pageSize.value);
const isResultsContext = computed(() => props.context === "results");
const canUseWhereSearch = computed(() => !!props.tableMeta && !!props.onExecuteSql && !isResultsContext.value);
const clientSearchText = computed(() => (searchText.value.trim() ? searchText.value : ""));

function currentWhereInput(): string | undefined {
  return whereFilterInput.value.trim() || undefined;
}

function currentOrderBy(): string | undefined {
  return (
    orderByInput.value.trim() ||
    (sortCol.value ? `${quoteIdent(sortCol.value)} ${sortDir.value.toUpperCase()}` : undefined)
  );
}

function prevPage() {
  if (currentPage.value <= 1) return;
  currentPage.value--;
  resetGridVerticalScroll(true);
  emit("paginate", (currentPage.value - 1) * pageSize.value, pageSize.value, currentWhereInput(), currentOrderBy());
}
function nextPage() {
  if (!isFullPage.value) return;
  currentPage.value++;
  resetGridVerticalScroll(true);
  emit("paginate", (currentPage.value - 1) * pageSize.value, pageSize.value, currentWhereInput(), currentOrderBy());
}
function changePageSize(size: number) {
  pageSize.value = size;
  currentPage.value = 1;
  resetGridVerticalScroll(true);
  emit("paginate", 0, size, currentWhereInput(), currentOrderBy());
}

// --- Editing ---
type CellValue = string | number | boolean | null;
const editingCell = ref<{ rowId: number; col: number } | null>(null);
const editValue = ref("");
type GridScrollerRef =
  | HTMLElement
  | {
      $el?: HTMLElement;
      el?: HTMLElement | { value?: HTMLElement };
      scrollToItem?: (index: number) => void;
      scrollToPosition?: (position: number) => void;
    };

const scrollerRef = ref<GridScrollerRef | null>(null);
const dirtyRows = ref<Map<number, Map<number, CellValue>>>(new Map());
const newRows = ref<CellValue[][]>([]);
const deletedRows = ref<Set<number>>(new Set());

const dirtyRowCount = computed(() => dirtyRows.value.size);
const newRowCount = computed(() => newRows.value.length);
const deletedRowCount = computed(() => deletedRows.value.size);
const pendingChangeCount = computed(() => dirtyRowCount.value + newRowCount.value + deletedRowCount.value);
const hasPendingChanges = computed(() => pendingChangeCount.value > 0);

// --- Transaction state ---
const transactionActive = ref(false);
const isSaving = ref(false);

function enterTransaction() {
  transactionActive.value = true;
}

function exitTransaction() {
  transactionActive.value = false;
}

const feishuBitableRecordIdColumn = computed(() =>
  props.databaseType === "feishu_bitable"
    ? externalRecordIdColumn(props.tableMeta?.primaryKeys, props.result.columns)
    : undefined,
);
const isFeishuBitableTableEditor = computed(() =>
  isFeishuBitableTableEditable({
    databaseType: props.databaseType,
    context: props.context,
    connectionId: props.connectionId,
    tableMeta: props.tableMeta,
    resultColumns: props.result.columns,
  }),
);
const useTransaction = computed(
  () =>
    props.editable &&
    !!props.connectionId &&
    !!props.database &&
    !!props.tableMeta &&
    !isFeishuBitableTableEditor.value,
);
const canUseRowChangeControls = computed(
  () => props.editable && !!props.tableMeta && (useTransaction.value || isFeishuBitableTableEditor.value),
);

function feishuBitableRecordIdColumnIndex(): number {
  const recordIdColumn = feishuBitableRecordIdColumn.value;
  return recordIdColumn ? props.result.columns.indexOf(recordIdColumn) : -1;
}

function isGridColumnEditable(columnIndex: number): boolean {
  if (!props.editable) return false;
  return !(isFeishuBitableTableEditor.value && props.result.columns[columnIndex] === feishuBitableRecordIdColumn.value);
}

async function onToolbarRefresh() {
  if (transactionActive.value) {
    discardChanges();
  }
  emit(
    "reload",
    props.sql,
    searchText.value,
    whereFilterInput.value.trim() || undefined,
    orderByInput.value.trim() || undefined,
  );
}

async function onToolbarCommit() {
  await saveChanges();
}

function onToolbarRollback() {
  discardChanges();
  emit(
    "reload",
    props.sql,
    searchText.value,
    whereFilterInput.value.trim() || undefined,
    orderByInput.value.trim() || undefined,
  );
}

const sortedRows = computed(() => {
  let rows = props.result.rows.map((row, sourceIndex) => ({ row, sourceIndex }));
  if (clientSearchText.value) {
    const q = clientSearchText.value.toLowerCase();
    rows = rows.filter(({ row, sourceIndex }) => {
      const data = rowDataWithChanges(row, sourceIndex);
      return data.some((cell) => cell !== null && String(cell).toLowerCase().includes(q));
    });
  }
  return rows;
});

function rowDataWithChanges(row: CellValue[], sourceIndex: number): CellValue[] {
  const dirty = dirtyRows.value.get(sourceIndex);
  return row.map((v, colIdx) => (dirty?.has(colIdx) ? dirty.get(colIdx)! : v));
}

interface RowItem {
  id: number;
  sourceIndex?: number;
  newIndex?: number;
  data: CellValue[];
  isNew: boolean;
  isDeleted: boolean;
  isDirtyCol: boolean[];
  status: RowStatus;
}

const displayItems = computed<RowItem[]>(() => {
  const cols = props.result.columns;
  const items: RowItem[] = sortedRows.value.map(({ row, sourceIndex }) => {
    const dirty = dirtyRows.value.get(sourceIndex);
    const data = rowDataWithChanges(row, sourceIndex);
    const isDirtyCol = row.map((_, colIdx) => dirty?.has(colIdx) ?? false);
    const isDeleted = deletedRows.value.has(sourceIndex);
    const status: RowStatus = isDeleted ? "deleted" : dirty ? "edited" : "clean";
    return { id: sourceIndex, sourceIndex, data, isNew: false, isDeleted, isDirtyCol, status };
  });
  newRows.value.forEach((row, i) => {
    items.push({
      id: -(i + 1),
      newIndex: i,
      data: row,
      isNew: true,
      isDeleted: false,
      isDirtyCol: cols.map(() => false),
      status: "new",
    });
  });
  return items.filter((item) => matchesRowStatusFilter(item.status, rowStatusFilter.value));
});
const hasVisibleRows = computed(() => displayItems.value.length > 0);
const hasActiveFilter = computed(() => !!clientSearchText.value || rowStatusFilter.value !== "all");
const totalFilterableRowCount = computed(() => props.result.rows.length + newRows.value.length);
const emptyTitle = computed(() => (hasActiveFilter.value ? t("grid.noFilteredRows") : t("grid.noRows")));
const emptyDescription = computed(() =>
  hasActiveFilter.value ? t("grid.noFilteredRowsDescription") : t("grid.noRowsDescription"),
);
const isErrorResult = computed(
  () => props.result.columns.length === 1 && props.result.columns[0] === "Error" && props.result.rows.length > 0,
);
const errorMessage = computed(() => (isErrorResult.value ? String(props.result.rows[0]?.[0] ?? "") : ""));
const selectedRange = computed<CellSelectionRange | null>(() => {
  if (!selectionAnchor.value || !selectionFocus.value) return null;
  return normalizeSelectionRange(selectionAnchor.value, selectionFocus.value);
});
const visibleSelectionRows = computed(() => displayItems.value.map((item) => item.data));
const selectedCells = computed(() =>
  extractSelection(props.result.columns, visibleSelectionRows.value, selectedRange.value),
);
const selectedCellCount = computed(() => selectedCells.value.columns.length * selectedCells.value.rows.length);
const hasCellSelection = computed(() => selectedCellCount.value > 0);
const selectionSummary = computed(() => t("grid.selectedCells", { count: selectedCellCount.value }));
const contextRowItem = computed(() => (contextCell.value ? getRowItem(contextCell.value.rowId) : undefined));
const contextColumn = computed(() => {
  if (!contextCell.value || contextCell.value.col < 0) return null;
  return props.result.columns[contextCell.value.col] ?? null;
});
const contextCellValue = computed<CellValue | null>(() => {
  if (!contextCell.value || contextCell.value.col < 0) return null;
  return contextRowItem.value?.data[contextCell.value.col] ?? null;
});
const activeCellDetail = computed(() => {
  const cell = detailCell.value;
  if (!cell) return null;
  const item = displayItems.value[cell.rowIndex];
  const column = props.result.columns[cell.col];
  if (!item || !column) return null;
  const value = item.data[cell.col] ?? null;
  const rawValue = formatCell(value);
  const valueText = value === null ? "" : typeof value === "object" ? JSON.stringify(value) : String(value);
  const trimmed = valueText.trim();
  const maybeJson = typeof value === "string" && (trimmed.startsWith("{") || trimmed.startsWith("["));
  let formattedJson = "";
  if (maybeJson) {
    try {
      formattedJson = JSON.stringify(JSON.parse(value), null, 2);
    } catch {
      formattedJson = "";
    }
  }
  return {
    rowNumber: cell.rowIndex + 1,
    rowId: item.id,
    colIndex: cell.col,
    column,
    type: columnTypeMap.value.get(column) || "",
    comment: columnCommentMap.value.get(column) || "",
    value,
    rawValue,
    length: value === null ? 0 : String(value).length,
    formattedJson,
    isEditable: props.editable && !item.isDeleted,
  };
});

const detailEditValue = ref("");
const isEditingDetail = ref(false);

function startDetailEdit() {
  const detail = activeCellDetail.value;
  if (!detail || !detail.isEditable) return;
  detailEditValue.value =
    detail.value === null ? "" : typeof detail.value === "object" ? JSON.stringify(detail.value) : String(detail.value);
  isEditingDetail.value = true;
}

function commitDetailEdit() {
  const detail = activeCellDetail.value;
  if (!detail || !isEditingDetail.value) return;
  isEditingDetail.value = false;

  const item = getRowItem(detail.rowId);
  if (!item || item.isDeleted) return;

  if (item.isNew && item.newIndex !== undefined) {
    const oldVal = newRows.value[item.newIndex]?.[detail.colIndex];
    newRows.value[item.newIndex][detail.colIndex] = coerceCellValue(detailEditValue.value, oldVal);
    return;
  }

  if (item.sourceIndex === undefined) return;

  const oldVal = props.result.rows[item.sourceIndex]?.[detail.colIndex];
  const newVal = coerceCellValue(detailEditValue.value, oldVal);
  if (newVal !== oldVal) {
    if (!dirtyRows.value.has(item.sourceIndex)) dirtyRows.value.set(item.sourceIndex, new Map());
    dirtyRows.value.get(item.sourceIndex)!.set(detail.colIndex, newVal);
    if (useTransaction.value && !transactionActive.value) {
      enterTransaction();
    }
  } else {
    const rowChanges = dirtyRows.value.get(item.sourceIndex);
    rowChanges?.delete(detail.colIndex);
    if (rowChanges?.size === 0) dirtyRows.value.delete(item.sourceIndex);
  }
  dirtyRows.value = new Map(dirtyRows.value);
}

function cancelDetailEdit() {
  isEditingDetail.value = false;
}

function setDetailNull() {
  const detail = activeCellDetail.value;
  if (!detail || !detail.isEditable) return;

  const item = getRowItem(detail.rowId);
  if (!item || item.isDeleted) return;

  if (item.isNew && item.newIndex !== undefined) {
    newRows.value[item.newIndex][detail.colIndex] = null;
    newRows.value = [...newRows.value];
    isEditingDetail.value = false;
    detailCell.value = { ...detailCell.value! };
    return;
  }

  if (item.sourceIndex === undefined) return;
  if (!dirtyRows.value.has(item.sourceIndex)) dirtyRows.value.set(item.sourceIndex, new Map());
  dirtyRows.value.get(item.sourceIndex)!.set(detail.colIndex, null);
  dirtyRows.value = new Map(dirtyRows.value);
  if (useTransaction.value && !transactionActive.value) {
    enterTransaction();
  }
  isEditingDetail.value = false;
  detailCell.value = { ...detailCell.value! };
}

function toggleSort(colName: string, colIdx: number) {
  if (isResizing) return;
  if (sortCol.value === colName && sortColIndex.value === colIdx) {
    if (sortDir.value === "asc") {
      sortDir.value = "desc";
      emit("sort", colName, colIdx, "desc", currentWhereInput());
    } else {
      sortCol.value = null;
      sortColIndex.value = null;
      sortDir.value = "asc";
      emit("sort", colName, colIdx, null, currentWhereInput());
    }
  } else {
    sortCol.value = colName;
    sortColIndex.value = colIdx;
    sortDir.value = "asc";
    emit("sort", colName, colIdx, "asc", currentWhereInput());
  }
}

function applyContextSort(direction: "asc" | "desc" | null) {
  if (!contextColumn.value || !contextCell.value) return;
  const column = contextColumn.value;
  const columnIndex = contextCell.value.col;
  orderByInput.value = "";
  currentPage.value = 1;
  if (direction) {
    sortCol.value = column;
    sortColIndex.value = columnIndex;
    sortDir.value = direction;
  } else {
    sortCol.value = null;
    sortColIndex.value = null;
    sortDir.value = "asc";
  }
  emit("sort", column, columnIndex, direction, currentWhereInput());
}

function contextFilterCondition(mode: "equals" | "not-equals" | "is-null" | "is-not-null"): string | null {
  if (!contextColumn.value) return null;
  const column = quoteIdent(contextColumn.value);
  const value = contextCellValue.value;

  if (mode === "is-null") return `${column} IS NULL`;
  if (mode === "is-not-null") return `${column} IS NOT NULL`;
  if (value === null) return mode === "equals" ? `${column} IS NULL` : `${column} IS NOT NULL`;
  return mode === "equals" ? `${column} = ${escapeVal(value)}` : `${column} <> ${escapeVal(value)}`;
}

async function applyContextFilter(mode: "equals" | "not-equals" | "is-null" | "is-not-null") {
  if (!canUseWhereSearch.value) return;
  const condition = contextFilterCondition(mode);
  if (!condition) return;
  const existing = whereFilterInput.value.trim();
  whereFilterInput.value = existing ? `(${existing}) AND (${condition})` : condition;
  await applyWhereFilter();
}

async function clearContextFilter() {
  if (!canUseWhereSearch.value) return;
  whereFilterInput.value = "";
  await applyWhereFilter();
}

async function applyOrderBySearch() {
  if (!props.tableMeta || !props.onExecuteSql) return;
  const orderByClause = orderByInput.value.trim() || undefined;
  isApplyingWhere.value = true;
  saveError.value = "";
  currentPage.value = 1;
  sortCol.value = null;
  sortColIndex.value = null;
  sortDir.value = "asc";
  try {
    const sql = buildTableSelectSql({
      databaseType: props.databaseType,
      schema: props.tableMeta.schema,
      tableName: props.tableMeta.tableName,
      primaryKeys: props.tableMeta.primaryKeys,
      orderBy: orderByClause,
      limit: pageSize.value,
      whereInput: whereFilterInput.value.trim() || undefined,
    });
    await props.onExecuteSql(sql);
  } catch (e: any) {
    saveError.value = String(e?.message || e);
  } finally {
    isApplyingWhere.value = false;
  }
}

async function applyWhereFilter() {
  if (!props.tableMeta || !props.onExecuteSql) return;
  isApplyingWhere.value = true;
  saveError.value = "";
  currentPage.value = 1;
  try {
    const sql = buildTableSelectSql({
      databaseType: props.databaseType,
      schema: props.tableMeta.schema,
      tableName: props.tableMeta.tableName,
      primaryKeys: props.tableMeta.primaryKeys,
      orderBy:
        orderByInput.value.trim() ||
        (sortCol.value ? `${quoteIdent(sortCol.value)} ${sortDir.value.toUpperCase()}` : undefined),
      limit: pageSize.value,
      whereInput: whereFilterInput.value.trim() || undefined,
    });
    await props.onExecuteSql(sql);
  } catch (e: any) {
    saveError.value = String(e?.message || e);
  } finally {
    isApplyingWhere.value = false;
  }
}

const CELL_DISPLAY_MAX_LENGTH = 256;

function formatCell(value: CellValue): string {
  if (value === null) return "NULL";
  if (typeof value === "boolean") return value ? "true" : "false";
  const s = typeof value === "object" ? JSON.stringify(value) : String(value);
  return s.length > CELL_DISPLAY_MAX_LENGTH ? s.slice(0, CELL_DISPLAY_MAX_LENGTH) : s;
}

function quoteIdent(name: string): string {
  return quoteTableIdentifier(props.databaseType, name);
}

function escapeVal(value: CellValue): string {
  return formatGridSqlLiteral(value);
}

function isNull(value: unknown): boolean {
  return value === null;
}

function rowNumberStatusClass(item: RowItem): string {
  if (item.status === "new") {
    return "border-emerald-500/40 bg-emerald-500/15 font-semibold text-emerald-700 dark:text-emerald-300";
  }
  if (item.status === "edited") {
    return "border-amber-500/40 bg-amber-500/15 font-semibold text-amber-700 dark:text-amber-300";
  }
  if (item.status === "deleted") {
    return "border-destructive/40 bg-destructive/15 font-semibold text-destructive line-through";
  }
  return "text-muted-foreground";
}

function setRowStatusFilter(value: string) {
  rowStatusFilter.value = value as RowStatusFilter;
}

// --- Inline editor ---
let isCancelling = false;
let cancelScrollRestoreFrame = 0;
let resetScrollFrame = 0;
let resetScrollAfterResult = false;

function getScrollerElement(): HTMLElement | null {
  const scroller = scrollerRef.value;
  if (!scroller) return null;
  if (scroller instanceof HTMLElement) return scroller;
  if (scroller.$el instanceof HTMLElement) return scroller.$el;
  if (scroller.el instanceof HTMLElement) return scroller.el;
  if (scroller.el?.value instanceof HTMLElement) return scroller.el.value;
  return null;
}

function scrollGridToTop() {
  const scroller = scrollerRef.value;
  if (scroller && !(scroller instanceof HTMLElement)) {
    scroller.scrollToItem?.(0);
    scroller.scrollToPosition?.(0);
  }

  const el = getScrollerElement();
  if (el) el.scrollTop = 0;
}

function resetGridVerticalScroll(afterResult = false) {
  if (afterResult) resetScrollAfterResult = true;
  if (resetScrollFrame) cancelAnimationFrame(resetScrollFrame);
  scrollGridToTop();
  nextTick(() => {
    scrollGridToTop();
    resetScrollFrame = requestAnimationFrame(() => {
      scrollGridToTop();
      resetScrollFrame = 0;
    });
  });
}

function preserveScrollPosition() {
  const el = getScrollerElement();
  if (!el) return () => {};
  const top = el.scrollTop;
  const left = el.scrollLeft;
  return () => {
    el.scrollTop = top;
    el.scrollLeft = left;
  };
}

function focusScrollerWithoutScrolling() {
  const el = getScrollerElement();
  if (!el) return;
  if (!el.hasAttribute("tabindex")) el.setAttribute("tabindex", "-1");
  el.focus({ preventScroll: true });
}

function restoreScrollAcrossFrames(restoreScroll: () => void) {
  if (cancelScrollRestoreFrame) cancelAnimationFrame(cancelScrollRestoreFrame);
  restoreScroll();
  nextTick(() => {
    restoreScroll();
    cancelScrollRestoreFrame = requestAnimationFrame(() => {
      restoreScroll();
      cancelScrollRestoreFrame = requestAnimationFrame(() => {
        restoreScroll();
        cancelScrollRestoreFrame = 0;
        isCancelling = false;
      });
    });
  });
}

function getRowItem(rowId: number): RowItem | undefined {
  return displayItems.value.find((item) => item.id === rowId);
}

function coerceCellValue(value: string, oldVal: CellValue | undefined): CellValue {
  if (value.toUpperCase() === "NULL") return null;
  if (value === "" && isNull(oldVal)) return null;
  if (typeof oldVal === "number") {
    const num = Number(value);
    if (!Number.isNaN(num)) return num;
  }
  if (typeof oldVal === "boolean") {
    return value === "true" || value === "1";
  }
  return value;
}

function startEdit(rowId: number, colIdx: number) {
  if (!isGridColumnEditable(colIdx)) return;
  const item = getRowItem(rowId);
  if (!item || item.isDeleted) return;
  isCancelling = false;
  editingCell.value = { rowId, col: colIdx };
  const val = item?.data[colIdx] ?? null;
  editValue.value = val === null ? "" : typeof val === "object" ? JSON.stringify(val) : String(val);
  nextTick(() => {
    const input = document.querySelector(".cell-edit-input") as HTMLInputElement;
    input?.focus();
    input?.select();
  });
}

function commitEdit() {
  if (isCancelling) return;
  if (!editingCell.value) return;
  const { rowId, col } = editingCell.value;
  const item = getRowItem(rowId);
  if (!item || item.isDeleted) {
    editingCell.value = null;
    return;
  }

  if (item.isNew && item.newIndex !== undefined) {
    const oldVal = newRows.value[item.newIndex]?.[col];
    const newVal = coerceCellValue(editValue.value, oldVal);
    if (newRows.value[item.newIndex]) {
      newRows.value[item.newIndex][col] = newVal;
    }
    editingCell.value = null;
    return;
  }

  if (item.sourceIndex === undefined) {
    editingCell.value = null;
    return;
  }

  const oldVal = props.result.rows[item.sourceIndex]?.[col];
  const newVal = coerceCellValue(editValue.value, oldVal);
  if (newVal !== oldVal) {
    if (!dirtyRows.value.has(item.sourceIndex)) dirtyRows.value.set(item.sourceIndex, new Map());
    dirtyRows.value.get(item.sourceIndex)!.set(col, newVal);
    // Enter transaction mode on first edit
    if (useTransaction.value && !transactionActive.value) {
      enterTransaction();
    }
  } else {
    const rowChanges = dirtyRows.value.get(item.sourceIndex);
    rowChanges?.delete(col);
    if (rowChanges?.size === 0) dirtyRows.value.delete(item.sourceIndex);
  }
  editingCell.value = null;
}

function cancelEdit() {
  const restoreScroll = preserveScrollPosition();
  isCancelling = true;
  focusScrollerWithoutScrolling();
  editingCell.value = null;
  restoreScrollAcrossFrames(restoreScroll);
}

function onEditKeydown(e: KeyboardEvent) {
  if (e.key === "Enter") {
    e.preventDefault();
    commitEdit();
  } else if (e.key === "Escape") {
    e.preventDefault();
    e.stopPropagation();
    cancelEdit();
  }
}

function addRow() {
  rowStatusFilter.value = rowStatusFilterAfterAddingRow(rowStatusFilter.value);
  newRows.value.push(props.result.columns.map(() => null));
  if (useTransaction.value && !transactionActive.value) {
    enterTransaction();
  }
  const rowId = -newRows.value.length;
  nextTick(() => {
    const el = getScrollerElement();
    if (el) el.scrollTop = el.scrollHeight;
    const firstEditableColumn = props.result.columns.findIndex((_, index) => isGridColumnEditable(index));
    if (firstEditableColumn >= 0) startEdit(rowId, firstEditableColumn);
  });
}

function applyDeleteRow(rowId: number) {
  const item = getRowItem(rowId);
  if (!item) return;
  if (item.isNew && item.newIndex !== undefined) {
    newRows.value.splice(item.newIndex, 1);
  } else if (item.sourceIndex !== undefined) {
    dirtyRows.value.delete(item.sourceIndex);
    deletedRows.value.add(item.sourceIndex);
  }
  if (editingCell.value?.rowId === rowId) editingCell.value = null;
  if (useTransaction.value && !transactionActive.value) {
    enterTransaction();
  }
}

const showDeleteRowConfirm = ref(false);
const pendingDeleteRowId = ref<number | null>(null);
const deleteRowDetails = computed(() =>
  props.tableMeta?.tableName
    ? t("dangerDialog.deleteRowDetails", { table: props.tableMeta.tableName })
    : t("dangerDialog.deleteRowDetailsNoTable"),
);

function requestDeleteRow(rowId: number) {
  pendingDeleteRowId.value = rowId;
  showDeleteRowConfirm.value = true;
}

function confirmDeleteRow() {
  if (pendingDeleteRowId.value === null) return;
  applyDeleteRow(pendingDeleteRowId.value);
  pendingDeleteRowId.value = null;
}

function restoreRow(rowId: number) {
  const item = getRowItem(rowId);
  if (item?.sourceIndex !== undefined) {
    deletedRows.value.delete(item.sourceIndex);
  }
}

function deleteSelectedRow() {
  if (!contextCell.value) return;
  requestDeleteRow(contextCell.value.rowId);
}

function generateSaveStatements(): string[] {
  if (!props.tableMeta) return [];
  return buildDataGridSaveStatements({
    databaseType: props.databaseType,
    tableMeta: props.tableMeta,
    columns: props.result.columns,
    rows: props.result.rows,
    dirtyRows: [...dirtyRows.value.entries()].map(([rowIndex, changes]) => [rowIndex, [...changes.entries()]]),
    deletedRows: [...deletedRows.value],
    newRows: newRows.value,
  });
}

async function saveChanges() {
  if (isFeishuBitableTableEditor.value) {
    saveError.value = "";
    isSaving.value = true;
    try {
      await saveFeishuBitableChanges();
    } catch (e: any) {
      saveError.value = String(e.message || e);
      isSaving.value = false;
      return;
    }
    dirtyRows.value.clear();
    newRows.value = [];
    deletedRows.value.clear();
    exitTransaction();
    isSaving.value = false;
    emit(
      "reload",
      props.sql,
      searchText.value,
      whereFilterInput.value.trim() || undefined,
      orderByInput.value.trim() || undefined,
    );
    return;
  }

  const stmts = generateSaveStatements();
  if (stmts.length === 0) return;
  saveError.value = "";
  isSaving.value = true;

  if (useTransaction.value && props.connectionId && props.database) {
    try {
      await api.executeInTransaction(props.connectionId, props.database, stmts, props.tableMeta?.schema);
    } catch (e: any) {
      saveError.value = String(e.message || e);
      isSaving.value = false;
      return;
    }
  } else if (props.connectionId && props.database) {
    try {
      await api.executeBatch(props.connectionId, props.database, stmts);
    } catch (e: any) {
      saveError.value = String(e.message || e);
      isSaving.value = false;
      return;
    }
  } else if (props.onExecuteSql) {
    try {
      for (const sql of stmts) {
        await props.onExecuteSql(sql);
      }
    } catch (e: any) {
      saveError.value = String(e.message || e);
      isSaving.value = false;
      return;
    }
  }
  dirtyRows.value.clear();
  newRows.value = [];
  deletedRows.value.clear();
  exitTransaction();
  isSaving.value = false;
  emit(
    "reload",
    props.sql,
    searchText.value,
    whereFilterInput.value.trim() || undefined,
    orderByInput.value.trim() || undefined,
  );
}

async function saveFeishuBitableChanges() {
  if (!props.connectionId || !props.tableMeta) {
    throw new Error("Missing Feishu Bitable table context");
  }

  const recordIdIndex = feishuBitableRecordIdColumnIndex();
  if (recordIdIndex < 0) {
    throw new Error("Feishu Bitable edits require the DBX record ID column");
  }

  const updates: ExternalRowUpdate[] = [];
  for (const [rowIndex, changes] of dirtyRows.value.entries()) {
    const row = props.result.rows[rowIndex];
    const rowId = String(row?.[recordIdIndex] ?? "").trim();
    if (!rowId) continue;

    const fields: Record<string, unknown> = {};
    for (const [columnIndex, value] of changes.entries()) {
      const columnName = props.result.columns[columnIndex];
      if (!columnName || columnName === feishuBitableRecordIdColumn.value) continue;
      fields[columnName] = value;
    }
    if (Object.keys(fields).length > 0) {
      updates.push({ rowId, fields });
    }
  }

  const rowIds = [...deletedRows.value]
    .map((rowIndex) => String(props.result.rows[rowIndex]?.[recordIdIndex] ?? "").trim())
    .filter(Boolean);

  const writableColumnIndexes = props.result.columns
    .map((columnName, index) => ({ columnName, index }))
    .filter(({ columnName }) => columnName !== feishuBitableRecordIdColumn.value)
    .map(({ index }) => index);
  const rowsToAppend = newRows.value.map((row) => writableColumnIndexes.map((index) => row[index] ?? null));

  if (updates.length > 0) {
    await api.updateExternalRows(props.connectionId, props.tableMeta.tableName, updates);
  }
  if (rowIds.length > 0) {
    await api.deleteExternalRows(props.connectionId, props.tableMeta.tableName, rowIds);
  }
  if (rowsToAppend.length > 0) {
    await api.appendExternalRows(props.connectionId, props.tableMeta.tableName, rowsToAppend);
  }
}

function discardChanges() {
  dirtyRows.value.clear();
  newRows.value = [];
  deletedRows.value.clear();
  editingCell.value = null;
  exitTransaction();
}

// --- Cell selection and detail ---
function clearCellSelection() {
  selectionAnchor.value = null;
  selectionFocus.value = null;
  isSelectingCells.value = false;
}

function selectSingleCell(rowIndex: number, colIndex: number) {
  const cell = { rowIndex, colIndex };
  selectionAnchor.value = cell;
  selectionFocus.value = cell;
}

function selectRow(rowIndex: number) {
  if (props.result.columns.length === 0) return;
  selectionAnchor.value = { rowIndex, colIndex: 0 };
  selectionFocus.value = { rowIndex, colIndex: props.result.columns.length - 1 };
}

function finishCellSelection() {
  isSelectingCells.value = false;
  document.removeEventListener("mouseup", finishCellSelection);
}

function beginCellSelection(rowIndex: number, colIndex: number, event: MouseEvent) {
  if (event.button !== 0) return;
  if (editingCell.value) return;
  event.preventDefault();
  selectSingleCell(rowIndex, colIndex);
  isSelectingCells.value = true;
  if (showTranspose.value) transposeRowIndex.value = rowIndex;
  document.addEventListener("mouseup", finishCellSelection);
}

function extendCellSelection(rowIndex: number, colIndex: number) {
  if (!isSelectingCells.value || !selectionAnchor.value) return;
  selectionFocus.value = { rowIndex, colIndex };
}

function cellIsSelected(rowIndex: number, colIndex: number): boolean {
  return isCellInSelection(rowIndex, colIndex, selectedRange.value);
}

function showCellDetails(rowIndex: number, colIndex: number) {
  detailCell.value = { rowIndex, col: colIndex };
  showCellDetail.value = true;
}

function copyText(text: string) {
  navigator.clipboard.writeText(text);
  toast(t("grid.copied"));
}

function copySelectionTsv() {
  if (!hasCellSelection.value) return;
  copyText(formatSelectionAsTsv(selectedCells.value));
}

function copySelectionCsv() {
  if (!hasCellSelection.value) return;
  copyText(formatSelectionAsCsv(selectedCells.value));
}

function copySelectionJson() {
  if (!hasCellSelection.value) return;
  copyText(formatSelectionAsJson(selectedCells.value));
}

function copySelectionSqlInList() {
  if (!hasCellSelection.value) return;
  copyText(formatSelectionAsSqlInList(selectedCells.value));
}

function copyDetailValue() {
  if (!activeCellDetail.value) return;
  copyText(activeCellDetail.value.rawValue);
}

function copyDetailColumnName() {
  if (!activeCellDetail.value) return;
  copyText(activeCellDetail.value.column);
}

function copyDetailSqlCondition() {
  const detail = activeCellDetail.value;
  if (!detail) return;
  const column = quoteIdent(detail.column);
  const condition = detail.value === null ? `${column} IS NULL` : `${column} = ${escapeVal(detail.value)}`;
  copyText(condition);
}

const transposeData = computed(() => {
  if (transposeRowIndex.value === null) return null;
  const item = displayItems.value[transposeRowIndex.value];
  if (!item) return null;
  return props.result.columns.map((col, i) => ({
    column: col,
    type: columnTypeMap.value.get(col) || "",
    value: item.data[i],
    display: formatCell(item.data[i]),
    isNull: item.data[i] === null,
  }));
});

function openTranspose(rowIndex: number) {
  transposeRowIndex.value = rowIndex;
  showTranspose.value = true;
  showCellDetail.value = false;
}

function transposeNav(delta: number) {
  if (transposeRowIndex.value === null) return;
  const next = transposeRowIndex.value + delta;
  if (next >= 0 && next < displayItems.value.length) {
    transposeRowIndex.value = next;
  }
}

watch(
  () => props.result,
  () => {
    if (resetScrollAfterResult) {
      resetScrollAfterResult = false;
      resetGridVerticalScroll();
    }
    clearCellSelection();
    showCellDetail.value = false;
    detailCell.value = null;
    showTranspose.value = false;
    transposeRowIndex.value = null;
    exitTransaction();
  },
);

// --- Copy/Export ---
function onCellContext(rowId: number, rowIndex: number, colIdx: number) {
  contextCell.value = { rowId, rowIndex, col: colIdx };
  if (!cellIsSelected(rowIndex, colIdx)) {
    selectSingleCell(rowIndex, colIdx);
  }
}

function onRowContext(rowId: number, rowIndex: number) {
  contextCell.value = { rowId, rowIndex, col: -1 };
  selectRow(rowIndex);
}

function copyCell() {
  if (!contextCell.value || contextCell.value.col < 0) return;
  const item = getRowItem(contextCell.value.rowId);
  const val = item?.data[contextCell.value.col] ?? null;
  copyText(formatCell(val));
}

function copyRow() {
  if (!contextCell.value) return;
  const item = getRowItem(contextCell.value.rowId);
  if (!item) return;
  const obj: Record<string, unknown> = {};
  props.result.columns.forEach((col, i) => {
    obj[col] = item.data[i];
  });
  copyText(JSON.stringify(obj, null, 2));
}

function copyRowAsInsert() {
  if (!contextCell.value) return;
  const item = getRowItem(contextCell.value.rowId);
  if (!item) return;
  const cols = props.result.columns.map((c) => quoteIdent(c)).join(", ");
  const vals = item.data.map((v) => escapeVal(v)).join(", ");
  const table = props.tableMeta
    ? (props.tableMeta.schema ? `${quoteIdent(props.tableMeta.schema)}.` : "") + quoteIdent(props.tableMeta.tableName)
    : "table_name";
  copyText(`INSERT INTO ${table} (${cols}) VALUES (${vals});`);
}

function copyAll() {
  const header = props.result.columns.join("\t");
  const body = displayItems.value.map((item) => item.data.map((c) => formatCell(c)).join("\t")).join("\n");
  copyText(`${header}\n${body}`);
}

async function saveFileContent(content: string, defaultFileName: string, filterName: string, filterExt: string) {
  if (isTauriRuntime()) {
    const { save } = await import("@tauri-apps/plugin-dialog");
    const { writeTextFile } = await import("@tauri-apps/plugin-fs");
    const path = await save({
      defaultPath: defaultFileName,
      filters: [{ name: filterName, extensions: [filterExt] }],
    });
    if (path) await writeTextFile(path, "﻿" + content);
  } else {
    const blob = new Blob(["﻿", content], { type: "text/csv;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = defaultFileName;
    a.click();
    URL.revokeObjectURL(url);
  }
}

async function exportCsv() {
  const escape = (v: string) => `"${v.replace(/"/g, '""')}"`;
  const header = props.result.columns.map(escape).join(",");
  const body = displayItems.value.map((item) => item.data.map((c) => escape(formatCell(c))).join(",")).join("\n");
  await saveFileContent(`${header}\n${body}`, "export.csv", "CSV", "csv");
}

async function exportJson() {
  const data = displayItems.value.map((item) => {
    const obj: Record<string, unknown> = {};
    props.result.columns.forEach((col, i) => {
      obj[col] = item.data[i];
    });
    return obj;
  });
  await saveFileContent(JSON.stringify(data, null, 2), "export.json", "JSON", "json");
}

async function exportMarkdown() {
  const cols = props.result.columns;
  const visibleRows = displayItems.value.map((item) => item.data);
  const md = formatMarkdownTable({ columns: cols, rows: visibleRows });
  await saveFileContent(md, "export.md", "Markdown", "md");
}

const sqlOneLiner = computed(() => props.sql?.replace(/\s+/g, " ").trim() || "");

function copySql() {
  if (!props.sql) return;
  navigator.clipboard.writeText(props.sql);
  toast(t("grid.copied"));
}

const showDdl = globalDdlOpen;
const ddlContent = ref("");
const ddlLoading = ref(false);
const ddlWidth = ref(320);
const ddlWrap = ref(true);
const isResizingDdl = ref(false);
const ddlSupported = computed(() => !isExternalTabular(props.databaseType));
let ddlResizeStartX = 0;
let ddlResizeStartWidth = 0;

const ddlDrawerStyle = computed(() => ({
  width: `${ddlWidth.value}px`,
}));

async function toggleDdl() {
  if (!ddlSupported.value) return;
  if (showDdl.value) {
    showDdl.value = false;
    return;
  }
  await fetchDdl();
}

async function fetchDdl() {
  if (!ddlSupported.value || !props.connectionId || !props.tableMeta) return;
  showDdl.value = true;
  ddlLoading.value = true;
  try {
    ddlContent.value = await api.getTableDdl(
      props.connectionId,
      props.database || "",
      props.tableMeta.schema || props.database || "",
      props.tableMeta.tableName,
    );
  } catch (e: any) {
    ddlContent.value = `-- Error: ${e}`;
  } finally {
    ddlLoading.value = false;
  }
}

if (showDdl.value && ddlSupported.value && props.tableMeta && props.connectionId) {
  fetchDdl();
}

function copyDdl() {
  navigator.clipboard.writeText(ddlContent.value);
  toast(t("grid.copied"));
}

function toggleDdlWrap() {
  ddlWrap.value = !ddlWrap.value;
}

function onDdlResizeStart(event: MouseEvent) {
  isResizingDdl.value = true;
  ddlResizeStartX = event.clientX;
  ddlResizeStartWidth = ddlWidth.value;
  document.body.classList.add("select-none", "cursor-col-resize");
  window.addEventListener("mousemove", onDdlResizeMove);
  window.addEventListener("mouseup", onDdlResizeEnd);
}

function onDdlResizeMove(event: MouseEvent) {
  if (!isResizingDdl.value) return;
  const nextWidth = ddlResizeStartWidth + ddlResizeStartX - event.clientX;
  ddlWidth.value = Math.min(Math.max(nextWidth, 240), 900);
}

function onDdlResizeEnd() {
  isResizingDdl.value = false;
  document.body.classList.remove("select-none", "cursor-col-resize");
  window.removeEventListener("mousemove", onDdlResizeMove);
  window.removeEventListener("mouseup", onDdlResizeEnd);
}

onUnmounted(() => {
  if (resetScrollFrame) cancelAnimationFrame(resetScrollFrame);
  if (cancelScrollRestoreFrame) cancelAnimationFrame(cancelScrollRestoreFrame);
  onDdlResizeEnd();
  finishCellSelection();
});

const SQL_KEYWORDS =
  /\b(CREATE|TABLE|INDEX|UNIQUE|PRIMARY|KEY|FOREIGN|REFERENCES|CONSTRAINT|NOT|NULL|DEFAULT|INT|INTEGER|BIGINT|SMALLINT|VARCHAR|CHARACTER|VARYING|TEXT|BOOLEAN|DOUBLE|PRECISION|REAL|FLOAT|NUMERIC|DECIMAL|TIMESTAMP|DATE|TIME|SERIAL|AUTOINCREMENT|AUTO_INCREMENT|IF|EXISTS|ON|SET|CASCADE|RESTRICT|CHECK|WITH|WITHOUT|ZONE)\b/gi;

function highlightSql(sql: string): string {
  const tokens: string[] = [];
  let rest = sql;
  const re = /("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')/g;
  let match: RegExpExecArray | null;
  let last = 0;
  while ((match = re.exec(rest)) !== null) {
    if (match.index > last) tokens.push(escapeAndHighlightKeywords(rest.slice(last, match.index)));
    const q = match[1];
    const escaped = q.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
    const cls = q.startsWith('"') ? "ddl-ident" : "ddl-str";
    tokens.push(`<span class="${cls}">${escaped}</span>`);
    last = re.lastIndex;
  }
  if (last < rest.length) tokens.push(escapeAndHighlightKeywords(rest.slice(last)));
  return tokens.join("");
}

function escapeAndHighlightKeywords(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(SQL_KEYWORDS, '<span class="ddl-kw">$1</span>');
}

defineExpose({
  useTransaction,
  transactionActive,
  isSaving,
  onToolbarRefresh,
  onToolbarCommit,
  onToolbarRollback,
  showDdl,
  ddlSupported,
  toggleDdl,
});
</script>

<template>
  <div ref="gridRef" class="h-full flex flex-col overflow-hidden" :style="columnVars">
    <ContextMenu>
      <ContextMenuTrigger as-child>
        <div v-if="hasData || canUseWhereSearch" class="flex-1 flex flex-col overflow-hidden">
          <!-- Search bar -->
          <div class="flex items-center border-b shrink-0 bg-muted/20 relative">
            <div class="flex-1 flex items-center gap-1 px-2 py-1 min-w-0">
              <Search class="w-3.5 h-3.5 text-muted-foreground shrink-0" />
              <input
                ref="searchInputRef"
                v-model="searchText"
                autocapitalize="off"
                autocorrect="off"
                spellcheck="false"
                class="flex-1 h-5 min-w-0 text-xs bg-transparent outline-none placeholder:text-muted-foreground"
                :placeholder="t('grid.search')"
                @keydown="onSearchKeydown"
                @click="updateSuggestionPosition"
              />
              <span
                ref="measureRef"
                class="invisible absolute left-0 top-0 text-xs whitespace-pre pointer-events-none"
                aria-hidden="true"
              />
              <!-- Suggestion dropdown -->
              <div
                v-if="searchSuggestions.length > 0"
                class="absolute top-full mt-0.5 z-50 min-w-[180px] rounded-md border bg-popover text-popover-foreground shadow-md"
                :style="{ left: suggestionLeft + 24 + 'px' }"
              >
                <div
                  v-for="(sug, idx) in searchSuggestions"
                  :key="sug"
                  class="flex items-center px-3 py-1.5 text-xs cursor-pointer"
                  :class="idx === suggestionIndex ? 'bg-accent text-accent-foreground' : 'hover:bg-accent/50'"
                  @mousedown.prevent="
                    suggestionIndex = idx;
                    acceptSuggestion();
                  "
                  @mouseenter="suggestionIndex = idx"
                >
                  <Search class="w-3 h-3 mr-2 text-muted-foreground shrink-0" />
                  <span>{{ sug }}</span>
                </div>
              </div>
              <span v-if="hasActiveFilter" class="text-xs text-muted-foreground shrink-0 px-1">
                {{ displayItems.length }}/{{ totalFilterableRowCount }}
              </span>
            </div>

            <template v-if="canUseWhereSearch">
              <div class="flex-1 flex items-center gap-1 px-2 py-1 border-l min-w-0 relative">
                <span class="text-foreground/60 text-xs font-medium select-none shrink-0">WHERE</span>
                <input
                  ref="whereFilterInputRef"
                  v-model="whereFilterInput"
                  autocapitalize="off"
                  autocorrect="off"
                  spellcheck="false"
                  class="flex-1 h-5 min-w-0 text-xs bg-transparent outline-none placeholder:text-muted-foreground/60"
                  placeholder="condition..."
                  @keydown="onWhereFilterKeydown"
                  @click="updateWhereSuggestionPosition"
                  @blur="dismissWhereSuggestions"
                />
                <span
                  ref="whereMeasureRef"
                  class="invisible absolute left-0 top-0 text-xs whitespace-pre pointer-events-none"
                  aria-hidden="true"
                />
                <!-- WHERE suggestion dropdown -->
                <div
                  v-if="whereSuggestions.length > 0"
                  class="absolute top-full mt-0.5 z-50 min-w-[180px] rounded-md border bg-popover text-popover-foreground shadow-md"
                  :style="{ left: whereSuggestionLeft + 24 + 'px' }"
                >
                  <div
                    v-for="(sug, idx) in whereSuggestions"
                    :key="sug"
                    class="flex items-center px-3 py-1.5 text-xs cursor-pointer"
                    :class="idx === whereSuggestionIndex ? 'bg-accent text-accent-foreground' : 'hover:bg-accent/50'"
                    @mousedown.prevent="
                      whereSuggestionIndex = idx;
                      acceptWhereSuggestion();
                    "
                    @mouseenter="whereSuggestionIndex = idx"
                  >
                    <Search class="w-3 h-3 mr-2 text-muted-foreground shrink-0" />
                    <span>{{ sug }}</span>
                  </div>
                </div>
                <button
                  v-if="hasWhereFilterInput"
                  class="text-muted-foreground hover:text-foreground shrink-0"
                  @click="
                    whereFilterInput = '';
                    applyWhereFilter();
                  "
                >
                  <X class="w-3 h-3" />
                </button>
              </div>
              <div class="flex-1 flex items-center gap-1 px-2 py-1 border-l border-r min-w-0 relative">
                <span class="text-foreground/60 text-xs font-medium select-none shrink-0">ORDER BY</span>
                <input
                  ref="orderByInputRef"
                  v-model="orderByInput"
                  autocapitalize="off"
                  autocorrect="off"
                  spellcheck="false"
                  class="flex-1 h-5 min-w-0 text-xs bg-transparent outline-none placeholder:text-muted-foreground/60"
                  placeholder="columns..."
                  @keydown="onOrderByKeydown"
                  @click="updateOrderBySuggestionPosition"
                  @blur="dismissOrderBySuggestions"
                />
                <span
                  ref="orderByMeasureRef"
                  class="invisible absolute left-0 top-0 text-xs whitespace-pre pointer-events-none"
                  aria-hidden="true"
                />
                <!-- ORDER BY suggestion dropdown -->
                <div
                  v-if="orderBySuggestions.length > 0"
                  class="absolute top-full mt-0.5 z-50 min-w-[180px] rounded-md border bg-popover text-popover-foreground shadow-md"
                  :style="{ left: orderBySuggestionLeft + 24 + 'px' }"
                >
                  <div
                    v-for="(sug, idx) in orderBySuggestions"
                    :key="sug"
                    class="flex items-center px-3 py-1.5 text-xs cursor-pointer"
                    :class="idx === orderBySuggestionIndex ? 'bg-accent text-accent-foreground' : 'hover:bg-accent/50'"
                    @mousedown.prevent="
                      orderBySuggestionIndex = idx;
                      acceptOrderBySuggestion();
                    "
                    @mouseenter="orderBySuggestionIndex = idx"
                  >
                    <Search class="w-3 h-3 mr-2 text-muted-foreground shrink-0" />
                    <span>{{ sug }}</span>
                  </div>
                </div>
                <button
                  v-if="hasOrderByInput"
                  class="text-muted-foreground hover:text-foreground shrink-0"
                  @click="
                    orderByInput = '';
                    applyOrderBySearch();
                  "
                >
                  <X class="w-3 h-3" />
                </button>
              </div>
            </template>

            <Button
              variant="ghost"
              size="sm"
              class="h-5 text-xs px-1.5 shrink-0"
              :disabled="isSaving"
              @click="onToolbarRefresh"
            >
              <Loader2 v-if="loading" class="w-3 h-3 mr-1 animate-spin" />
              <RefreshCcw v-else class="w-3 h-3 mr-1" />
              {{ t("grid.refresh") }}
            </Button>
            <Select
              v-if="canUseRowChangeControls"
              :model-value="rowStatusFilter"
              @update:model-value="(value: any) => setRowStatusFilter(String(value))"
            >
              <SelectTrigger class="h-6 w-28 px-2 text-xs">
                <SelectValue :placeholder="t('grid.filterRows')" />
              </SelectTrigger>
              <SelectContent position="popper">
                <SelectItem value="all">{{ t("grid.filterAllRows") }}</SelectItem>
                <SelectItem value="changed">{{ t("grid.filterChangedRows") }}</SelectItem>
                <SelectItem value="edited">{{ t("grid.statusEdited") }}</SelectItem>
                <SelectItem value="new">{{ t("grid.statusNew") }}</SelectItem>
                <SelectItem value="deleted">{{ t("grid.statusDeleted") }}</SelectItem>
              </SelectContent>
            </Select>
            <Button
              v-if="canUseRowChangeControls"
              variant="ghost"
              size="sm"
              class="h-5 text-xs px-1.5 shrink-0"
              @click="addRow"
            >
              <Plus class="w-3 h-3 mr-1" /> {{ t("grid.addRow") }}
            </Button>
            <span
              v-if="transactionActive"
              class="flex shrink-0 items-center gap-1 px-1 text-xs text-emerald-600 dark:text-emerald-400"
            >
              <span class="h-1.5 w-1.5 rounded-full bg-emerald-500 animate-pulse" />
              {{ t("grid.transactionActive") }}
            </span>
            <Button
              v-if="useTransaction"
              :variant="transactionActive ? 'default' : 'secondary'"
              size="sm"
              class="h-5 text-xs px-1.5"
              :disabled="!transactionActive || isSaving"
              @click="onToolbarCommit"
            >
              <Loader2 v-if="isSaving" class="w-3 h-3 mr-1 animate-spin" />
              <Save v-else class="w-3 h-3 mr-1" />
              {{ t("grid.commit") }}
            </Button>
            <Button
              v-if="useTransaction"
              variant="outline"
              size="sm"
              class="h-5 text-xs px-1.5"
              :disabled="!transactionActive"
              @click="onToolbarRollback"
            >
              <RotateCcw class="w-3 h-3 mr-1" />
              {{ t("grid.rollback") }}
            </Button>
          </div>
          <!-- Content area: table + DDL drawer -->
          <div class="flex-1 flex min-h-0 overflow-hidden">
            <div class="flex-1 flex flex-col min-w-0 overflow-hidden">
              <!-- Sticky header -->
              <div ref="headerRef" class="shrink-0 bg-muted/60 z-10 border-y border-border overflow-hidden">
                <div class="flex text-xs font-semibold text-foreground" :style="{ width: 'var(--total-w)' }">
                  <div
                    class="shrink-0 px-2 py-1.5 border-r border-border text-center text-muted-foreground select-none"
                    :style="{ width: 'var(--row-num-w)' }"
                  >
                    #
                  </div>
                  <Tooltip v-for="(col, colIdx) in result.columns" :key="`${col}-${colIdx}`">
                    <TooltipTrigger as-child>
                      <div
                        class="shrink-0 px-3 py-1.5 border-r border-border whitespace-nowrap cursor-pointer hover:bg-accent/60 select-none relative overflow-hidden"
                        :style="{ width: `var(--col-w-${colIdx})` }"
                        @click="toggleSort(col, colIdx)"
                      >
                        <span class="flex min-w-0 items-center gap-1 overflow-hidden group">
                          <span class="min-w-0 truncate">{{ col }}</span>
                          <ArrowUp
                            v-if="sortCol === col && sortColIndex === colIdx && sortDir === 'asc'"
                            class="h-3 w-3 shrink-0"
                          />
                          <ArrowDown
                            v-else-if="sortCol === col && sortColIndex === colIdx && sortDir === 'desc'"
                            class="h-3 w-3 shrink-0"
                          />
                          <ArrowUpDown
                            v-else
                            class="h-3 w-3 shrink-0 opacity-0 group-hover:opacity-30 transition-opacity"
                          />
                        </span>
                        <div
                          class="absolute right-0 top-0 bottom-0 w-1.5 cursor-col-resize hover:bg-primary/30"
                          @mousedown.stop="onResizeStart(colIdx, $event)"
                          @dblclick.stop="autoFitColumn(colIdx)"
                        />
                      </div>
                    </TooltipTrigger>
                    <TooltipContent
                      v-if="columnTypeMap.get(col) || columnCommentMap.get(col)"
                      side="bottom"
                      class="text-xs grid grid-cols-[auto_1fr] gap-x-2"
                    >
                      <template v-if="columnTypeMap.get(col)">
                        <span class="text-muted-foreground">{{ t("grid.columnType") }}</span>
                        <span :class="typeColorClass(columnTypeMap.get(col)!)">{{ columnTypeMap.get(col) }}</span>
                      </template>
                      <template v-if="columnCommentMap.get(col)">
                        <span class="text-muted-foreground">{{ t("grid.columnComment") }}</span>
                        <span>{{ columnCommentMap.get(col) }}</span>
                      </template>
                    </TooltipContent>
                  </Tooltip>
                </div>
              </div>

              <div
                v-if="isErrorResult"
                class="flex-1 flex flex-col items-center justify-center gap-2 px-6 text-center text-destructive"
              >
                <TriangleAlert class="h-8 w-8 text-destructive/50" aria-hidden="true" />
                <div class="space-y-1">
                  <div class="text-sm font-medium">{{ t("grid.queryError") }}</div>
                  <div class="text-xs max-w-lg break-all text-destructive/80">{{ errorMessage }}</div>
                </div>
              </div>

              <div
                v-else-if="!hasVisibleRows"
                class="flex-1 flex flex-col items-center justify-center gap-2 px-6 text-center text-muted-foreground"
              >
                <component
                  :is="hasActiveFilter ? SearchX : Inbox"
                  class="h-8 w-8 text-muted-foreground/50"
                  aria-hidden="true"
                />
                <div class="space-y-1">
                  <div class="text-sm font-medium text-foreground">{{ emptyTitle }}</div>
                  <div class="text-xs">{{ emptyDescription }}</div>
                </div>
              </div>

              <!-- Virtual scrolled rows -->
              <RecycleScroller
                v-else
                ref="scrollerRef"
                class="data-grid-scroller flex-1 overflow-x-auto overscroll-none"
                :items="displayItems"
                :item-size="26"
                key-field="id"
                @scroll="syncHeaderScroll"
              >
                <template #default="{ item, index }">
                  <div
                    class="flex text-xs border-b border-border hover:bg-accent/50"
                    :class="{
                      'bg-destructive/5 opacity-70': item.isDeleted,
                      'bg-primary/5': item.isNew,
                      'bg-muted/30': !item.isNew && !item.isDeleted && index % 2 === 1,
                    }"
                    :style="{ height: '26px', width: 'var(--total-w)' }"
                  >
                    <div
                      class="shrink-0 px-2 py-1 border-r border-border text-center select-none cursor-default hover:bg-accent/50"
                      :class="rowNumberStatusClass(item)"
                      :style="{ width: 'var(--row-num-w)' }"
                      @click="selectRow(index)"
                      @contextmenu="onRowContext(item.id, index)"
                    >
                      {{ index + 1 }}
                    </div>
                    <div
                      v-for="(cell, colIdx) in item.data"
                      :key="colIdx"
                      class="group/cell shrink-0 px-3 py-1 border-r border-border whitespace-nowrap overflow-hidden text-ellipsis relative select-none"
                      :style="{ width: `var(--col-w-${colIdx})` }"
                      :class="{
                        'text-muted-foreground italic': isNull(cell),
                        'bg-yellow-500/10': item.isDirtyCol[colIdx],
                        'cell-selected': cellIsSelected(index, colIdx),
                        'tabular-nums': typeof cell === 'number',
                        'cursor-text hover:bg-accent/50': editable && !item.isDeleted,
                        'line-through': item.isDeleted,
                      }"
                      @mousedown="beginCellSelection(index, colIdx, $event)"
                      @mouseenter="extendCellSelection(index, colIdx)"
                      @dblclick="isGridColumnEditable(colIdx) && !item.isDeleted && startEdit(item.id, colIdx)"
                      @contextmenu="onCellContext(item.id, index, colIdx)"
                    >
                      <template v-if="editingCell?.rowId === item.id && editingCell?.col === colIdx">
                        <input
                          v-model="editValue"
                          autocapitalize="off"
                          autocorrect="off"
                          spellcheck="false"
                          class="cell-edit-input absolute inset-0 bg-background border-2 border-primary px-2 py-0.5 text-xs outline-none z-10"
                          @blur="commitEdit"
                          @click.stop
                          @keydown.stop="onEditKeydown"
                        />
                      </template>
                      <template v-else>
                        {{ formatCell(cell) }}
                        <button
                          class="absolute right-0.5 top-0.5 hidden h-5 w-5 items-center justify-center rounded bg-background/90 text-muted-foreground shadow-sm ring-1 ring-border hover:text-foreground group-hover/cell:flex"
                          :title="t('grid.cellDetails')"
                          @mousedown.stop
                          @click.stop="showCellDetails(index, colIdx)"
                        >
                          <Info class="h-3 w-3" />
                        </button>
                      </template>
                    </div>
                  </div>
                </template>
              </RecycleScroller>
            </div>
            <!-- DDL Drawer -->
            <div
              v-if="showDdl && ddlSupported"
              class="relative shrink-0 border-l flex flex-col bg-background min-w-0"
              :class="{ 'ddl-drawer-resizing': isResizingDdl }"
              :style="ddlDrawerStyle"
            >
              <div
                class="absolute left-0 top-0 bottom-0 z-20 w-1.5 -translate-x-1/2 cursor-col-resize hover:bg-primary/30"
                @mousedown.prevent="onDdlResizeStart"
              />
              <div class="flex items-center gap-2 px-3 py-1.5 border-b shrink-0 bg-muted/20">
                <Code2 class="w-3.5 h-3.5 text-muted-foreground" />
                <span class="text-xs font-medium flex-1 min-w-0 truncate">{{ tableMeta?.tableName }} DDL</span>
                <Button variant="ghost" size="icon" class="h-5 w-5" @click="copyDdl">
                  <Copy class="w-3 h-3" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-5 w-5"
                  :class="{ 'bg-accent': ddlWrap }"
                  @click="toggleDdlWrap"
                >
                  <WrapText class="w-3 h-3" />
                </Button>
                <Button variant="ghost" size="icon" class="h-5 w-5" @click="showDdl = false">
                  <X class="w-3 h-3" />
                </Button>
              </div>
              <div v-if="ddlLoading" class="flex-1 flex items-center justify-center">
                <Loader2 class="w-4 h-4 animate-spin text-muted-foreground" />
              </div>
              <pre
                v-else
                class="flex-1 min-w-0 text-xs font-mono p-3 overflow-auto ddl-code leading-5 select-text"
                :class="ddlWrap ? 'whitespace-pre-wrap break-words' : 'whitespace-pre'"
                v-html="highlightSql(ddlContent)"
              ></pre>
            </div>
            <!-- Cell Detail Drawer -->
            <div
              v-if="showCellDetail && activeCellDetail"
              class="relative w-80 shrink-0 border-l flex flex-col bg-background min-w-0"
            >
              <div class="h-9 flex items-center gap-2 px-3 border-b shrink-0 bg-muted/20">
                <Info class="w-3.5 h-3.5 text-muted-foreground" />
                <span class="text-xs font-medium flex-1 min-w-0 truncate">{{ t("grid.cellDetails") }}</span>
                <Button variant="ghost" size="icon" class="h-5 w-5" @click="showCellDetail = false">
                  <X class="w-3 h-3" />
                </Button>
              </div>

              <div class="flex-1 min-h-0 overflow-auto p-3 text-xs space-y-3">
                <div class="space-y-1">
                  <div class="text-muted-foreground">{{ t("grid.columnName") }}</div>
                  <div class="font-medium break-all">{{ activeCellDetail.column }}</div>
                </div>
                <div class="grid grid-cols-2 gap-3">
                  <div class="space-y-1">
                    <div class="text-muted-foreground">{{ t("grid.rowNumber") }}</div>
                    <div>{{ activeCellDetail.rowNumber }}</div>
                  </div>
                  <div class="space-y-1">
                    <div class="text-muted-foreground">{{ t("grid.columnType") }}</div>
                    <div
                      :class="activeCellDetail.type ? typeColorClass(activeCellDetail.type) : 'text-muted-foreground'"
                    >
                      {{ activeCellDetail.type || "-" }}
                    </div>
                  </div>
                  <div class="space-y-1">
                    <div class="text-muted-foreground">{{ t("grid.nullValue") }}</div>
                    <div>{{ activeCellDetail.value === null ? "true" : "false" }}</div>
                  </div>
                  <div class="space-y-1">
                    <div class="text-muted-foreground">{{ t("grid.valueLength") }}</div>
                    <div>{{ activeCellDetail.length }}</div>
                  </div>
                </div>
                <div class="space-y-1">
                  <div class="text-muted-foreground">{{ t("grid.columnComment") }}</div>
                  <div class="whitespace-pre-wrap break-words">
                    {{ activeCellDetail.comment || t("grid.noComment") }}
                  </div>
                </div>
                <div class="space-y-1">
                  <div class="text-muted-foreground">{{ t("grid.cellValue") }}</div>
                  <template v-if="isEditingDetail">
                    <textarea
                      v-model="detailEditValue"
                      class="w-full h-40 rounded border bg-background p-2 font-mono text-xs outline-none resize-y focus:border-primary"
                      @keydown.escape.stop="cancelDetailEdit"
                    />
                    <div class="flex gap-1 mt-1">
                      <Button size="sm" class="h-6 text-xs" @click="commitDetailEdit">
                        {{ t("dangerDialog.confirm") }}
                      </Button>
                      <Button variant="outline" size="sm" class="h-6 text-xs" @click="cancelDetailEdit">
                        {{ t("dangerDialog.cancel") }}
                      </Button>
                    </div>
                  </template>
                  <pre
                    v-else
                    class="max-h-56 overflow-auto rounded border bg-muted/20 p-2 font-mono text-xs whitespace-pre-wrap break-words cursor-pointer hover:border-primary/50"
                    :class="{ 'cursor-text': activeCellDetail.isEditable }"
                    @dblclick="startDetailEdit"
                    >{{ activeCellDetail.rawValue }}</pre
                  >
                </div>
                <div v-if="activeCellDetail.formattedJson" class="space-y-1">
                  <div class="text-muted-foreground">{{ t("grid.formattedJson") }}</div>
                  <pre
                    class="max-h-72 overflow-auto rounded border bg-muted/20 p-2 font-mono text-xs whitespace-pre-wrap break-words"
                    >{{ activeCellDetail.formattedJson }}</pre
                  >
                </div>
              </div>

              <div class="border-t p-2 grid grid-cols-1 gap-1">
                <Button
                  v-if="activeCellDetail.isEditable && !isEditingDetail"
                  variant="ghost"
                  size="sm"
                  class="h-7 justify-start text-xs"
                  @click="startDetailEdit"
                >
                  <Pencil class="w-3 h-3 mr-2" /> {{ t("grid.editValue") }}
                </Button>
                <Button
                  v-if="activeCellDetail.isEditable && activeCellDetail.value !== null"
                  variant="ghost"
                  size="sm"
                  class="h-7 justify-start text-xs"
                  @click="setDetailNull"
                >
                  <X class="w-3 h-3 mr-2" /> {{ t("grid.setNull") }}
                </Button>
                <Button variant="ghost" size="sm" class="h-7 justify-start text-xs" @click="copyDetailValue">
                  <Copy class="w-3 h-3 mr-2" /> {{ t("grid.copyValue") }}
                </Button>
                <Button variant="ghost" size="sm" class="h-7 justify-start text-xs" @click="copyDetailColumnName">
                  <Copy class="w-3 h-3 mr-2" /> {{ t("grid.copyColumnName") }}
                </Button>
                <Button variant="ghost" size="sm" class="h-7 justify-start text-xs" @click="copyDetailSqlCondition">
                  <Code2 class="w-3 h-3 mr-2" /> {{ t("grid.copySqlCondition") }}
                </Button>
              </div>
            </div>
            <!-- Transpose Panel -->
            <div
              v-if="showTranspose && transposeData"
              class="relative w-80 shrink-0 border-l flex flex-col bg-background min-w-0"
            >
              <div class="h-9 flex items-center gap-2 px-3 border-b shrink-0 bg-muted/20">
                <Rows3 class="w-3.5 h-3.5 text-muted-foreground" />
                <span class="text-xs font-medium">{{ t("grid.transpose") }}</span>
                <span class="text-xs text-muted-foreground"
                  >{{ t("grid.rowNumber") }} {{ (transposeRowIndex ?? 0) + 1 }}</span
                >
                <span class="flex-1" />
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-5 w-5"
                  :disabled="transposeRowIndex === 0"
                  @click="transposeNav(-1)"
                >
                  <ChevronLeft class="w-3 h-3" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-5 w-5"
                  :disabled="transposeRowIndex === displayItems.length - 1"
                  @click="transposeNav(1)"
                >
                  <ChevronRight class="w-3 h-3" />
                </Button>
                <Button variant="ghost" size="icon" class="h-5 w-5" @click="showTranspose = false">
                  <X class="w-3 h-3" />
                </Button>
              </div>
              <div class="flex-1 min-h-0 overflow-auto">
                <table class="w-full text-xs">
                  <tbody>
                    <tr
                      v-for="(field, fi) in transposeData"
                      :key="fi"
                      class="border-b border-border/50 hover:bg-accent/50"
                    >
                      <td class="px-3 py-1.5 font-medium text-muted-foreground whitespace-nowrap w-1/3 align-top">
                        <div>{{ field.column }}</div>
                        <div v-if="field.type" :class="typeColorClass(field.type)" class="text-[10px]">
                          {{ field.type }}
                        </div>
                      </td>
                      <td class="px-3 py-1.5 break-all" :class="{ 'text-muted-foreground italic': field.isNull }">
                        {{ field.display }}
                      </td>
                    </tr>
                  </tbody>
                </table>
              </div>
            </div>
          </div>
        </div>
      </ContextMenuTrigger>

      <ContextMenuContent class="w-max max-w-[min(80vw,20rem)]">
        <template v-if="contextColumn">
          <ContextMenuItem @click="applyContextSort('asc')">
            <ArrowUp class="w-3.5 h-3.5 mr-2" /> {{ t("grid.sortAscending") }}
          </ContextMenuItem>
          <ContextMenuItem @click="applyContextSort('desc')">
            <ArrowDown class="w-3.5 h-3.5 mr-2" /> {{ t("grid.sortDescending") }}
          </ContextMenuItem>
          <ContextMenuItem v-if="sortCol" @click="applyContextSort(null)">
            <ArrowUpDown class="w-3.5 h-3.5 mr-2" /> {{ t("grid.clearSort") }}
          </ContextMenuItem>
          <ContextMenuSub v-if="canUseWhereSearch">
            <ContextMenuSubTrigger> <Filter class="w-3.5 h-3.5 mr-2" /> {{ t("grid.filter") }} </ContextMenuSubTrigger>
            <ContextMenuSubContent class="w-max max-w-[min(80vw,18rem)]">
              <ContextMenuItem @click="applyContextFilter('equals')">{{ t("grid.filterByValue") }}</ContextMenuItem>
              <ContextMenuItem @click="applyContextFilter('not-equals')">
                {{ t("grid.filterExcludeValue") }}
              </ContextMenuItem>
              <ContextMenuItem @click="applyContextFilter('is-null')">{{ t("grid.filterIsNull") }}</ContextMenuItem>
              <ContextMenuItem @click="applyContextFilter('is-not-null')">
                {{ t("grid.filterIsNotNull") }}
              </ContextMenuItem>
              <ContextMenuSeparator />
              <ContextMenuItem @click="clearContextFilter">{{ t("grid.clearFilter") }}</ContextMenuItem>
            </ContextMenuSubContent>
          </ContextMenuSub>
          <ContextMenuSeparator />
        </template>
        <ContextMenuSub>
          <ContextMenuSubTrigger> <Copy class="w-3.5 h-3.5 mr-2" /> {{ t("grid.copy") }} </ContextMenuSubTrigger>
          <ContextMenuSubContent class="w-max max-w-[min(80vw,18rem)]">
            <ContextMenuItem v-if="contextColumn" @click="copyCell">{{ t("grid.copyCell") }}</ContextMenuItem>
            <ContextMenuItem @click="copyRow">{{ t("grid.copyRow") }}</ContextMenuItem>
            <ContextMenuItem @click="copyRowAsInsert">{{ t("grid.copyRowInsert") }}</ContextMenuItem>
            <ContextMenuItem @click="copyAll">{{ t("grid.copyAll") }}</ContextMenuItem>
          </ContextMenuSubContent>
        </ContextMenuSub>
        <ContextMenuItem v-if="contextCell" @click="openTranspose(contextCell.rowIndex)">
          <Rows3 class="w-3.5 h-3.5 mr-2" /> {{ t("grid.transpose") }}
        </ContextMenuItem>
        <ContextMenuSub v-if="hasCellSelection">
          <ContextMenuSubTrigger>
            <SquareDashed class="w-3.5 h-3.5 mr-2" /> {{ t("grid.selection") }}
          </ContextMenuSubTrigger>
          <ContextMenuSubContent class="w-max max-w-[min(80vw,20rem)]">
            <ContextMenuItem @click="copySelectionTsv">{{ t("grid.copySelectionTsv") }}</ContextMenuItem>
            <ContextMenuItem @click="copySelectionCsv">{{ t("grid.copySelectionCsv") }}</ContextMenuItem>
            <ContextMenuItem @click="copySelectionJson">{{ t("grid.copySelectionJson") }}</ContextMenuItem>
            <ContextMenuItem @click="copySelectionSqlInList">{{ t("grid.copySelectionSql") }}</ContextMenuItem>
            <ContextMenuSeparator />
            <ContextMenuItem @click="clearCellSelection">{{ t("grid.clearSelection") }}</ContextMenuItem>
          </ContextMenuSubContent>
        </ContextMenuSub>
        <ContextMenuSeparator />
        <template v-if="editable && contextRowItem">
          <ContextMenuItem v-if="contextRowItem.isDeleted" @click="restoreRow(contextRowItem.id)">
            <Undo2 class="w-3.5 h-3.5 mr-2" /> {{ t("grid.restoreRow") }}
          </ContextMenuItem>
          <ContextMenuItem v-else class="text-destructive" @click="deleteSelectedRow">
            <Trash2 class="w-3.5 h-3.5 mr-2" /> {{ t("grid.deleteRow") }}
          </ContextMenuItem>
          <ContextMenuSeparator />
        </template>
        <ContextMenuSub>
          <ContextMenuSubTrigger> <FileDown class="w-3.5 h-3.5 mr-2" /> {{ t("grid.export") }} </ContextMenuSubTrigger>
          <ContextMenuSubContent class="w-max max-w-[min(80vw,16rem)]">
            <ContextMenuItem @click="exportCsv">{{ t("grid.exportCsv") }}</ContextMenuItem>
            <ContextMenuItem @click="exportJson">{{ t("grid.exportJson") }}</ContextMenuItem>
            <ContextMenuItem @click="exportMarkdown">{{ t("grid.exportMarkdown") }}</ContextMenuItem>
          </ContextMenuSubContent>
        </ContextMenuSub>
      </ContextMenuContent>
    </ContextMenu>

    <div v-if="!hasData" class="flex-1 flex items-center justify-center text-muted-foreground text-sm">
      {{ t("grid.querySuccess") }}
    </div>

    <!-- Error bar -->
    <div
      v-if="saveError"
      class="px-3 py-1.5 border-t bg-destructive/10 text-destructive text-xs shrink-0 flex items-center gap-2"
    >
      <span class="flex-1">{{ saveError }}</span>
      <button class="hover:underline" @click="saveError = ''">{{ t("grid.dismiss") }}</button>
    </div>

    <!-- Bottom status bar -->
    <div class="flex items-center gap-2 px-3 py-1 border-t text-xs text-muted-foreground bg-muted/30 shrink-0">
      <span v-if="hasData">{{ t("grid.totalRows", { count: result.rows.length }) }}</span>
      <span v-else>{{ t("grid.rowsAffected", { count: result.affected_rows }) }}</span>
      <span>{{ result.execution_time_ms }}ms</span>
      <span v-if="hasCellSelection" class="text-foreground">{{ selectionSummary }}</span>

      <template v-if="editable && tableMeta && !transactionActive">
        <span v-if="hasPendingChanges" class="ml-2 text-foreground">
          {{ t("grid.pendingChanges", { count: pendingChangeCount }) }}
        </span>
        <Button v-if="hasPendingChanges" variant="default" size="sm" class="h-5 text-xs ml-2" @click="saveChanges">
          <Save class="w-3 h-3 mr-1" /> {{ t("grid.save") }}
        </Button>
        <Button v-if="hasPendingChanges" variant="ghost" size="sm" class="h-5 text-xs" @click="discardChanges">
          {{ t("grid.discard") }}
        </Button>
      </template>

      <span class="ml-auto flex items-center gap-1">
        <Loader2 v-if="loading" class="w-3 h-3 animate-spin text-muted-foreground" />
        <DropdownMenu>
          <DropdownMenuTrigger as-child>
            <Button variant="ghost" size="sm" class="h-5 text-xs px-1.5">
              {{ pageSize }}{{ t("grid.rowsPerPageShort") }}
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent>
            <DropdownMenuItem v-for="s in [50, 100, 500, 1000]" :key="s" @click="changePageSize(s)">
              {{ s }} {{ t("grid.rowsPerPageShort") }}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        <Button variant="ghost" size="icon" class="h-5 w-5" :disabled="currentPage <= 1" @click="prevPage">
          <ChevronLeft class="h-3 w-3" />
        </Button>
        <span>{{ currentPage }}</span>
        <Button variant="ghost" size="icon" class="h-5 w-5" :disabled="!isFullPage" @click="nextPage">
          <ChevronRight class="h-3 w-3" />
        </Button>
      </span>

      <DropdownMenu>
        <DropdownMenuTrigger as-child>
          <Button variant="ghost" size="icon" class="h-5 w-5">
            <Download class="h-3 w-3" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent>
          <DropdownMenuItem @click="exportCsv">{{ t("grid.exportCsv") }}</DropdownMenuItem>
          <DropdownMenuItem @click="exportJson">{{ t("grid.exportJson") }}</DropdownMenuItem>
          <DropdownMenuItem @click="exportMarkdown">{{ t("grid.exportMarkdown") }}</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <Tooltip v-if="sqlOneLiner">
        <TooltipTrigger as-child>
          <span class="truncate max-w-[30%] opacity-60 cursor-pointer hover:opacity-100" @click="copySql">
            {{ sqlOneLiner }}
          </span>
        </TooltipTrigger>
        <TooltipContent side="top" class="max-w-md">
          <pre class="text-xs font-mono whitespace-pre-wrap">{{ props.sql }}</pre>
        </TooltipContent>
      </Tooltip>
    </div>

    <DangerConfirmDialog
      v-model:open="showDeleteRowConfirm"
      :message="t('dangerDialog.deleteRowMessage')"
      :details="deleteRowDetails"
      :confirm-label="t('grid.deleteRow')"
      @confirm="confirmDeleteRow"
    />
  </div>
</template>

<style scoped>
.data-grid-scroller {
  overflow-anchor: none;
}

.data-grid-scroller :deep(.vue-recycle-scroller__item-wrapper) {
  min-width: var(--total-w);
  overflow: visible;
}

.ddl-drawer-resizing {
  transition: none;
}

.cell-selected {
  background-color: color-mix(in oklab, var(--primary) 18%, transparent);
  box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--primary) 55%, transparent);
}

.ddl-code :deep(.ddl-kw) {
  color: oklch(0.6 0.15 250);
  font-weight: 600;
}
.ddl-code :deep(.ddl-ident) {
  color: oklch(0.65 0.15 150);
}
.ddl-code :deep(.ddl-str) {
  color: oklch(0.65 0.15 50);
}
</style>
