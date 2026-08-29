use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use calamine::{open_workbook_auto, Data, Reader};
use percent_encoding::{percent_decode_str, utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::Value;

use super::file_support::{file_sha256, parse_index_key, replace_staged_file, unique_display_names};
use super::{
    AdapterCapabilities, ApplyChangesRequest, ApplyChangesResult, ConflictMode, DeleteMode, ExternalColumn,
    ExternalConnectionTestResult, ExternalOperation, ExternalRow, ExternalTableAdapter, ExternalTableError,
    ExternalTableRef, ExternalTableSchema, ExternalValueType, InsertMode, OperationOutcome, OperationResult,
    PageSnapshot, ReadPageRequest, ReadState, XlsxExternalConfig,
};

const SHEET_TABLE_PREFIX: &str = "sheet:";
const MAX_PAGE_SIZE: usize = 2_000;

#[derive(Debug)]
pub struct XlsxAdapter {
    path: PathBuf,
    config: XlsxExternalConfig,
    write_lock: tokio::sync::Mutex<()>,
}

#[derive(Debug, Clone, Copy)]
struct SheetBounds {
    start_row: u32,
    end_row: u32,
    start_col: u32,
    end_col: u32,
}

#[derive(Debug)]
struct XlsxSheetDocument {
    columns: Vec<ExternalColumn>,
    values: HashMap<(u32, u32), Value>,
    readonly_cells: HashSet<(u32, u32)>,
    bounds: SheetBounds,
    data_start_row: u32,
}

impl XlsxAdapter {
    pub fn new(path: PathBuf, config: XlsxExternalConfig) -> Self {
        Self { path, config, write_lock: tokio::sync::Mutex::new(()) }
    }

    fn table_ref(sheet_name: &str) -> ExternalTableRef {
        ExternalTableRef {
            table_key: format!("{SHEET_TABLE_PREFIX}{}", utf8_percent_encode(sheet_name, NON_ALPHANUMERIC)),
            display_name: sheet_name.to_string(),
        }
    }

    fn sheet_name(table: &ExternalTableRef) -> Result<String, ExternalTableError> {
        let encoded = table
            .table_key
            .strip_prefix(SHEET_TABLE_PREFIX)
            .ok_or_else(|| ExternalTableError::invalid(format!("Invalid XLSX worksheet key: {}", table.table_key)))?;
        percent_decode_str(encoded)
            .decode_utf8()
            .map(|value| value.into_owned())
            .map_err(|_| ExternalTableError::invalid(format!("Invalid XLSX worksheet key: {}", table.table_key)))
    }
}

#[async_trait]
impl ExternalTableAdapter for XlsxAdapter {
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            can_read: true,
            can_update: true,
            insert_mode: InsertMode::Append,
            delete_mode: DeleteMode::RemoveRow,
            supports_cell_readonly: true,
            conflict_mode: ConflictMode::FileSnapshot,
        }
    }

    async fn test_connection(&self) -> Result<ExternalConnectionTestResult, ExternalTableError> {
        let path = self.path.clone();
        let sheet_names = tokio::task::spawn_blocking(move || workbook_sheet_names(&path))
            .await
            .map_err(|error| ExternalTableError::io(format!("XLSX validation task failed: {error}")))??;
        Ok(ExternalConnectionTestResult::success(format!("XLSX file valid: {} worksheet(s)", sheet_names.len())))
    }

    async fn list_tables(&self) -> Result<Vec<ExternalTableRef>, ExternalTableError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            workbook_sheet_names(&path).map(|names| names.iter().map(|name| Self::table_ref(name)).collect())
        })
        .await
        .map_err(|error| ExternalTableError::io(format!("XLSX worksheet listing task failed: {error}")))?
    }

    async fn describe_table(&self, table: &ExternalTableRef) -> Result<ExternalTableSchema, ExternalTableError> {
        let sheet_name = Self::sheet_name(table)?;
        let path = self.path.clone();
        let config = self.config.clone();
        let table = table.clone();
        let capabilities = self.capabilities();
        tokio::task::spawn_blocking(move || {
            let readonly_reason = workbook_write_restriction(&path)?;
            let document = read_sheet_document(&path, &sheet_name, &config)?;
            let mut columns = document.columns;
            let writable = readonly_reason.is_none();
            if !writable {
                for column in &mut columns {
                    column.writable = false;
                }
            }
            Ok(ExternalTableSchema { table, columns, capabilities, writable, readonly_reason })
        })
        .await
        .map_err(|error| ExternalTableError::io(format!("XLSX describe task failed: {error}")))?
    }

    async fn read_page(&self, request: ReadPageRequest) -> Result<PageSnapshot, ExternalTableError> {
        let sheet_name = Self::sheet_name(&request.table)?;
        let limit = request.bounded_limit(MAX_PAGE_SIZE)?;
        let offset = parse_cursor(request.cursor.as_deref())?;
        let path = self.path.clone();
        let config = self.config.clone();
        let table = request.table;
        tokio::task::spawn_blocking(move || {
            let mut document = read_sheet_document(&path, &sheet_name, &config)?;
            let readonly_reason = workbook_write_restriction(&path)?;
            if readonly_reason.is_some() {
                for column in &mut document.columns {
                    column.writable = false;
                }
            }
            let row_count = document.bounds.end_row.saturating_sub(document.data_start_row).saturating_add(1) as usize;
            if offset > row_count {
                return Err(ExternalTableError::invalid(format!(
                    "XLSX cursor is past the end of the worksheet: {offset}"
                )));
            }
            let end = (offset + limit).min(row_count);
            let rows = (offset..end)
                .map(|relative_row| {
                    let row = document.data_start_row + relative_row as u32;
                    let mut readonly_column_keys = Vec::new();
                    let values = (document.bounds.start_col..=document.bounds.end_col)
                        .map(|column| {
                            if document.readonly_cells.contains(&(row, column)) {
                                readonly_column_keys.push(format!("col:{}", column + 1));
                            }
                            document.values.get(&(row, column)).cloned().unwrap_or(Value::Null)
                        })
                        .collect();
                    ExternalRow { row_key: format!("row:{}", row + 1), values, readonly_column_keys }
                })
                .collect();
            Ok(PageSnapshot {
                table,
                columns: document.columns,
                rows,
                next_cursor: (end < row_count).then(|| end.to_string()),
                snapshot_token: file_sha256(&path)?,
                read_state: ReadState::Complete,
            })
        })
        .await
        .map_err(|error| ExternalTableError::io(format!("XLSX read task failed: {error}")))?
    }

    async fn apply_changes(&self, request: ApplyChangesRequest) -> Result<ApplyChangesResult, ExternalTableError> {
        request.validate()?;
        let sheet_name = Self::sheet_name(&request.table)?;
        let _write_guard = self.write_lock.lock().await;
        let path = self.path.clone();
        let config = self.config.clone();
        tokio::task::spawn_blocking(move || apply_xlsx_changes(&path, &sheet_name, &config, request))
            .await
            .map_err(|error| ExternalTableError::io(format!("XLSX write task failed: {error}")))?
    }
}

