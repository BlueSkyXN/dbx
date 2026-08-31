use std::collections::HashSet;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::feishu::{FeishuClient, FeishuRequestErrorKind};
use super::file_support::{
    json_number_as_safe_f64, json_numbers_equal, parse_index_key, scoped_snapshot_token, unique_display_names,
};
use super::{
    AdapterCapabilities, ApplyChangesRequest, ApplyChangesResult, ConflictMode, DeleteMode, ExternalCellInput,
    ExternalColumn, ExternalConnectionTestResult, ExternalOperation, ExternalRow, ExternalTableAdapter,
    ExternalTableError, ExternalTableRef, ExternalTableSchema, ExternalValueType, FeishuSheetsExternalConfig,
    InsertMode, OperationOutcome, OperationResult, PageSnapshot, ReadPageRequest, ReadState,
};

const SHEET_KEY_PREFIX: &str = "sheet:";
const MAX_PAGE_SIZE: usize = 500;
const MAX_COLUMNS: u32 = 500;
const MAX_BATCH_OPERATIONS: usize = 100;
const USED_RANGE_PROBE_MAX_CHARS: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct FeishuSheetsAdapter {
    client: FeishuClient,
    config: FeishuSheetsExternalConfig,
    write_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
}

#[derive(Debug, Clone)]
struct WorkbookSheet {
    sheet_id: String,
    title: String,
    row_count: u32,
    column_count: u32,
}

#[derive(Debug, Clone)]
struct WorkbookStructure {
    revision: String,
    sheets: Vec<WorkbookSheet>,
}

#[derive(Debug, Clone, Copy)]
struct SheetBounds {
    start_row: u32,
    end_row: u32,
    start_col: u32,
    end_col: u32,
    empty: bool,
}

impl SheetBounds {
    fn contains(self, other: Self) -> bool {
        other.empty
            || (self.start_row <= other.start_row
                && self.end_row >= other.end_row
                && self.start_col <= other.start_col
                && self.end_col >= other.end_col)
    }
}

#[derive(Debug, Clone)]
struct SheetCell {
    value: Value,
    readonly: bool,
    incomplete: bool,
}

#[derive(Debug, Clone)]
struct CellRange {
    cells: Vec<Vec<SheetCell>>,
    incomplete: bool,
}

impl FeishuSheetsAdapter {
    pub fn new(
        app_id: impl Into<String>,
        app_secret: impl Into<String>,
        config: FeishuSheetsExternalConfig,
        timeout: Duration,
    ) -> Result<Self, ExternalTableError> {
        let client = FeishuClient::new(app_id, app_secret, timeout)?;
        Self::from_client(client, config)
    }