fn workbook_sheet_names(path: &Path) -> Result<Vec<String>, ExternalTableError> {
    validate_xlsx_path(path)?;
    let workbook = open_workbook_auto(path)
        .map_err(|error| ExternalTableError::invalid(format!("Failed to open XLSX workbook: {error}")))?;
    Ok(workbook.sheet_names())
}

fn validate_xlsx_path(path: &Path) -> Result<(), ExternalTableError> {
    if !path.exists() {
        return Err(ExternalTableError::io(format!("XLSX file not found: {}", path.display())));
    }
    if !path.is_file() {
        return Err(ExternalTableError::invalid(format!("XLSX path is not a file: {}", path.display())));
    }
    let extension = path.extension().and_then(|extension| extension.to_str()).unwrap_or_default();
    if !extension.eq_ignore_ascii_case("xlsx") {
        return Err(ExternalTableError::unsupported(
            "Only ordinary .xlsx workbooks are supported; .xls, .xlsm, and .xlsb are read-only/unsupported",
        ));
    }
    Ok(())
}

fn workbook_write_restriction(path: &Path) -> Result<Option<String>, ExternalTableError> {
    validate_xlsx_path(path)?;
    let file = File::open(path)
        .map_err(|error| ExternalTableError::io(format!("Failed to inspect XLSX workbook: {error}")))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| ExternalTableError::invalid(format!("Invalid or encrypted XLSX workbook: {error}")))?;
    for index in 0..archive.len() {
        let name = archive
            .by_index(index)
            .map_err(|error| ExternalTableError::invalid(format!("Failed to inspect XLSX part: {error}")))?
            .name()
            .to_ascii_lowercase();
        let reason = if name.ends_with("vbaproject.bin") {
            Some("Macro-enabled workbooks are not writable in this DBX version")
        } else if name.starts_with("xl/externallinks/") {
            Some("Workbooks with external links are not writable in this DBX version")
        } else if name.starts_with("xl/pivottables/") || name.starts_with("xl/pivotcache/") {
            Some("Workbooks with pivot tables are not writable in this DBX version")
        } else if name.starts_with("xl/slicers/") || name.starts_with("xl/slicercaches/") {
            Some("Workbooks with slicers are not writable in this DBX version")
        } else {
            None
        };
        if let Some(reason) = reason {
            return Ok(Some(reason.to_string()));
        }
    }
    Ok(None)
}