    pub(crate) fn from_client(
        client: FeishuClient,
        config: FeishuSheetsExternalConfig,
    ) -> Result<Self, ExternalTableError> {
        if config.spreadsheet_token.trim().is_empty() {
            return Err(ExternalTableError::invalid("Feishu Sheets spreadsheetToken is required"));
        }
        Ok(Self { client, config, write_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())) })
    }

    fn table_ref(sheet: &WorkbookSheet) -> ExternalTableRef {
        ExternalTableRef {
            table_key: format!(
                "{SHEET_KEY_PREFIX}{}",
                percent_encoding::utf8_percent_encode(&sheet.sheet_id, percent_encoding::NON_ALPHANUMERIC)
            ),
            display_name: sheet.title.clone(),
        }
    }

    fn table_sheet_id(table: &ExternalTableRef) -> Result<String, ExternalTableError> {
        let value = table
            .table_key
            .strip_prefix(SHEET_KEY_PREFIX)
            .ok_or_else(|| ExternalTableError::invalid(format!("Invalid Feishu sheet key: {}", table.table_key)))?;
        percent_encoding::percent_decode_str(value)
            .decode_utf8()
            .map(|value| value.into_owned())
            .map_err(|_| ExternalTableError::invalid(format!("Invalid Feishu sheet key: {}", table.table_key)))
    }

    async fn structure(&self) -> Result<WorkbookStructure, ExternalTableError> {
        let output = self
            .client
            .invoke_sheet_tool(
                &self.config.spreadsheet_token,
                "get_workbook_structure",
                json!({ "excel_id": self.config.spreadsheet_token }),
                false,
            )
            .await
            .map_err(|error| error.as_external_error())?;
        parse_structure(&output, self.config.sheet_id.as_deref())
    }

    async fn fetch_ranges(&self, sheet_id: &str, ranges: Vec<String>) -> Result<Vec<CellRange>, ExternalTableError> {
        let output = self
            .client
            .invoke_sheet_tool(
                &self.config.spreadsheet_token,
                "get_cell_ranges",
                json!({
                    "excel_id": self.config.spreadsheet_token,
                    "sheet_id": sheet_id,
                    "ranges": ranges,
                    "include_styles": true,
                    "include_truncation_info": true,
                    "value_render_option": "formula"
                }),
                false,
            )
            .await
            .map_err(|error| error.as_external_error())?;
        parse_cell_ranges(&output)
    }

    async fn fetch_row_values(
        &self,
        sheet_id: &str,
        bounds: SheetBounds,
        row: u32,
        physical_row_count: u32,
    ) -> Result<Vec<Value>, ExternalTableError> {
        let width = bounds.end_col.saturating_sub(bounds.start_col).saturating_add(1) as usize;
        if row > physical_row_count {
            return Ok(vec![Value::Null; width]);
        }
        let range = format!("{}{}:{}{}", column_label(bounds.start_col), row, column_label(bounds.end_col), row);
        let fetched = self.fetch_ranges(sheet_id, vec![range]).await?;
        let fetched = fetched
            .first()
            .ok_or_else(|| ExternalTableError::invalid("Feishu Sheets row readback returned no range"))?;
        if fetched.incomplete {
            return Err(ExternalTableError::invalid("Feishu Sheets row readback was incomplete"));
        }
        Ok((0..width)
            .map(|index| {
                fetched
                    .cells
                    .first()
                    .and_then(|cells| cells.get(index))
                    .map(|cell| cell.value.clone())
                    .unwrap_or(Value::Null)
            })
            .collect())
    }

    async fn bounds(&self, sheet: &WorkbookSheet) -> Result<SheetBounds, ExternalTableError> {
        if self.config.data_range.as_deref().is_some_and(|value| !value.trim().is_empty()) {
            return resolve_bounds(sheet, self.config.data_range.as_deref());
        }
        self.detected_bounds(sheet).await
    }

    async fn detected_bounds(&self, sheet: &WorkbookSheet) -> Result<SheetBounds, ExternalTableError> {
        let anchor = format!("A1:{}{}", column_label(sheet.column_count), sheet.row_count);
        let output = self
            .client
            .invoke_sheet_tool(
                &self.config.spreadsheet_token,
                "get_range_as_csv",
                json!({
                    "excel_id": self.config.spreadsheet_token,
                    "sheet_id": sheet.sheet_id,
                    "range": anchor,
                    "max_rows": sheet.row_count,
                    "max_chars": USED_RANGE_PROBE_MAX_CHARS
                }),
                false,
            )
            .await
            .map_err(|error| error.as_external_error())?;
        if output.get("truncated").and_then(Value::as_bool).unwrap_or(false)
            || output.get("has_more").and_then(Value::as_bool).unwrap_or(false)
            || output.get("truncation_warning").is_some()
        {
            return Err(ExternalTableError::unsupported(
                "Feishu Sheets used-range probe was truncated; configure an explicit data range before editing",
            ));
        }
        let region = output
            .get("current_region")
            .or_else(|| output.get("actual_range"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let Some(region) = region else {
            if sheet.column_count > MAX_COLUMNS {
                return Err(ExternalTableError::unsupported(format!(
                    "Feishu Sheets grid has {} columns; at most {MAX_COLUMNS} are supported",
                    sheet.column_count
                )));
            }
            return Ok(SheetBounds { start_row: 1, end_row: 1, start_col: 1, end_col: 1, empty: true });
        };
        let (start_row, start_col, end_row, end_col) = parse_a1_range(region)?;
        let end_row = end_row.unwrap_or(start_row);
        let end_col = end_col.unwrap_or(start_col);
        if end_row > sheet.row_count || end_col > sheet.column_count {
            return Err(ExternalTableError::invalid("Feishu Sheets used range exceeds the workbook grid dimensions"));
        }
        let width = end_col.saturating_sub(start_col).saturating_add(1);
        if width > MAX_COLUMNS {
            return Err(ExternalTableError::unsupported(format!(
                "Feishu Sheets data range has {width} columns; at most {MAX_COLUMNS} are supported"
            )));
        }
        Ok(SheetBounds { start_row, end_row, start_col, end_col, empty: false })
    }

    async fn page(&self, request: ReadPageRequest) -> Result<PageSnapshot, ExternalTableError> {
        let limit = request.bounded_limit(MAX_PAGE_SIZE)?;
        let offset = parse_cursor(request.cursor.as_deref())?;
        let sheet_id = Self::table_sheet_id(&request.table)?;
        let structure = self.structure().await?;
        let sheet = structure
            .sheets
            .iter()
            .find(|sheet| sheet.sheet_id == sheet_id)
            .ok_or_else(|| ExternalTableError::invalid(format!("Feishu worksheet no longer exists: {sheet_id}")))?;
        let bounds = self.bounds(sheet).await?;
        let data_start_row = bounds.start_row + u32::from(self.config.has_header);
        let data_row_count = if bounds.empty || data_start_row > bounds.end_row {
            0
        } else {
            bounds.end_row.saturating_sub(data_start_row).saturating_add(1) as usize
        };
        if offset > data_row_count {
            return Err(ExternalTableError::invalid(format!("Feishu Sheets cursor is past the end: {offset}")));
        }
        let end_offset = (offset + limit).min(data_row_count);
        let mut ranges = Vec::new();
        if self.config.has_header && !bounds.empty {
            ranges.push(format!(
                "{}{}:{}{}",
                column_label(bounds.start_col),
                bounds.start_row,
                column_label(bounds.end_col),
                bounds.start_row
            ));
        }
        if end_offset > offset {
            let start_row = data_start_row + offset as u32;
            let end_row = data_start_row + end_offset as u32 - 1;
            ranges.push(format!(
                "{}{}:{}{}",
                column_label(bounds.start_col),
                start_row,
                column_label(bounds.end_col),
                end_row
            ));
        }
        let fetched = if ranges.is_empty() { Vec::new() } else { self.fetch_ranges(&sheet_id, ranges).await? };
        let width = bounds.end_col.saturating_sub(bounds.start_col).saturating_add(1) as usize;
        let (header_cells, data_cells) = if self.config.has_header {
            (fetched.first().and_then(|range| range.cells.first().cloned()), fetched.get(1))
        } else {
            (None, fetched.first())
        };
        let raw_headers = if let Some(header_cells) = header_cells {
            (0..width)
                .map(|index| header_cells.get(index).map(|cell| display_value(&cell.value)).unwrap_or_default())
                .collect::<Vec<_>>()
        } else {
            (0..width).map(|index| format!("column_{}", index + 1)).collect()
        };
        let display_headers = unique_display_names(&raw_headers);
        let mut inferred_types = vec![ExternalValueType::Unknown; width];
        let mut rows = Vec::new();
        if let Some(data_range) = data_cells {
            for (row_offset, cells) in data_range.cells.iter().enumerate() {
                let absolute_row = data_start_row + offset as u32 + row_offset as u32;
                let mut readonly_column_keys = Vec::new();
                let values = (0..width)
                    .map(|column_offset| {
                        let cell = cells.get(column_offset).cloned().unwrap_or(SheetCell {
                            value: Value::Null,
                            readonly: false,
                            incomplete: false,
                        });
                        inferred_types[column_offset] =
                            merge_value_type(inferred_types[column_offset], value_type(&cell.value));
                        if cell.readonly {
                            readonly_column_keys.push(format!("col:{}", bounds.start_col + column_offset as u32));
                        }
                        cell.value
                    })
                    .collect();
                rows.push(ExternalRow { row_key: format!("row:{absolute_row}"), values, readonly_column_keys });
            }
        }
        let columns = display_headers
            .into_iter()
            .enumerate()
            .map(|(index, display_name)| ExternalColumn {
                column_key: format!("col:{}", bounds.start_col + index as u32),
                display_name,
                value_type: inferred_types[index],
                writable: true,
            })
            .collect();
        let incomplete = fetched.iter().any(|range| range.incomplete);
        Ok(PageSnapshot {
            table: request.table,
            columns,
            rows,
            next_cursor: (end_offset < data_row_count).then(|| end_offset.to_string()),
            snapshot_token: sheets_snapshot_token(&structure.revision, &sheet_id, &self.config),
            read_state: if incomplete { ReadState::Incomplete } else { ReadState::Complete },
        })
    }
}

#[async_trait]
impl ExternalTableAdapter for FeishuSheetsAdapter {
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            can_read: true,
            can_update: true,
            insert_mode: InsertMode::Append,
            delete_mode: DeleteMode::RemoveRow,
            supports_cell_readonly: true,
            conflict_mode: ConflictMode::RevisionAndReadback,
        }
    }

    async fn test_connection(&self) -> Result<ExternalConnectionTestResult, ExternalTableError> {
        let structure = self.structure().await?;
        Ok(ExternalConnectionTestResult::success(format!(
            "Feishu Sheets connection valid: {} worksheet(s)",
            structure.sheets.len()
        )))
    }

    async fn list_tables(&self) -> Result<Vec<ExternalTableRef>, ExternalTableError> {
        Ok(self.structure().await?.sheets.iter().map(Self::table_ref).collect())
    }

    async fn describe_table(&self, table: &ExternalTableRef) -> Result<ExternalTableSchema, ExternalTableError> {
        let page = self.page(ReadPageRequest { table: table.clone(), cursor: None, limit: 1 }).await?;
        let has_explicit_range = self.config.data_range.as_deref().is_some_and(|value| !value.trim().is_empty());
        let mut capabilities = self.capabilities();
        let structural_mutations_safe = if has_explicit_range {
            let sheet_id = Self::table_sheet_id(table)?;
            let structure = self.structure().await?;
            let sheet =
                structure.sheets.iter().find(|sheet| sheet.sheet_id == sheet_id).ok_or_else(|| {
                    ExternalTableError::invalid(format!("Feishu worksheet no longer exists: {sheet_id}"))
                })?;
            let bounds = self.bounds(sheet).await?;
            self.detected_bounds(sheet).await.ok().is_some_and(|detected| bounds.contains(detected))
        } else {
            false
        };
        if !structural_mutations_safe {
            capabilities.insert_mode = InsertMode::Unsupported;
            capabilities.delete_mode = DeleteMode::Unsupported;
        }
        let writable = page.read_state == ReadState::Complete && has_explicit_range;
        let readonly_reason = if page.read_state != ReadState::Complete {
            Some("Feishu sheet metadata/read response is incomplete".to_string())
        } else if !has_explicit_range {
            Some("Configure an explicit Feishu Sheets data range before editing".to_string())
        } else {
            None
        };
        Ok(ExternalTableSchema { table: table.clone(), columns: page.columns, capabilities, writable, readonly_reason })
    }

    async fn read_page(&self, request: ReadPageRequest) -> Result<PageSnapshot, ExternalTableError> {
        self.page(request).await
    }

    async fn apply_changes(&self, request: ApplyChangesRequest) -> Result<ApplyChangesResult, ExternalTableError> {
        request.validate()?;
        if self.config.data_range.as_deref().is_none_or(|value| value.trim().is_empty()) {
            return Err(ExternalTableError::unsupported(
                "Configure an explicit Feishu Sheets data range before editing",
            ));
        }
        let _guard = self.write_lock.lock().await;
        apply_sheet_changes(self, request).await
    }
}

async fn apply_sheet_changes(
    adapter: &FeishuSheetsAdapter,
    request: ApplyChangesRequest,
) -> Result<ApplyChangesResult, ExternalTableError> {
    let sheet_id = FeishuSheetsAdapter::table_sheet_id(&request.table)?;
    let structure = adapter.structure().await?;
    let sheet = structure
        .sheets
        .iter()
        .find(|sheet| sheet.sheet_id == sheet_id)
        .ok_or_else(|| ExternalTableError::invalid(format!("Feishu worksheet no longer exists: {sheet_id}")))?;
    let bounds = adapter.bounds(sheet).await?;
    let current_snapshot = sheets_snapshot_token(&structure.revision, &sheet_id, &adapter.config);
    let expected_source = sheets_source_fingerprint(&sheet_id, &adapter.config);
    if sheets_snapshot_source(&request.snapshot_token) != Some(expected_source.as_str()) {
        return Ok(ApplyChangesResult {
            operation_results: request
                .operations
                .iter()
                .map(|operation| {
                    OperationResult::new(operation.operation_id(), OperationOutcome::Conflict)
                        .message("Feishu Sheets source or range changed; reload before saving")
                })
                .collect(),
            new_snapshot_token: None,
            reload_required: true,
            save_blocked: true,
        });
    }
    let has_structural_operations = request
        .operations
        .iter()
        .any(|operation| matches!(operation, ExternalOperation::Insert { .. } | ExternalOperation::Delete { .. }));
    let detected_bounds = if has_structural_operations { adapter.detected_bounds(sheet).await.ok() } else { None };
    let structural_mutations_safe =
        !has_structural_operations || detected_bounds.is_some_and(|detected| bounds.contains(detected));
    let data_start_row = bounds.start_row + u32::from(adapter.config.has_header);
    let detected_data_row_count = detected_bounds
        .filter(|detected| !detected.empty && detected.end_row >= data_start_row)
        .map(|detected| detected.end_row.saturating_sub(data_start_row).saturating_add(1) as usize)
        .unwrap_or(0);
    let mut results = vec![None; request.operations.len()];
    let mut updates = Vec::new();
    let mut deletes = Vec::new();
    let mut inserts = Vec::new();
    let mut update_ranges = Vec::new();
    let mut update_old_values = Vec::new();
    let mut seen_delete_rows = HashSet::new();
    let mut insert_readbacks = Vec::new();

    for (index, operation) in request.operations.iter().enumerate() {
        match operation {
            ExternalOperation::Update { operation_id, row_key, column_key, old_value, new_value } => {
                let row = match parse_remote_key(row_key, "row:") {
                    Ok(value) => value,
                    Err(error) => {
                        results[index] = Some(
                            OperationResult::new(operation_id, OperationOutcome::Rejected).message(error.to_string()),
                        );
                        continue;
                    }
                };
                let column = match parse_remote_key(column_key, "col:") {
                    Ok(value) => value,
                    Err(error) => {
                        results[index] = Some(
                            OperationResult::new(operation_id, OperationOutcome::Rejected).message(error.to_string()),
                        );
                        continue;
                    }
                };
                if row < data_start_row || row > bounds.end_row || column < bounds.start_col || column > bounds.end_col
                {
                    results[index] = Some(
                        OperationResult::new(operation_id, OperationOutcome::Rejected)
                            .message("Feishu Sheets cell key is outside the selected data range"),
                    );
                    continue;
                }
                if let Err(error) = validate_sheet_value(new_value) {
                    results[index] =
                        Some(OperationResult::new(operation_id, OperationOutcome::Rejected).message(error.to_string()));
                    continue;
                }
                update_ranges.push(format!("{}{}", column_label(column), row));
                update_old_values.push((index, old_value.clone()));
                updates.push((index, row, column, new_value.clone()));
            }
            ExternalOperation::Delete { operation_id, row_key } => {
                let row = match parse_remote_key(row_key, "row:") {
                    Ok(value) => value,
                    Err(error) => {
                        results[index] = Some(
                            OperationResult::new(operation_id, OperationOutcome::Rejected).message(error.to_string()),
                        );
                        continue;
                    }
                };
                if row < data_start_row || row > bounds.end_row {
                    results[index] = Some(
                        OperationResult::new(operation_id, OperationOutcome::Rejected)
                            .message("Feishu Sheets row key is outside the data range or points to the header"),
                    );
                } else if !structural_mutations_safe {
                    results[index] = Some(
                        OperationResult::new(operation_id, OperationOutcome::Rejected).message(
                            "Feishu Sheets row deletion is disabled because the configured data range does not contain all detected used cells",
                        ),
                    );
                } else if detected_bounds.is_none_or(|detected| detected.empty || row > detected.end_row) {
                    results[index] = Some(
                        OperationResult::new(operation_id, OperationOutcome::Rejected)
                            .message("Feishu Sheets row key is outside the detected used data rows"),
                    );
                } else if !seen_delete_rows.insert(row) {
                    results[index] = Some(
                        OperationResult::new(operation_id, OperationOutcome::Rejected)
                            .message("Feishu Sheets row is already scheduled for deletion"),
                    );
                } else if request.snapshot_token != current_snapshot {
                    results[index] = Some(
                        OperationResult::new(operation_id, OperationOutcome::Conflict)
                            .message("Worksheet revision changed; delete requires reload"),
                    );
                } else {
                    deletes.push((index, row));
                }
            }
            ExternalOperation::Insert { operation_id, values } => {
                if !structural_mutations_safe {
                    results[index] = Some(
                        OperationResult::new(operation_id, OperationOutcome::Rejected).message(
                            "Feishu Sheets append is disabled because the configured data range does not contain all detected used cells",
                        ),
                    );
                    continue;
                }
                if request.snapshot_token != current_snapshot {
                    results[index] = Some(
                        OperationResult::new(operation_id, OperationOutcome::Conflict)
                            .message("Worksheet revision changed; append requires reload"),
                    );
                    continue;
                }
                match prepare_insert_row(values, bounds) {
                    Ok(row) => inserts.push((index, row)),
                    Err(error) => {
                        results[index] = Some(
                            OperationResult::new(operation_id, OperationOutcome::Rejected).message(error.to_string()),
                        );
                    }
                }
            }
        }
    }

    if structural_mutations_safe {
        let capacity = if data_start_row > bounds.end_row {
            0
        } else {
            bounds.end_row.saturating_sub(data_start_row).saturating_add(1) as usize
        };
        let remaining_rows = detected_data_row_count.saturating_sub(deletes.len());
        let available_rows = capacity.saturating_sub(remaining_rows);
        if inserts.len() > available_rows {
            for (operation_index, _) in inserts.split_off(available_rows) {
                results[operation_index] = Some(
                    OperationResult::new(
                        request.operations[operation_index].operation_id(),
                        OperationOutcome::Rejected,
                    )
                    .message("Feishu Sheets append would exceed the configured data range"),
                );
            }
        }
    }

    if !update_ranges.is_empty() {
        let current_values = adapter.fetch_ranges(&sheet_id, update_ranges).await?;
        for (position, (operation_index, expected_old)) in update_old_values.iter().enumerate() {
            let current = current_values
                .get(position)
                .and_then(|range| range.cells.first())
                .and_then(|row| row.first())
                .cloned()
                .unwrap_or(SheetCell { value: Value::Null, readonly: false, incomplete: false });
            if current.incomplete {
                results[*operation_index] = Some(
                    OperationResult::new(
                        request.operations[*operation_index].operation_id(),
                        OperationOutcome::Rejected,
                    )
                    .message("Feishu cell preflight response is incomplete"),
                );
            } else if current.readonly {
                results[*operation_index] = Some(
                    OperationResult::new(
                        request.operations[*operation_index].operation_id(),
                        OperationOutcome::Rejected,
                    )
                    .message("Formula and merged non-anchor cells are read-only"),
                );
            } else if !sheet_values_equal(&current.value, expected_old) {
                results[*operation_index] = Some(
                    OperationResult::new(
                        request.operations[*operation_index].operation_id(),
                        OperationOutcome::Conflict,
                    )
                    .message("Feishu cell value changed after it was read"),
                );
            }
        }
        updates.retain(|(index, _, _, _)| results[*index].is_none());
    }

    let mut stop_dispatch = false;
    if !updates.is_empty() {
        for chunk in updates.chunks(MAX_BATCH_OPERATIONS) {
            let operations = chunk
                .iter()
                .map(|(_, row, column, value)| {
                    json!({
                        "tool_name": "set_cell_range",
                        "input": {
                            "excel_id": adapter.config.spreadsheet_token,
                            "sheet_id": sheet_id,
                            "range": format!("{}{}", column_label(*column), row),
                            "cells": [[{ "value": value }]]
                        }
                    })
                })
                .collect::<Vec<_>>();
            match adapter
                .client
                .invoke_sheet_tool(
                    &adapter.config.spreadsheet_token,
                    "batch_update",
                    json!({
                        "excel_id": adapter.config.spreadsheet_token,
                        "operations": operations,
                        "continue_on_error": false
                    }),
                    true,
                )
                .await
            {
                Ok(output) => {
                    let failed = batch_failure_indexes(&output);
                    let first_failed = failed.iter().copied().min();
                    for (batch_index, (operation_index, _, _, _)) in chunk.iter().enumerate() {
                        let outcome = match first_failed {
                            Some(failed) if batch_index < failed => OperationOutcome::Applied,
                            Some(failed) if batch_index == failed => OperationOutcome::Rejected,
                            Some(_) => OperationOutcome::NotAttempted,
                            None => OperationOutcome::Applied,
                        };
                        results[*operation_index] =
                            Some(OperationResult::new(request.operations[*operation_index].operation_id(), outcome));
                    }
                    stop_dispatch = !failed.is_empty();
                }
                Err(error) if error.kind == FeishuRequestErrorKind::Unknown => {
                    for (operation_index, _, _, _) in chunk {
                        results[*operation_index] = Some(
                            OperationResult::new(
                                request.operations[*operation_index].operation_id(),
                                OperationOutcome::Unknown,
                            )
                            .message(error.message.clone()),
                        );
                    }
                    stop_dispatch = true;
                }
                Err(error) => {
                    let failed_index = batch_failure_index_from_message(&error.message);
                    for (batch_index, (operation_index, _, _, _)) in chunk.iter().enumerate() {
                        let (outcome, message) = match failed_index {
                            Some(failed) if batch_index < failed => (OperationOutcome::Applied, None),
                            Some(failed) if batch_index == failed => {
                                (OperationOutcome::Rejected, Some(error.message.clone()))
                            }
                            Some(_) => (OperationOutcome::NotAttempted, None),
                            None => (OperationOutcome::Rejected, Some(error.message.clone())),
                        };
                        let mut result =
                            OperationResult::new(request.operations[*operation_index].operation_id(), outcome);
                        result.message = message;
                        results[*operation_index] = Some(result);
                    }
                    stop_dispatch = true;
                }
            }
            if stop_dispatch {
                break;
            }
        }
    }

    deletes.sort_by_key(|(_, row)| std::cmp::Reverse(*row));
    if !stop_dispatch {
        for (position, (operation_index, row)) in deletes.iter().enumerate() {
            let expected_successor =
                match adapter.fetch_row_values(&sheet_id, bounds, row.saturating_add(1), sheet.row_count).await {
                    Ok(values) => values,
                    Err(error) => {
                        results[*operation_index] = Some(
                            OperationResult::new(
                                request.operations[*operation_index].operation_id(),
                                OperationOutcome::Rejected,
                            )
                            .message(format!("Feishu Sheets delete preflight failed: {error}")),
                        );
                        for (later_index, _) in deletes.iter().skip(position + 1) {
                            results[*later_index] = Some(OperationResult::new(
                                request.operations[*later_index].operation_id(),
                                OperationOutcome::NotAttempted,
                            ));
                        }
                        stop_dispatch = true;
                        break;
                    }
                };
            let input = json!({
                "excel_id": adapter.config.spreadsheet_token,
                "operation": "delete",
                "sheet_id": sheet_id,
                "range": format!("{row}:{row}")
            });
            match adapter
                .client
                .invoke_sheet_tool(&adapter.config.spreadsheet_token, "modify_sheet_structure", input, true)
                .await
            {
                Ok(_) => {
                    results[*operation_index] =
                        Some(match adapter.fetch_row_values(&sheet_id, bounds, *row, sheet.row_count).await {
                            Ok(actual) if sheet_rows_equal(&actual, &expected_successor) => OperationResult::new(
                                request.operations[*operation_index].operation_id(),
                                OperationOutcome::Applied,
                            ),
                            Ok(_) => OperationResult::new(
                                request.operations[*operation_index].operation_id(),
                                OperationOutcome::Unknown,
                            )
                            .message(
                                "Feishu Sheets acknowledged the row deletion, but readback differs; reload required",
                            ),
                            Err(error) => OperationResult::new(
                                request.operations[*operation_index].operation_id(),
                                OperationOutcome::Unknown,
                            )
                            .message(format!(
                                "Feishu Sheets acknowledged the row deletion, but readback failed: {error}"
                            )),
                        });
                    if results[*operation_index]
                        .as_ref()
                        .is_some_and(|result| result.outcome == OperationOutcome::Unknown)
                    {
                        for (later_index, _) in deletes.iter().skip(position + 1) {
                            results[*later_index] = Some(OperationResult::new(
                                request.operations[*later_index].operation_id(),
                                OperationOutcome::NotAttempted,
                            ));
                        }
                        stop_dispatch = true;
                        break;
                    }
                }
                Err(error) => {
                    let outcome = if error.kind == FeishuRequestErrorKind::Unknown {
                        OperationOutcome::Unknown
                    } else {
                        OperationOutcome::Rejected
                    };
                    results[*operation_index] = Some(
                        OperationResult::new(request.operations[*operation_index].operation_id(), outcome)
                            .message(error.message),
                    );
                    for (later_index, _) in deletes.iter().skip(position + 1) {
                        results[*later_index] = Some(OperationResult::new(
                            request.operations[*later_index].operation_id(),
                            OperationOutcome::NotAttempted,
                        ));
                    }
                    stop_dispatch = true;
                    break;
                }
            }
        }
    }

    if !stop_dispatch {
        let deleted_count = deletes
            .iter()
            .filter(|(index, _)| {
                results[*index].as_ref().is_some_and(|result| result.outcome == OperationOutcome::Applied)
            })
            .count() as u32;
        let remaining_rows = detected_data_row_count.saturating_sub(deleted_count as usize) as u32;
        let mut append_row = data_start_row.saturating_add(remaining_rows);
        for (position, (operation_index, values)) in inserts.iter().enumerate() {
            let input = json!({
                "excel_id": adapter.config.spreadsheet_token,
                "sheet_id": sheet_id,
                "range": format!("{}{}", column_label(bounds.start_col), append_row),
                "cells": [values.iter().map(|value| json!({ "value": value })).collect::<Vec<_>>()]
            });
            match adapter
                .client
                .invoke_sheet_tool(&adapter.config.spreadsheet_token, "set_cell_range", input, true)
                .await
            {
                Ok(_) => {
                    insert_readbacks.push((*operation_index, append_row, values.clone()));
                    results[*operation_index] = Some(
                        OperationResult::new(
                            request.operations[*operation_index].operation_id(),
                            OperationOutcome::Applied,
                        )
                        .message(format!("Appended Feishu row {append_row}")),
                    );
                    append_row += 1;
                }
                Err(error) => {
                    let outcome = if error.kind == FeishuRequestErrorKind::Unknown {
                        OperationOutcome::Unknown
                    } else {
                        OperationOutcome::Rejected
                    };
                    results[*operation_index] = Some(
                        OperationResult::new(request.operations[*operation_index].operation_id(), outcome)
                            .message(error.message),
                    );
                    for (later_index, _) in inserts.iter().skip(position + 1) {
                        results[*later_index] = Some(OperationResult::new(
                            request.operations[*later_index].operation_id(),
                            OperationOutcome::NotAttempted,
                        ));
                    }
                    stop_dispatch = true;
                    break;
                }
            }
        }
    }

    if stop_dispatch {
        for (index, result) in results.iter_mut().enumerate() {
            if result.is_none() {
                *result = Some(OperationResult::new(
                    request.operations[index].operation_id(),
                    OperationOutcome::NotAttempted,
                ));
            }
        }
    }
    let mut readbacks = updates
        .iter()
        .filter(|(index, _, _, _)| {
            results[*index].as_ref().is_some_and(|result| result.outcome == OperationOutcome::Applied)
        })
        .map(|(index, row, column, value)| (*index, format!("{}{}", column_label(*column), row), vec![value.clone()]))
        .collect::<Vec<_>>();
    readbacks.extend(insert_readbacks.into_iter().map(|(index, row, values)| {
        (index, format!("{}{}:{}{}", column_label(bounds.start_col), row, column_label(bounds.end_col), row), values)
    }));
    let mut operation_results = results
        .into_iter()
        .enumerate()
        .map(|(index, result)| {
            result.unwrap_or_else(|| {
                OperationResult::new(request.operations[index].operation_id(), OperationOutcome::Rejected)
                    .message("Feishu Sheets operation could not be prepared")
            })
        })
        .collect::<Vec<_>>();
    let has_applied = operation_results.iter().any(|result| result.outcome == OperationOutcome::Applied);
    let new_snapshot_token = if has_applied {
        match adapter.structure().await {
            Ok(structure) => {
                if !readbacks.is_empty() {
                    let ranges = readbacks.iter().map(|(_, range, _)| range.clone()).collect();
                    match adapter.fetch_ranges(&sheet_id, ranges).await {
                        Ok(actual) => {
                            for (position, (operation_index, _, expected)) in readbacks.iter().enumerate() {
                                let values: Vec<Value> = actual
                                    .get(position)
                                    .and_then(|range| range.cells.first())
                                    .map(|row| row.iter().take(expected.len()).map(|cell| cell.value.clone()).collect())
                                    .unwrap_or_default();
                                if !sheet_rows_equal(&values, expected) {
                                    mark_result_unknown(
                                        &mut operation_results[*operation_index],
                                        "Feishu Sheets acknowledged the operation, but readback differs; reload required",
                                    );
                                }
                            }
                        }
                        Err(error) => annotate_applied_readback_failure(&mut operation_results, &error.to_string()),
                    }
                }
                Some(sheets_snapshot_token(&structure.revision, &sheet_id, &adapter.config))
            }
            Err(error) => {
                annotate_applied_readback_failure(&mut operation_results, &error.to_string());
                None
            }
        }
    } else {
        let has_unresolved = operation_results
            .iter()
            .any(|result| matches!(result.outcome, OperationOutcome::Conflict | OperationOutcome::Unknown));
        (!has_unresolved).then_some(current_snapshot)
    };
    let reload_required = operation_results.iter().any(|result| {
        matches!(result.outcome, OperationOutcome::Applied | OperationOutcome::Conflict | OperationOutcome::Unknown)
    });
    let save_blocked = operation_results
        .iter()
        .any(|result| matches!(result.outcome, OperationOutcome::Conflict | OperationOutcome::Unknown));
    Ok(ApplyChangesResult { operation_results, new_snapshot_token, reload_required, save_blocked })
}