fn read_sheet_document(
    path: &Path,
    sheet_name: &str,
    config: &XlsxExternalConfig,
) -> Result<XlsxSheetDocument, ExternalTableError> {
    validate_xlsx_path(path)?;
    let mut workbook = open_workbook_auto(path)
        .map_err(|error| ExternalTableError::invalid(format!("Failed to open XLSX workbook: {error}")))?;
    let range = workbook
        .worksheet_range(sheet_name)
        .map_err(|error| ExternalTableError::invalid(format!("Failed to read worksheet '{sheet_name}': {error}")))?;
    let formulas = workbook
        .worksheet_formula(sheet_name)
        .map_err(|error| ExternalTableError::invalid(format!("Failed to read worksheet formulas: {error}")))?;
    let bounds = resolve_sheet_bounds(&range, config.data_range.as_deref())?;
    let data_start_row = bounds.start_row + u32::from(config.has_header);
    let width = bounds.end_col.saturating_sub(bounds.start_col).saturating_add(1) as usize;
    let raw_headers = if config.has_header {
        (bounds.start_col..=bounds.end_col)
            .map(|column| range.get_value((bounds.start_row, column)).map(data_display_string).unwrap_or_default())
            .collect::<Vec<_>>()
    } else {
        (0..width).map(|index| format!("column_{}", index + 1)).collect()
    };
    let display_headers = unique_display_names(&raw_headers);
    let mut values = HashMap::new();
    let mut readonly_cells = HashSet::new();
    let mut inferred_types = vec![ExternalValueType::Unknown; width];
    if data_start_row <= bounds.end_row {
        for row in data_start_row..=bounds.end_row {
            for (column_index, column) in (bounds.start_col..=bounds.end_col).enumerate() {
                let value = range.get_value((row, column)).map(data_to_json).unwrap_or(Value::Null);
                inferred_types[column_index] = merge_value_type(inferred_types[column_index], value_type(&value));
                values.insert((row, column), value);
                if formulas.get_value((row, column)).is_some_and(|formula| !formula.trim().is_empty()) {
                    readonly_cells.insert((row, column));
                }
            }
        }
    }
    for cell in merged_non_anchor_cells(path, sheet_name)? {
        readonly_cells.insert(cell);
    }
    let columns = display_headers
        .into_iter()
        .enumerate()
        .map(|(index, display_name)| ExternalColumn {
            column_key: format!("col:{}", bounds.start_col + index as u32 + 1),
            display_name,
            value_type: inferred_types[index],
            writable: true,
        })
        .collect();
    Ok(XlsxSheetDocument { columns, values, readonly_cells, bounds, data_start_row })
}

fn resolve_sheet_bounds(
    range: &calamine::Range<Data>,
    configured: Option<&str>,
) -> Result<SheetBounds, ExternalTableError> {
    let used_start = range.start().unwrap_or((0, 0));
    let used_end = range.end().unwrap_or(used_start);
    if let Some(configured) = configured.map(str::trim).filter(|value| !value.is_empty()) {
        let (start, end) = parse_a1_range(configured)?;
        return Ok(SheetBounds {
            start_row: start.0,
            start_col: start.1,
            end_row: end.map(|value| value.0).unwrap_or(used_end.0).max(start.0),
            end_col: end.map(|value| value.1).unwrap_or(used_end.1).max(start.1),
        });
    }
    Ok(SheetBounds { start_row: used_start.0, start_col: used_start.1, end_row: used_end.0, end_col: used_end.1 })
}