fn annotate_applied_readback_failure(results: &mut [OperationResult], message: &str) {
    for result in results.iter_mut().filter(|result| result.outcome == OperationOutcome::Applied) {
        mark_result_unknown(
            result,
            &format!("Feishu Sheets acknowledged the operation, but readback failed: {message}"),
        );
    }
}

fn mark_result_unknown(result: &mut OperationResult, message: &str) {
    result.outcome = OperationOutcome::Unknown;
    append_result_message(result, message);
}

fn append_result_message(result: &mut OperationResult, message: &str) {
    result.message = Some(match result.message.take() {
        Some(existing) => format!("{existing}; {message}"),
        None => message.to_string(),
    });
}

fn sheet_rows_equal(left: &[Value], right: &[Value]) -> bool {
    left.len() == right.len() && left.iter().zip(right).all(|(left, right)| sheet_values_equal(left, right))
}

fn sheet_values_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => json_numbers_equal(left, right),
        _ => left == right,
    }
}

fn parse_structure(output: &Value, configured_sheet_id: Option<&str>) -> Result<WorkbookStructure, ExternalTableError> {
    let revision = output
        .get("revision")
        .map(value_as_stable_string)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ExternalTableError::unsupported("Feishu workbook structure did not return a revision"))?;
    let raw_sheets = output
        .get("sheets")
        .and_then(Value::as_array)
        .ok_or_else(|| ExternalTableError::invalid("Feishu workbook structure is missing sheets"))?;
    let mut sheets = Vec::new();
    for sheet in raw_sheets {
        let Some(sheet_id) = sheet.get("sheet_id").and_then(Value::as_str).filter(|value| !value.is_empty()) else {
            continue;
        };
        if configured_sheet_id.is_some_and(|configured| configured != sheet_id) {
            continue;
        }
        let title = sheet
            .get("sheet_name")
            .or_else(|| sheet.get("title"))
            .and_then(Value::as_str)
            .unwrap_or(sheet_id)
            .to_string();
        let row_count = u32_value(sheet.get("row_count")).filter(|value| *value > 0).ok_or_else(|| {
            ExternalTableError::invalid(format!("Feishu worksheet '{title}' is missing a valid row_count"))
        })?;
        let column_count = u32_value(sheet.get("column_count")).filter(|value| *value > 0).ok_or_else(|| {
            ExternalTableError::invalid(format!("Feishu worksheet '{title}' is missing a valid column_count"))
        })?;
        sheets.push(WorkbookSheet { sheet_id: sheet_id.to_string(), title, row_count, column_count });
    }
    if sheets.is_empty() {
        return Err(ExternalTableError::invalid("No matching Feishu worksheets were returned"));
    }
    Ok(WorkbookStructure { revision, sheets })
}

fn parse_cell_ranges(output: &Value) -> Result<Vec<CellRange>, ExternalTableError> {
    let ranges = output
        .get("ranges")
        .and_then(Value::as_array)
        .ok_or_else(|| ExternalTableError::invalid("Feishu get_cell_ranges output is missing ranges"))?;
    Ok(ranges
        .iter()
        .map(|range| {
            let rows = range
                .get("cells")
                .or_else(|| range.get("values"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let cells = rows
                .iter()
                .map(|row| row.as_array().cloned().unwrap_or_default().iter().map(parse_sheet_cell).collect())
                .collect::<Vec<Vec<SheetCell>>>();
            let incomplete = range.get("truncated").and_then(Value::as_bool).unwrap_or(false)
                || output.get("has_more").and_then(Value::as_bool).unwrap_or(false)
                || cells.iter().flatten().any(|cell| cell.incomplete);
            CellRange { cells, incomplete }
        })
        .collect())
}

fn parse_sheet_cell(value: &Value) -> SheetCell {
    let Some(object) = value.as_object() else {
        let readonly = value.as_str().is_some_and(|value| value.starts_with('='));
        return SheetCell { value: value.clone(), readonly, incomplete: false };
    };
    let formula = object.get("formula").and_then(Value::as_str).unwrap_or_default();
    let merged_non_anchor = object
        .get("merged_non_anchor")
        .or_else(|| object.get("is_merged_non_anchor"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let incomplete = object
        .get("isRowTruncated")
        .or_else(|| object.get("is_row_truncated"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || object
            .get("isColTruncated")
            .or_else(|| object.get("is_col_truncated"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
    let cell_value = object
        .get("value")
        .cloned()
        .or_else(|| (!formula.is_empty()).then(|| Value::String(formula.to_string())))
        .unwrap_or(Value::Null);
    SheetCell { value: cell_value, readonly: !formula.is_empty() || merged_non_anchor, incomplete }
}

fn resolve_bounds(sheet: &WorkbookSheet, configured: Option<&str>) -> Result<SheetBounds, ExternalTableError> {
    if let Some(value) = configured.map(str::trim).filter(|value| !value.is_empty()) {
        let (start_row, start_col, end_row, end_col) = parse_a1_range(value)?;
        let end_row = end_row.unwrap_or(sheet.row_count).max(start_row);
        let end_col = end_col.unwrap_or(sheet.column_count).max(start_col);
        if start_row > sheet.row_count
            || start_col > sheet.column_count
            || end_row > sheet.row_count
            || end_col > sheet.column_count
        {
            return Err(ExternalTableError::invalid(
                "Feishu Sheets configured data range exceeds the workbook grid dimensions",
            ));
        }
        let width = end_col.saturating_sub(start_col).saturating_add(1);
        if width > MAX_COLUMNS {
            return Err(ExternalTableError::unsupported(format!(
                "Feishu Sheets data range has {width} columns; at most {MAX_COLUMNS} are supported"
            )));
        }
        return Ok(SheetBounds { start_row, start_col, end_row, end_col, empty: false });
    }
    Err(ExternalTableError::invalid("Feishu Sheets data bounds were not resolved"))
}

fn parse_a1_range(value: &str) -> Result<(u32, u32, Option<u32>, Option<u32>), ExternalTableError> {
    let range = value.rsplit_once('!').map(|(_, range)| range).unwrap_or(value);
    let mut cells = range.split(':');
    let (start_row, start_col) = parse_a1_cell(cells.next().unwrap_or_default())?;
    let end = cells.next().map(parse_a1_cell).transpose()?;
    if cells.next().is_some() {
        return Err(ExternalTableError::invalid(format!("Invalid Feishu Sheets data range: {value}")));
    }
    if end.is_some_and(|(row, column)| row < start_row || column < start_col) {
        return Err(ExternalTableError::invalid(format!("Feishu Sheets range end precedes start: {value}")));
    }
    Ok((start_row, start_col, end.map(|value| value.0), end.map(|value| value.1)))
}

fn parse_a1_cell(value: &str) -> Result<(u32, u32), ExternalTableError> {
    let value = value.trim().replace('$', "");
    let split = value
        .find(|character: char| character.is_ascii_digit())
        .ok_or_else(|| ExternalTableError::invalid(format!("Invalid A1 cell reference: {value}")))?;
    let (column, row) = value.split_at(split);
    let row = row.parse::<u32>().map_err(|_| ExternalTableError::invalid(format!("Invalid A1 row: {row}")))?;
    let mut column_number = 0_u32;
    for character in column.chars() {
        if !character.is_ascii_alphabetic() {
            return Err(ExternalTableError::invalid(format!("Invalid A1 column: {column}")));
        }
        column_number = column_number
            .checked_mul(26)
            .and_then(|value| value.checked_add(character.to_ascii_uppercase() as u32 - 'A' as u32 + 1))
            .ok_or_else(|| ExternalTableError::invalid(format!("A1 column is too large: {column}")))?;
    }
    if row == 0 || column_number == 0 {
        return Err(ExternalTableError::invalid(format!("A1 coordinates start at 1: {value}")));
    }
    Ok((row, column_number))
}

fn prepare_insert_row(values: &[ExternalCellInput], bounds: SheetBounds) -> Result<Vec<Value>, ExternalTableError> {
    let width = bounds.end_col.saturating_sub(bounds.start_col).saturating_add(1) as usize;
    let mut row = vec![Value::Null; width];
    let mut seen = HashSet::new();
    for cell in values {
        let column = parse_remote_key(&cell.column_key, "col:")?;
        if column < bounds.start_col || column > bounds.end_col {
            return Err(ExternalTableError::invalid(format!(
                "Feishu Sheets column is outside the selected data range: {}",
                cell.column_key
            )));
        }
        let index = (column - bounds.start_col) as usize;
        if !seen.insert(index) {
            return Err(ExternalTableError::invalid(format!(
                "Feishu Sheets insert contains duplicate column: {}",
                cell.column_key
            )));
        }
        validate_sheet_value(&cell.value)?;
        row[index] = cell.value.clone();
    }
    Ok(row)
}

fn validate_sheet_value(value: &Value) -> Result<(), ExternalTableError> {
    if matches!(value, Value::Array(_) | Value::Object(_)) {
        return Err(ExternalTableError::invalid("Feishu Sheets cells accept only scalar or null values"));
    }
    if value.as_number().is_some_and(|number| json_number_as_safe_f64(number).is_none()) {
        return Err(ExternalTableError::invalid(
            "Feishu Sheets integer values outside the JavaScript-safe range must be stored as text",
        ));
    }
    Ok(())
}

fn parse_remote_key(key: &str, prefix: &str) -> Result<u32, ExternalTableError> {
    let value = parse_index_key(key, prefix)?;
    if value == 0 || value > u32::MAX as usize {
        return Err(ExternalTableError::invalid(format!("Invalid stable key: {key}")));
    }
    Ok(value as u32)
}

fn parse_cursor(cursor: Option<&str>) -> Result<usize, ExternalTableError> {
    match cursor {
        None | Some("") => Ok(0),
        Some(cursor) => cursor
            .parse::<usize>()
            .map_err(|_| ExternalTableError::invalid(format!("Invalid Feishu Sheets cursor: {cursor}"))),
    }
}

fn column_label(mut column: u32) -> String {
    let mut label = String::new();
    while column > 0 {
        column -= 1;
        label.insert(0, char::from_u32('A' as u32 + column % 26).unwrap());
        column /= 26;
    }
    label
}

fn snapshot_digest(namespace: &str, parts: &[&str]) -> String {
    scoped_snapshot_token(namespace, parts)
        .rsplit(':')
        .next()
        .expect("scoped snapshot tokens contain a digest")
        .to_string()
}

fn sheets_source_fingerprint(sheet_id: &str, config: &FeishuSheetsExternalConfig) -> String {
    let data_range = config.data_range.as_deref().map(str::trim).filter(|value| !value.is_empty()).unwrap_or("<used>");
    let has_header = if config.has_header { "header" } else { "no-header" };
    snapshot_digest("feishu-sheets-source", &[&config.spreadsheet_token, sheet_id, data_range, has_header])
}

fn sheets_snapshot_token(revision: &str, sheet_id: &str, config: &FeishuSheetsExternalConfig) -> String {
    let source = sheets_source_fingerprint(sheet_id, config);
    let state = snapshot_digest("feishu-sheets-state", &[revision]);
    format!("feishu-sheets:v2:source:{source}:state:{state}")
}

fn sheets_snapshot_source(snapshot: &str) -> Option<&str> {
    let tail = snapshot.strip_prefix("feishu-sheets:v2:source:")?;
    let (source, state) = tail.split_once(":state:")?;
    (!source.is_empty() && !state.is_empty()).then_some(source)
}

fn display_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        value => value.to_string(),
    }
}

fn value_type(value: &Value) -> ExternalValueType {
    match value {
        Value::Null => ExternalValueType::Unknown,
        Value::String(_) => ExternalValueType::String,
        Value::Number(_) => ExternalValueType::Number,
        Value::Bool(_) => ExternalValueType::Boolean,
        Value::Array(_) | Value::Object(_) => ExternalValueType::Json,
    }
}

fn merge_value_type(current: ExternalValueType, next: ExternalValueType) -> ExternalValueType {
    if current == ExternalValueType::Unknown {
        next
    } else if next == ExternalValueType::Unknown || current == next {
        current
    } else {
        ExternalValueType::String
    }
}

fn value_as_stable_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => String::new(),
        value => value.to_string(),
    }
}

fn u32_value(value: Option<&Value>) -> Option<u32> {
    value.and_then(|value| value.as_u64().and_then(|value| u32::try_from(value).ok()))
}

fn batch_failure_indexes(output: &Value) -> HashSet<usize> {
    output
        .get("failures")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|failure| failure.get("index").and_then(Value::as_u64).map(|index| index as usize))
        .collect()
}

fn batch_failure_index_from_message(message: &str) -> Option<usize> {
    fn find_index(value: &Value) -> Option<usize> {
        if let Some(index) = value
            .get("failures")
            .and_then(Value::as_array)
            .and_then(|failures| failures.first())
            .and_then(|failure| failure.get("index"))
            .and_then(Value::as_u64)
        {
            return usize::try_from(index).ok();
        }
        match value {
            Value::String(value) => serde_json::from_str::<Value>(value).ok().as_ref().and_then(find_index),
            Value::Array(values) => values.iter().find_map(find_index),
            Value::Object(values) => values.values().find_map(find_index),
            _ => None,
        }
    }

    let json_start = message.find('{')?;
    serde_json::from_str::<Value>(&message[json_start..]).ok().as_ref().and_then(find_index)
}

#[cfg(test)]
mod tests {
    use super::super::feishu::test_support::{serve, MockReply};
    use super::*;

    fn token_reply() -> MockReply {
        MockReply::Json(
            json!({ "code": 0, "msg": "ok", "tenant_access_token": "tenant-token", "expire": 7200 }).to_string(),
        )
    }

    fn tool_reply(output: Value) -> MockReply {
        MockReply::Json(json!({ "code": 0, "msg": "ok", "data": { "output": output.to_string() } }).to_string())
    }

    fn adapter(client: FeishuClient) -> FeishuSheetsAdapter {
        FeishuSheetsAdapter::from_client(
            client,
            FeishuSheetsExternalConfig {
                spreadsheet_token: "spreadsheet".to_string(),
                sheet_id: Some("sh1".to_string()),
                data_range: Some("A1:B3".to_string()),
                has_header: true,
            },
        )
        .unwrap()
    }

    fn adapter_without_range(client: FeishuClient) -> FeishuSheetsAdapter {
        FeishuSheetsAdapter::from_client(
            client,
            FeishuSheetsExternalConfig {
                spreadsheet_token: "spreadsheet".to_string(),
                sheet_id: Some("sh1".to_string()),
                data_range: None,
                has_header: true,
            },
        )
        .unwrap()
    }

    #[test]
    fn structure_without_revision_or_grid_dimensions_is_rejected() {
        assert!(parse_structure(
            &json!({ "sheets": [{ "sheet_id": "sh1", "title": "Sheet1", "row_count": 3, "column_count": 2 }] }),
            None,
        )
        .unwrap_err()
        .to_string()
        .contains("revision"));
        assert!(
            parse_structure(&json!({ "revision": 1, "sheets": [{ "sheet_id": "sh1", "title": "Sheet1" }] }), None,)
                .unwrap_err()
                .to_string()
                .contains("row_count")
        );
    }

    #[test]
    fn sheets_snapshot_is_bound_to_spreadsheet_and_header_config() {
        let first = FeishuSheetsExternalConfig {
            spreadsheet_token: "spreadsheet-a".to_string(),
            sheet_id: Some("sh1".to_string()),
            data_range: Some("A1:B3".to_string()),
            has_header: true,
        };
        let other_spreadsheet =
            FeishuSheetsExternalConfig { spreadsheet_token: "spreadsheet-b".to_string(), ..first.clone() };
        let no_header = FeishuSheetsExternalConfig { has_header: false, ..first.clone() };

        assert_ne!(sheets_snapshot_token("7", "sh1", &first), sheets_snapshot_token("7", "sh1", &other_spreadsheet));
        assert_ne!(sheets_snapshot_token("7", "sh1", &first), sheets_snapshot_token("7", "sh1", &no_header));
    }

    #[tokio::test]
    async fn sheets_source_change_conflicts_before_update_preflight() {
        let structure = json!({
            "revision": 7,
            "sheets": [{ "sheet_id": "sh1", "title": "Sheet1", "row_count": 3, "column_count": 2 }]
        });
        let original_config = FeishuSheetsExternalConfig {
            spreadsheet_token: "spreadsheet-a".to_string(),
            sheet_id: Some("sh1".to_string()),
            data_range: Some("A1:B3".to_string()),
            has_header: true,
        };
        let changed_config =
            FeishuSheetsExternalConfig { spreadsheet_token: "spreadsheet-b".to_string(), ..original_config.clone() };
        let (base_url, server) = serve(vec![token_reply(), tool_reply(structure)]).await;
        let client = FeishuClient::with_base_url(base_url, "app", "secret", Duration::from_secs(5)).unwrap();
        let adapter = FeishuSheetsAdapter::from_client(client, changed_config).unwrap();

        let result = adapter
            .apply_changes(ApplyChangesRequest {
                table: ExternalTableRef { table_key: "sheet:sh1".to_string(), display_name: "Sheet1".to_string() },
                snapshot_token: sheets_snapshot_token("7", "sh1", &original_config),
                operations: vec![ExternalOperation::Update {
                    operation_id: "update".to_string(),
                    row_key: "row:2".to_string(),
                    column_key: "col:1".to_string(),
                    old_value: Value::String("Ada".to_string()),
                    new_value: Value::String("Wrong source".to_string()),
                }],
            })
            .await
            .unwrap();

        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 2, "source mismatch must stop before old-value preflight");
        assert_eq!(result.operation_results[0].outcome, OperationOutcome::Conflict);
        assert!(result.save_blocked);
    }

    #[test]
    fn sheets_numeric_comparison_preserves_large_integer_identity() {
        let first = Value::Number(9_007_199_254_740_992_u64.into());
        let second = Value::Number(9_007_199_254_740_993_u64.into());
        let safe_integer = Value::Number(42.into());
        let safe_float = Value::Number(serde_json::Number::from_f64(42.0).unwrap());

        assert!(!sheet_values_equal(&first, &second));
        assert!(sheet_values_equal(&safe_integer, &safe_float));
        assert!(validate_sheet_value(&first).is_err());
    }

    #[tokio::test]
    async fn sheets_explicit_range_append_uses_detected_end_and_rejects_overflow() {
        let structure = json!({
            "revision": 7,
            "sheets": [{ "sheet_id": "sh1", "title": "Sheet1", "row_count": 20, "column_count": 2 }]
        });
        let (base_url, server) = serve(vec![
            token_reply(),
            tool_reply(structure.clone()),
            tool_reply(json!({ "current_region": "A1:B3", "annotated_csv": "" })),
            tool_reply(json!({})),
            tool_reply(json!({
                "revision": 8,
                "sheets": [{ "sheet_id": "sh1", "title": "Sheet1", "row_count": 20, "column_count": 2 }]
            })),
            tool_reply(json!({ "ranges": [{ "cells": [[{ "value": "Lin" }, { "value": null }]] }] })),
        ])
        .await;
        let client = FeishuClient::with_base_url(base_url, "app", "secret", Duration::from_secs(5)).unwrap();
        let adapter = FeishuSheetsAdapter::from_client(
            client,
            FeishuSheetsExternalConfig {
                spreadsheet_token: "spreadsheet".to_string(),
                sheet_id: Some("sh1".to_string()),
                data_range: Some("A1:B10".to_string()),
                has_header: true,
            },
        )
        .unwrap();

        let result = adapter
            .apply_changes(ApplyChangesRequest {
                table: ExternalTableRef { table_key: "sheet:sh1".to_string(), display_name: "Sheet1".to_string() },
                snapshot_token: sheets_snapshot_token("7", "sh1", &adapter.config),
                operations: vec![ExternalOperation::Insert {
                    operation_id: "append".to_string(),
                    values: vec![ExternalCellInput {
                        column_key: "col:1".to_string(),
                        value: Value::String("Lin".to_string()),
                    }],
                }],
            })
            .await
            .unwrap();

        let requests = server.await.unwrap();
        assert_eq!(result.operation_results[0].outcome, OperationOutcome::Applied);
        assert!(requests[3].contains("\\\"range\\\":\\\"A4\\\""), "unexpected append request: {}", requests[3]);

        let full_config = FeishuSheetsExternalConfig {
            spreadsheet_token: "spreadsheet".to_string(),
            sheet_id: Some("sh1".to_string()),
            data_range: Some("A1:B3".to_string()),
            has_header: true,
        };
        let (base_url, server) = serve(vec![
            token_reply(),
            tool_reply(structure),
            tool_reply(json!({ "current_region": "A1:B3", "annotated_csv": "" })),
        ])
        .await;
        let client = FeishuClient::with_base_url(base_url, "app", "secret", Duration::from_secs(5)).unwrap();
        let full_adapter = FeishuSheetsAdapter::from_client(client, full_config).unwrap();
        let overflow = full_adapter
            .apply_changes(ApplyChangesRequest {
                table: ExternalTableRef { table_key: "sheet:sh1".to_string(), display_name: "Sheet1".to_string() },
                snapshot_token: sheets_snapshot_token("7", "sh1", &full_adapter.config),
                operations: vec![ExternalOperation::Insert { operation_id: "overflow".to_string(), values: vec![] }],
            })
            .await
            .unwrap();

        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 3, "overflow must be rejected before write dispatch");
        assert_eq!(overflow.operation_results[0].outcome, OperationOutcome::Rejected);
    }

    #[tokio::test]
    async fn sheets_without_explicit_range_is_browse_only() {
        let (base_url, server) = serve(vec![]).await;
        let client = FeishuClient::with_base_url(base_url, "app", "secret", Duration::from_secs(5)).unwrap();

        let error = adapter_without_range(client)
            .apply_changes(ApplyChangesRequest {
                table: ExternalTableRef { table_key: "sheet:sh1".to_string(), display_name: "Sheet1".to_string() },
                snapshot_token: "stale".to_string(),
                operations: vec![ExternalOperation::Insert { operation_id: "append".to_string(), values: vec![] }],
            })
            .await
            .unwrap_err();

        let requests = server.await.unwrap();
        assert!(requests.is_empty());
        assert!(error.to_string().contains("explicit Feishu Sheets data range"));
    }

    #[tokio::test]
    async fn sheets_read_decodes_tool_envelope_and_marks_formula_cells_readonly() {
        let structure = json!({
            "revision": 7,
            "sheets": [{ "sheet_id": "sh1", "title": "Sheet1", "row_count": 3, "column_count": 2 }]
        });
        let ranges = json!({
            "ranges": [
                { "cells": [[{ "value": "id" }, { "value": "total" }]] },
                { "cells": [
                [{ "value": 1 }, { "value": 2, "formula": "=A2*2" }],
                [{ "value": 2 }, { "value": 4 }]
                ] }
            ]
        });
        let (base_url, server) = serve(vec![token_reply(), tool_reply(structure), tool_reply(ranges)]).await;
        let client = FeishuClient::with_base_url(base_url, "app", "secret", Duration::from_secs(5)).unwrap();
        let adapter = adapter(client);

        let page = adapter
            .read_page(ReadPageRequest {
                table: ExternalTableRef { table_key: "sheet:sh1".to_string(), display_name: "Sheet1".to_string() },
                cursor: None,
                limit: 20,
            })
            .await
            .unwrap();

        server.await.unwrap();
        assert_eq!(page.snapshot_token, sheets_snapshot_token("7", "sh1", &adapter.config));
        assert_eq!(page.rows.len(), 2);
        assert!(page.rows[0].readonly_column_keys.contains(&"col:2".to_string()));
    }

    #[tokio::test]
    async fn sheets_empty_used_range_has_no_phantom_data_row() {
        let structure = json!({
            "revision": 7,
            "sheets": [{ "sheet_id": "sh1", "title": "Sheet1", "row_count": 200, "column_count": 20 }]
        });
        let (base_url, server) =
            serve(vec![token_reply(), tool_reply(structure), tool_reply(json!({ "annotated_csv": "" }))]).await;
        let client = FeishuClient::with_base_url(base_url, "app", "secret", Duration::from_secs(5)).unwrap();
        let adapter = adapter_without_range(client);

        let page = adapter
            .read_page(ReadPageRequest {
                table: ExternalTableRef { table_key: "sheet:sh1".to_string(), display_name: "Sheet1".to_string() },
                cursor: None,
                limit: 20,
            })
            .await
            .unwrap();

        server.await.unwrap();
        assert!(page.rows.is_empty());
        assert!(page.next_cursor.is_none());
    }

    #[tokio::test]
    async fn sheets_partial_batch_maps_applied_rejected_and_not_attempted() {
        let structure = json!({
            "revision": 7,
            "sheets": [{ "sheet_id": "sh1", "title": "Sheet1", "row_count": 3, "column_count": 2 }]
        });
        let preflight = json!({
            "ranges": [
                { "cells": [[{ "value": "Ada" }]] },
                { "cells": [[{ "value": 2 }]] }
            ]
        });
        let failure = json!({
            "code": 9001,
            "msg": "{\"error\":\"{\\\"message\\\":\\\"batch_update: 1 succeeded, 1 failed\\\",\\\"failures\\\":[{\\\"index\\\":1}]}\"}",
            "data": {}
        });
        let (base_url, server) = serve(vec![
            token_reply(),
            tool_reply(structure.clone()),
            tool_reply(json!({ "current_region": "A1:B2", "annotated_csv": "" })),
            tool_reply(preflight),
            MockReply::Json(failure.to_string()),
            tool_reply(json!({
                "revision": 8,
                "sheets": [{ "sheet_id": "sh1", "title": "Sheet1", "row_count": 3, "column_count": 2 }]
            })),
            tool_reply(json!({ "ranges": [{ "cells": [[{ "value": "A" }]] }] })),
        ])
        .await;
        let client = FeishuClient::with_base_url(base_url, "app", "secret", Duration::from_secs(5)).unwrap();
        let adapter = adapter(client);

        let result = adapter
            .apply_changes(ApplyChangesRequest {
                table: ExternalTableRef { table_key: "sheet:sh1".to_string(), display_name: "Sheet1".to_string() },
                snapshot_token: sheets_snapshot_token("7", "sh1", &adapter.config),
                operations: vec![
                    ExternalOperation::Update {
                        operation_id: "first".to_string(),
                        row_key: "row:2".to_string(),
                        column_key: "col:1".to_string(),
                        old_value: Value::String("Ada".to_string()),
                        new_value: Value::String("A".to_string()),
                    },
                    ExternalOperation::Update {
                        operation_id: "second".to_string(),
                        row_key: "row:2".to_string(),
                        column_key: "col:2".to_string(),
                        old_value: Value::Number(2.into()),
                        new_value: Value::Number(3.into()),
                    },
                    ExternalOperation::Insert { operation_id: "later".to_string(), values: vec![] },
                ],
            })
            .await
            .unwrap();

        server.await.unwrap();
        assert_eq!(result.operation_results[0].outcome, OperationOutcome::Applied);
        assert_eq!(result.operation_results[1].outcome, OperationOutcome::Rejected);
        assert_eq!(result.operation_results[2].outcome, OperationOutcome::NotAttempted);
    }

    #[tokio::test]
    async fn sheets_unknown_insert_is_not_retried_and_blocks_save() {
        let structure = json!({
            "revision": 7,
            "sheets": [{ "sheet_id": "sh1", "title": "Sheet1", "row_count": 3, "column_count": 2 }]
        });
        let (base_url, server) = serve(vec![
            token_reply(),
            tool_reply(structure),
            tool_reply(json!({ "current_region": "A1:B2", "annotated_csv": "" })),
            MockReply::DropConnection,
        ])
        .await;
        let client = FeishuClient::with_base_url(base_url, "app", "secret", Duration::from_secs(5)).unwrap();
        let adapter = adapter(client);

        let result = adapter
            .apply_changes(ApplyChangesRequest {
                table: ExternalTableRef { table_key: "sheet:sh1".to_string(), display_name: "Sheet1".to_string() },
                snapshot_token: sheets_snapshot_token("7", "sh1", &adapter.config),
                operations: vec![ExternalOperation::Insert {
                    operation_id: "insert".to_string(),
                    values: vec![ExternalCellInput {
                        column_key: "col:1".to_string(),
                        value: Value::String("value".to_string()),
                    }],
                }],
            })
            .await
            .unwrap();

        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 4, "unknown insert must not be retried");
        assert_eq!(result.operation_results[0].outcome, OperationOutcome::Unknown);
        assert!(result.save_blocked);
    }

    #[tokio::test]
    async fn sheets_delete_readback_mismatch_is_unknown_and_stops_retry() {
        let structure = json!({
            "revision": 7,
            "sheets": [{ "sheet_id": "sh1", "title": "Sheet1", "row_count": 3, "column_count": 2 }]
        });
        let (base_url, server) = serve(vec![
            token_reply(),
            tool_reply(structure),
            tool_reply(json!({ "current_region": "A1:B3", "annotated_csv": "" })),
            tool_reply(json!({ "ranges": [{ "cells": [[{ "value": "Grace" }, { "value": 5 }]] }] })),
            tool_reply(json!({})),
            tool_reply(json!({ "ranges": [{ "cells": [[{ "value": "Unexpected" }, { "value": 9 }]] }] })),
        ])
        .await;
        let client = FeishuClient::with_base_url(base_url, "app", "secret", Duration::from_secs(5)).unwrap();
        let adapter = adapter(client);

        let result = adapter
            .apply_changes(ApplyChangesRequest {
                table: ExternalTableRef { table_key: "sheet:sh1".to_string(), display_name: "Sheet1".to_string() },
                snapshot_token: sheets_snapshot_token("7", "sh1", &adapter.config),
                operations: vec![ExternalOperation::Delete {
                    operation_id: "delete".to_string(),
                    row_key: "row:2".to_string(),
                }],
            })
            .await
            .unwrap();

        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 6);
        assert_eq!(result.operation_results[0].outcome, OperationOutcome::Unknown);
        assert!(result.new_snapshot_token.is_none());
        assert!(result.save_blocked);
    }
}