fn parse_a1_range(value: &str) -> Result<((u32, u32), Option<(u32, u32)>), ExternalTableError> {
    let value = value.rsplit_once('!').map(|(_, range)| range).unwrap_or(value);
    let mut parts = value.split(':');
    let start = parse_a1_cell(parts.next().unwrap_or_default())?;
    let end = parts.next().map(parse_a1_cell).transpose()?;
    if parts.next().is_some() {
        return Err(ExternalTableError::invalid(format!("Invalid XLSX data range: {value}")));
    }
    if end.is_some_and(|end| end.0 < start.0 || end.1 < start.1) {
        return Err(ExternalTableError::invalid(format!("XLSX data range end precedes start: {value}")));
    }
    Ok((start, end))
}

fn parse_a1_cell(value: &str) -> Result<(u32, u32), ExternalTableError> {
    let value = value.trim().trim_matches('$');
    let split = value
        .find(|character: char| character.is_ascii_digit())
        .ok_or_else(|| ExternalTableError::invalid(format!("Invalid A1 cell reference: {value}")))?;
    let (column, row) = value.split_at(split);
    if column.is_empty() || row.is_empty() || !column.chars().all(|character| character.is_ascii_alphabetic()) {
        return Err(ExternalTableError::invalid(format!("Invalid A1 cell reference: {value}")));
    }
    let mut column_index = 0_u32;
    for character in column.chars() {
        column_index = column_index
            .checked_mul(26)
            .and_then(|value| value.checked_add(character.to_ascii_uppercase() as u32 - 'A' as u32 + 1))
            .ok_or_else(|| ExternalTableError::invalid(format!("A1 column is too large: {column}")))?;
    }
    let row_index = row.parse::<u32>().map_err(|_| ExternalTableError::invalid(format!("Invalid A1 row: {row}")))?;
    if row_index == 0 || column_index == 0 {
        return Err(ExternalTableError::invalid(format!("A1 coordinates start at 1: {value}")));
    }
    Ok((row_index - 1, column_index - 1))
}

fn merged_non_anchor_cells(path: &Path, sheet_name: &str) -> Result<HashSet<(u32, u32)>, ExternalTableError> {
    let workbook = umya_spreadsheet::reader::xlsx::read(path)
        .map_err(|error| ExternalTableError::invalid(format!("Failed to inspect XLSX merge cells: {error}")))?;
    let sheet = workbook
        .sheet_by_name(sheet_name)
        .map_err(|error| ExternalTableError::invalid(format!("Worksheet not found: {error}")))?;
    let mut cells = HashSet::new();
    for range in sheet.merge_cells() {
        let Some(start_col) = range.coordinate_start_col().map(|value| value.num()) else {
            continue;
        };
        let Some(start_row) = range.coordinate_start_row().map(|value| value.num()) else {
            continue;
        };
        let end_col = range.coordinate_end_col().map(|value| value.num()).unwrap_or(start_col);
        let end_row = range.coordinate_end_row().map(|value| value.num()).unwrap_or(start_row);
        for row in start_row..=end_row {
            for column in start_col..=end_col {
                if row != start_row || column != start_col {
                    cells.insert((row - 1, column - 1));
                }
            }
        }
    }
    Ok(cells)
}

fn data_to_json(data: &Data) -> Value {
    match data {
        Data::Empty => Value::Null,
        Data::String(value) => Value::String(value.clone()),
        Data::Int(value) => Value::Number((*value).into()),
        Data::Float(value) => serde_json::Number::from_f64(*value).map(Value::Number).unwrap_or(Value::Null),
        Data::Bool(value) => Value::Bool(*value),
        Data::DateTime(value) => Value::String(value.to_string()),
        Data::DateTimeIso(value) | Data::DurationIso(value) => Value::String(value.clone()),
        Data::Error(error) => Value::String(format!("#{error:?}")),
    }
}

fn data_display_string(data: &Data) -> String {
    match data_to_json(data) {
        Value::Null => String::new(),
        Value::String(value) => value,
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

fn parse_cursor(cursor: Option<&str>) -> Result<usize, ExternalTableError> {
    match cursor {
        None | Some("") => Ok(0),
        Some(cursor) => {
            cursor.parse::<usize>().map_err(|_| ExternalTableError::invalid(format!("Invalid XLSX cursor: {cursor}")))
        }
    }
}

fn apply_xlsx_changes(
    path: &Path,
    sheet_name: &str,
    config: &XlsxExternalConfig,
    request: ApplyChangesRequest,
) -> Result<ApplyChangesResult, ExternalTableError> {
    if let Some(reason) = workbook_write_restriction(path)? {
        return Ok(ApplyChangesResult {
            operation_results: request
                .operations
                .iter()
                .map(|operation| {
                    OperationResult::new(operation.operation_id(), OperationOutcome::Rejected).message(reason.clone())
                })
                .collect(),
            new_snapshot_token: Some(file_sha256(path)?),
            reload_required: false,
            save_blocked: false,
        });
    }
    let current_snapshot = file_sha256(path)?;
    if current_snapshot != request.snapshot_token {
        return Ok(ApplyChangesResult {
            operation_results: request
                .operations
                .iter()
                .map(|operation| {
                    OperationResult::new(operation.operation_id(), OperationOutcome::Conflict)
                        .message("XLSX workbook changed after it was read")
                })
                .collect(),
            new_snapshot_token: Some(current_snapshot),
            reload_required: true,
            save_blocked: false,
        });
    }

    let document = read_sheet_document(path, sheet_name, config)?;
    let mut workbook = umya_spreadsheet::reader::xlsx::read(path)
        .map_err(|error| ExternalTableError::invalid(format!("Failed to open XLSX workbook for editing: {error}")))?;
    let mut results = vec![None; request.operations.len()];
    let mut updates = Vec::new();
    let mut deletes = Vec::new();
    let mut inserts = Vec::new();
    let mut seen_delete_rows = HashSet::new();

    for (operation_index, operation) in request.operations.iter().enumerate() {
        match operation {
            ExternalOperation::Update { operation_id, row_key, column_key, old_value, new_value } => {
                let row = match parse_index_key(row_key, "row:") {
                    Ok(value) => value as u32,
                    Err(error) => {
                        results[operation_index] = Some(
                            OperationResult::new(operation_id, OperationOutcome::Rejected).message(error.to_string()),
                        );
                        continue;
                    }
                };
                let column = match parse_index_key(column_key, "col:") {
                    Ok(value) => value as u32,
                    Err(error) => {
                        results[operation_index] = Some(
                            OperationResult::new(operation_id, OperationOutcome::Rejected).message(error.to_string()),
                        );
                        continue;
                    }
                };
                if row == 0
                    || column == 0
                    || row - 1 < document.data_start_row
                    || row - 1 > document.bounds.end_row
                    || column - 1 < document.bounds.start_col
                    || column - 1 > document.bounds.end_col
                {
                    results[operation_index] = Some(
                        OperationResult::new(operation_id, OperationOutcome::Rejected)
                            .message("XLSX cell key is outside the selected data range"),
                    );
                    continue;
                }
                if document.readonly_cells.contains(&(row - 1, column - 1)) {
                    results[operation_index] = Some(
                        OperationResult::new(operation_id, OperationOutcome::Rejected)
                            .message("Formula cells and merged non-anchor cells are read-only"),
                    );
                    continue;
                }
                if document.values.get(&(row - 1, column - 1)).cloned().unwrap_or(Value::Null) != *old_value {
                    results[operation_index] = Some(
                        OperationResult::new(operation_id, OperationOutcome::Conflict)
                            .message("XLSX cell value changed after it was read"),
                    );
                    continue;
                }
                if let Err(error) = validate_xlsx_value(new_value) {
                    results[operation_index] =
                        Some(OperationResult::new(operation_id, OperationOutcome::Rejected).message(error.to_string()));
                    continue;
                }
                updates.push((operation_index, row, column, new_value.clone()));
            }
            ExternalOperation::Delete { operation_id, row_key } => {
                let row = match parse_index_key(row_key, "row:") {
                    Ok(value) => value as u32,
                    Err(error) => {
                        results[operation_index] = Some(
                            OperationResult::new(operation_id, OperationOutcome::Rejected).message(error.to_string()),
                        );
                        continue;
                    }
                };
                if row == 0 || row - 1 < document.data_start_row || row - 1 > document.bounds.end_row {
                    results[operation_index] = Some(
                        OperationResult::new(operation_id, OperationOutcome::Rejected)
                            .message("XLSX row key is outside the selected data range or points to the header"),
                    );
                } else if !seen_delete_rows.insert(row) {
                    results[operation_index] = Some(
                        OperationResult::new(operation_id, OperationOutcome::Rejected)
                            .message("XLSX row is already scheduled for deletion"),
                    );
                } else {
                    deletes.push((operation_index, row));
                }
            }
            ExternalOperation::Insert { operation_id, values } => {
                let mut row = vec![Value::Null; document.columns.len()];
                let mut seen_columns = HashSet::new();
                let mut rejected = None;
                for cell in values {
                    let absolute_column = match parse_index_key(&cell.column_key, "col:") {
                        Ok(value) => value as u32,
                        Err(error) => {
                            rejected = Some(error.to_string());
                            break;
                        }
                    };
                    if absolute_column == 0
                        || absolute_column - 1 < document.bounds.start_col
                        || absolute_column - 1 > document.bounds.end_col
                    {
                        rejected = Some(format!("XLSX column is outside the selected data range: {}", cell.column_key));
                        break;
                    }
                    let relative_column = (absolute_column - 1 - document.bounds.start_col) as usize;
                    if !seen_columns.insert(relative_column) {
                        rejected = Some(format!("XLSX insert contains duplicate column: {}", cell.column_key));
                        break;
                    }
                    if let Err(error) = validate_xlsx_value(&cell.value) {
                        rejected = Some(error.to_string());
                        break;
                    }
                    row[relative_column] = cell.value.clone();
                }
                if let Some(message) = rejected {
                    results[operation_index] =
                        Some(OperationResult::new(operation_id, OperationOutcome::Rejected).message(message));
                } else {
                    inserts.push((operation_index, row));
                }
            }
        }
    }

    {
        let sheet = workbook
            .sheet_by_name_mut(sheet_name)
            .map_err(|error| ExternalTableError::invalid(format!("Worksheet not found: {error}")))?;
        for (operation_index, row, column, value) in updates {
            set_umya_cell_value(sheet.cell_mut((column, row)), &value)?;
            results[operation_index] = Some(OperationResult::new(
                request.operations[operation_index].operation_id(),
                OperationOutcome::Applied,
            ));
        }
    }

    deletes.sort_by_key(|(_, row)| std::cmp::Reverse(*row));
    for (operation_index, row) in &deletes {
        workbook.remove_row(sheet_name, *row, 1);
        results[*operation_index] =
            Some(OperationResult::new(request.operations[*operation_index].operation_id(), OperationOutcome::Applied));
    }

    let deleted_count = deletes.len() as u32;
    let mut append_row = document.bounds.end_row.saturating_add(2).saturating_sub(deleted_count);
    {
        let sheet = workbook
            .sheet_by_name_mut(sheet_name)
            .map_err(|error| ExternalTableError::invalid(format!("Worksheet not found: {error}")))?;
        for (operation_index, row) in inserts {
            for (column_index, value) in row.iter().enumerate() {
                let column = document.bounds.start_col + column_index as u32 + 1;
                set_umya_cell_value(sheet.cell_mut((column, append_row)), value)?;
            }
            results[operation_index] = Some(
                OperationResult::new(request.operations[operation_index].operation_id(), OperationOutcome::Applied)
                    .message(format!("Appended XLSX row {append_row}")),
            );
            append_row += 1;
        }
    }

    let mut operation_results = results
        .into_iter()
        .enumerate()
        .map(|(index, result)| {
            result.unwrap_or_else(|| {
                OperationResult::new(request.operations[index].operation_id(), OperationOutcome::Rejected)
                    .message("XLSX operation could not be prepared")
            })
        })
        .collect::<Vec<_>>();
    if !operation_results.iter().any(|result| result.outcome == OperationOutcome::Applied) {
        let reload_required = operation_results.iter().any(|result| result.outcome == OperationOutcome::Conflict);
        return Ok(ApplyChangesResult {
            operation_results,
            new_snapshot_token: Some(current_snapshot),
            reload_required,
            save_blocked: false,
        });
    }

    let parent = path.parent().ok_or_else(|| ExternalTableError::io("XLSX path has no parent directory"))?;
    let staged = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| ExternalTableError::io(format!("Failed to create XLSX staging file: {error}")))?;
    let staged_path = staged.into_temp_path();
    umya_spreadsheet::writer::xlsx::write(&workbook, &staged_path)
        .map_err(|error| ExternalTableError::io(format!("Failed to write XLSX staging file: {error}")))?;
    File::open(&staged_path)
        .and_then(|file| file.sync_all())
        .map_err(|error| ExternalTableError::io(format!("Failed to sync XLSX staging file: {error}")))?;
    let before_replace = file_sha256(path)?;
    if before_replace != request.snapshot_token {
        let operation_results = operation_results
            .into_iter()
            .map(|mut result| {
                if result.outcome == OperationOutcome::Applied {
                    result.outcome = OperationOutcome::Conflict;
                    result.message = Some("XLSX workbook changed before replacement".to_string());
                }
                result
            })
            .collect();
        return Ok(ApplyChangesResult {
            operation_results,
            new_snapshot_token: Some(before_replace),
            reload_required: true,
            save_blocked: false,
        });
    }
    replace_staged_file(&staged_path, path)?;
    let new_snapshot = match file_sha256(path) {
        Ok(snapshot) => Some(snapshot),
        Err(error) => {
            annotate_xlsx_applied(&mut operation_results, &format!("saved, but snapshot readback failed: {error}"));
            None
        }
    };
    if let Err(error) = read_sheet_document(path, sheet_name, config) {
        annotate_xlsx_applied(&mut operation_results, &format!("saved, but worksheet readback failed: {error}"));
    }
    Ok(ApplyChangesResult {
        operation_results,
        new_snapshot_token: new_snapshot,
        reload_required: true,
        save_blocked: false,
    })
}

fn annotate_xlsx_applied(results: &mut [OperationResult], message: &str) {
    for result in results.iter_mut().filter(|result| result.outcome == OperationOutcome::Applied) {
        result.message = Some(match result.message.take() {
            Some(existing) => format!("{existing}; {message}"),
            None => message.to_string(),
        });
    }
}

fn validate_xlsx_value(value: &Value) -> Result<(), ExternalTableError> {
    if matches!(value, Value::Array(_) | Value::Object(_)) {
        return Err(ExternalTableError::invalid("XLSX cells accept only string, number, boolean, or null values"));
    }
    Ok(())
}

fn set_umya_cell_value(cell: &mut umya_spreadsheet::Cell, value: &Value) -> Result<(), ExternalTableError> {
    match value {
        Value::Null => {
            cell.set_blank();
        }
        Value::String(value) => {
            cell.set_value_string(value);
        }
        Value::Bool(value) => {
            cell.set_value_bool(*value);
        }
        Value::Number(value) => {
            let number = value
                .as_f64()
                .ok_or_else(|| ExternalTableError::invalid("XLSX number is outside the supported range"))?;
            cell.set_value_number(number);
        }
        Value::Array(_) | Value::Object(_) => return Err(ExternalTableError::invalid("Unsupported XLSX cell value")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::ExternalCellInput;
    use super::*;

    fn create_round_trip_fixture(path: &Path) {
        let mut workbook = umya_spreadsheet::new_file();
        {
            let sheet = workbook.sheet_by_name_mut("Sheet1").unwrap();
            for (column, value) in ["id", "name", "amount", "total", "merged", "merged-tail"].iter().enumerate() {
                sheet.cell_mut((column as u32 + 1, 1)).set_value_string(*value);
            }
            sheet.cell_mut((1, 2)).set_value_number(1);
            sheet.cell_mut((2, 2)).set_value_string("Ada");
            sheet.cell_mut((3, 2)).set_value_number(10);
            sheet.cell_mut((4, 2)).set_formula("=C2*2").set_formula_result_number(20);
            sheet.cell_mut((5, 2)).set_value_string("anchor");
            sheet.cell_mut((1, 3)).set_value_number(2);
            sheet.cell_mut((2, 3)).set_value_string("Delete me");
            sheet.cell_mut((3, 3)).set_value_number(5);
            sheet.cell_mut((1, 4)).set_value_number(3);
            sheet.cell_mut((2, 4)).set_value_string("Keep me");
            sheet.cell_mut((3, 4)).set_value_number(7);
            sheet.style_mut("B2").set_background_color("FFFF0000");
            sheet.add_merge_cells("E2:F2");
            sheet.column_dimension_by_number_mut(2).set_width(24.0);
            sheet.row_dimension_mut(1).set_height(28.0);
        }
        workbook.new_sheet("Untouched").unwrap().cell_mut("A1").set_value_string("preserve");
        umya_spreadsheet::writer::xlsx::write(&workbook, path).unwrap();
    }

    #[tokio::test]
    async fn xlsx_crud_round_trip_preserves_formula_style_merge_dimensions_and_other_sheet() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("round-trip.xlsx");
        create_round_trip_fixture(&path);
        let adapter = XlsxAdapter::new(path.clone(), XlsxExternalConfig::default());
        let table = XlsxAdapter::table_ref("Sheet1");
        let page = adapter.read_page(ReadPageRequest { table: table.clone(), cursor: None, limit: 20 }).await.unwrap();
        assert!(page.rows[0].readonly_column_keys.contains(&"col:4".to_string()));
        assert!(page.rows[0].readonly_column_keys.contains(&"col:6".to_string()));

        let result = adapter
            .apply_changes(ApplyChangesRequest {
                table,
                snapshot_token: page.snapshot_token,
                operations: vec![
                    ExternalOperation::Update {
                        operation_id: "update".to_string(),
                        row_key: "row:2".to_string(),
                        column_key: "col:2".to_string(),
                        old_value: Value::String("Ada".to_string()),
                        new_value: Value::String("Ada Lovelace".to_string()),
                    },
                    ExternalOperation::Delete { operation_id: "delete".to_string(), row_key: "row:3".to_string() },
                    ExternalOperation::Insert {
                        operation_id: "insert".to_string(),
                        values: vec![
                            ExternalCellInput { column_key: "col:1".to_string(), value: Value::Number(4.into()) },
                            ExternalCellInput {
                                column_key: "col:2".to_string(),
                                value: Value::String("Grace".to_string()),
                            },
                        ],
                    },
                ],
            })
            .await
            .unwrap();

        assert!(result.operation_results.iter().all(|result| result.outcome == OperationOutcome::Applied));
        let workbook = umya_spreadsheet::reader::xlsx::read(&path).unwrap();
        let sheet = workbook.sheet_by_name("Sheet1").unwrap();
        assert_eq!(sheet.cell("B2").unwrap().value(), "Ada Lovelace");
        assert_eq!(sheet.cell("B3").unwrap().value(), "Keep me");
        assert_eq!(sheet.cell("B4").unwrap().value(), "Grace");
        assert!(!sheet.cell("D2").unwrap().formula().is_empty());
        assert_eq!(sheet.merge_cells()[0].range(), "E2:F2");
        assert_eq!(sheet.column_dimension_by_number(2).unwrap().width(), 24.0);
        assert_eq!(sheet.row_dimension(1).unwrap().height(), 28.0);
        assert_eq!(workbook.sheet_by_name("Untouched").unwrap().cell("A1").unwrap().value(), "preserve");
        assert_eq!(
            sheet
                .cell("B2")
                .unwrap()
                .style()
                .fill()
                .unwrap()
                .pattern_fill()
                .unwrap()
                .foreground_color()
                .unwrap()
                .argb_str(),
            "FFFF0000"
        );
    }

    #[tokio::test]
    async fn xlsx_hash_conflict_keeps_external_workbook() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("conflict.xlsx");
        create_round_trip_fixture(&path);
        let adapter = XlsxAdapter::new(path.clone(), XlsxExternalConfig::default());
        let table = XlsxAdapter::table_ref("Sheet1");
        let page = adapter.read_page(ReadPageRequest { table: table.clone(), cursor: None, limit: 20 }).await.unwrap();
        let mut external = umya_spreadsheet::reader::xlsx::read(&path).unwrap();
        external.sheet_by_name_mut("Sheet1").unwrap().cell_mut("B2").set_value_string("External");
        umya_spreadsheet::writer::xlsx::write(&external, &path).unwrap();

        let result = adapter
            .apply_changes(ApplyChangesRequest {
                table,
                snapshot_token: page.snapshot_token,
                operations: vec![ExternalOperation::Update {
                    operation_id: "update".to_string(),
                    row_key: "row:2".to_string(),
                    column_key: "col:2".to_string(),
                    old_value: Value::String("Ada".to_string()),
                    new_value: Value::String("Local".to_string()),
                }],
            })
            .await
            .unwrap();

        assert_eq!(result.operation_results[0].outcome, OperationOutcome::Conflict);
        let readback = umya_spreadsheet::reader::xlsx::read(&path).unwrap();
        assert_eq!(readback.sheet_by_name("Sheet1").unwrap().cell("B2").unwrap().value(), "External");
    }
}
