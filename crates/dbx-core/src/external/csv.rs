use std::borrow::Cow;
use std::io::Write;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use encoding_rs::{Encoding, UTF_8};
use serde_json::Value;

use super::file_support::{bytes_sha256, file_sha256, parse_index_key, replace_staged_file, unique_display_names};
use super::{
    AdapterCapabilities, ApplyChangesRequest, ApplyChangesResult, ConflictMode, CsvExternalConfig, DeleteMode,
    ExternalColumn, ExternalConnectionTestResult, ExternalOperation, ExternalRow, ExternalTableAdapter,
    ExternalTableError, ExternalTableRef, ExternalTableSchema, ExternalValueType, InsertMode, OperationOutcome,
    OperationResult, PageSnapshot, ReadPageRequest, ReadState,
};

const CSV_TABLE_KEY: &str = "csv";
const MAX_PAGE_SIZE: usize = 2_000;

#[derive(Debug)]
pub struct CsvAdapter {
    path: PathBuf,
    config: CsvExternalConfig,
    write_lock: tokio::sync::Mutex<()>,
}

#[derive(Debug, Clone)]
struct CsvFormat {
    encoding: &'static Encoding,
    utf8_bom: bool,
    crlf: bool,
}

#[derive(Debug, Clone)]
struct CsvDocument {
    raw_headers: Vec<String>,
    display_headers: Vec<String>,
    rows: Vec<Vec<String>>,
    format: CsvFormat,
}

impl CsvAdapter {
    pub fn new(path: PathBuf, mut config: CsvExternalConfig) -> Self {
        if config.delimiter == ","
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("tsv"))
        {
            config.delimiter = "\t".to_string();
        }
        Self { path, config, write_lock: tokio::sync::Mutex::new(()) }
    }

    fn table_ref(&self) -> ExternalTableRef {
        let display_name = self
            .path
            .file_stem()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .unwrap_or("CSV")
            .to_string();
        ExternalTableRef { table_key: CSV_TABLE_KEY.to_string(), display_name }
    }

    fn validate_table(table: &ExternalTableRef) -> Result<(), ExternalTableError> {
        if table.table_key != CSV_TABLE_KEY {
            return Err(ExternalTableError::invalid(format!("Unknown CSV table key: {}", table.table_key)));
        }
        Ok(())
    }
}

#[async_trait]
impl ExternalTableAdapter for CsvAdapter {
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            can_read: true,
            can_update: true,
            insert_mode: InsertMode::Append,
            delete_mode: DeleteMode::RemoveRow,
            supports_cell_readonly: false,
            conflict_mode: ConflictMode::FileSnapshot,
        }
    }

    async fn test_connection(&self) -> Result<ExternalConnectionTestResult, ExternalTableError> {
        let path = self.path.clone();
        let config = self.config.clone();
        let (column_count, row_count) = tokio::task::spawn_blocking(move || {
            let document = read_document(&path, &config)?;
            Ok::<_, ExternalTableError>((document.raw_headers.len(), document.rows.len()))
        })
        .await
        .map_err(|error| ExternalTableError::io(format!("CSV validation task failed: {error}")))??;
        Ok(ExternalConnectionTestResult::success(format!("CSV file valid: {column_count} columns, {row_count} rows")))
    }

    async fn list_tables(&self) -> Result<Vec<ExternalTableRef>, ExternalTableError> {
        self.test_connection().await?;
        Ok(vec![self.table_ref()])
    }

    async fn describe_table(&self, table: &ExternalTableRef) -> Result<ExternalTableSchema, ExternalTableError> {
        Self::validate_table(table)?;
        let page = self.read_page(ReadPageRequest { table: table.clone(), cursor: None, limit: 1 }).await?;
        Ok(ExternalTableSchema {
            table: table.clone(),
            columns: page.columns,
            capabilities: self.capabilities(),
            writable: true,
            readonly_reason: None,
        })
    }

    async fn read_page(&self, request: ReadPageRequest) -> Result<PageSnapshot, ExternalTableError> {
        Self::validate_table(&request.table)?;
        let limit = request.bounded_limit(MAX_PAGE_SIZE)?;
        let offset = parse_cursor(request.cursor.as_deref())?;
        let path = self.path.clone();
        let config = self.config.clone();
        let table = request.table;
        tokio::task::spawn_blocking(move || {
            let (document, snapshot_token) = read_document_with_snapshot(&path, &config)?;
            let columns = csv_columns(&document.display_headers);
            if offset > document.rows.len() {
                return Err(ExternalTableError::invalid(format!("CSV cursor is past the end of the file: {offset}")));
            }
            let end = (offset + limit).min(document.rows.len());
            let rows = document.rows[offset..end]
                .iter()
                .enumerate()
                .map(|(index, row)| ExternalRow {
                    row_key: format!("row:{}", offset + index),
                    values: row.iter().map(|value| csv_string_to_json(value)).collect(),
                    readonly_column_keys: Vec::new(),
                })
                .collect();
            Ok(PageSnapshot {
                table,
                columns,
                rows,
                next_cursor: (end < document.rows.len()).then(|| end.to_string()),
                snapshot_token,
                read_state: ReadState::Complete,
            })
        })
        .await
        .map_err(|error| ExternalTableError::io(format!("CSV read task failed: {error}")))?
    }

    async fn apply_changes(&self, request: ApplyChangesRequest) -> Result<ApplyChangesResult, ExternalTableError> {
        Self::validate_table(&request.table)?;
        request.validate()?;
        let _write_guard = self.write_lock.lock().await;
        let path = self.path.clone();
        let config = self.config.clone();
        tokio::task::spawn_blocking(move || apply_csv_changes(&path, &config, request))
            .await
            .map_err(|error| ExternalTableError::io(format!("CSV write task failed: {error}")))?
    }
}

fn read_document(path: &Path, config: &CsvExternalConfig) -> Result<CsvDocument, ExternalTableError> {
    if !path.exists() {
        return Err(ExternalTableError::io(format!("CSV file not found: {}", path.display())));
    }
    if !path.is_file() {
        return Err(ExternalTableError::invalid(format!("CSV path is not a file: {}", path.display())));
    }
    let bytes = std::fs::read(path)
        .map_err(|error| ExternalTableError::io(format!("Failed to read CSV file {}: {error}", path.display())))?;
    read_document_bytes(&bytes, config)
}

fn read_document_with_snapshot(
    path: &Path,
    config: &CsvExternalConfig,
) -> Result<(CsvDocument, String), ExternalTableError> {
    if !path.exists() {
        return Err(ExternalTableError::io(format!("CSV file not found: {}", path.display())));
    }
    if !path.is_file() {
        return Err(ExternalTableError::invalid(format!("CSV path is not a file: {}", path.display())));
    }
    let bytes = std::fs::read(path)
        .map_err(|error| ExternalTableError::io(format!("Failed to read CSV file {}: {error}", path.display())))?;
    let snapshot = bytes_sha256(&bytes);
    Ok((read_document_bytes(&bytes, config)?, snapshot))
}

fn read_document_bytes(bytes: &[u8], config: &CsvExternalConfig) -> Result<CsvDocument, ExternalTableError> {
    let delimiter = config.delimiter_byte().map_err(ExternalTableError::invalid)?;
    let utf8_bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
    let label = config.encoding.trim();
    let encoding = if label.is_empty() || label.eq_ignore_ascii_case("auto") {
        UTF_8
    } else {
        Encoding::for_label(label.as_bytes()).ok_or_else(|| {
            ExternalTableError::invalid(format!("Unsupported CSV encoding label: {}", config.encoding))
        })?
    };
    let (decoded, actual_encoding, had_errors) = encoding.decode(&bytes);
    if had_errors {
        return Err(ExternalTableError::invalid(format!(
            "CSV file contains bytes invalid for encoding {}",
            actual_encoding.name()
        )));
    }
    let crlf = decoded.contains("\r\n");
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(config.has_header)
        .flexible(true)
        .from_reader(decoded.as_bytes());

    let mut raw_headers = if config.has_header {
        reader
            .headers()
            .map_err(|error| ExternalTableError::invalid(format!("Failed to read CSV headers: {error}")))?
            .iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let mut rows = Vec::new();
    let mut width = raw_headers.len();
    for record in reader.records() {
        let record = record.map_err(|error| ExternalTableError::invalid(format!("CSV parse error: {error}")))?;
        width = width.max(record.len());
        rows.push(record.iter().map(str::to_string).collect::<Vec<_>>());
    }
    if !config.has_header {
        raw_headers = (0..width).map(|index| format!("column_{}", index + 1)).collect();
    } else if raw_headers.len() < width {
        raw_headers.resize(width, String::new());
    }
    for row in &mut rows {
        row.resize(width, String::new());
    }
    let display_headers = unique_display_names(&raw_headers);
    Ok(CsvDocument {
        raw_headers,
        display_headers,
        rows,
        format: CsvFormat { encoding: actual_encoding, utf8_bom, crlf },
    })
}

fn csv_columns(headers: &[String]) -> Vec<ExternalColumn> {
    headers
        .iter()
        .enumerate()
        .map(|(index, display_name)| ExternalColumn {
            column_key: format!("col:{index}"),
            display_name: display_name.clone(),
            value_type: ExternalValueType::String,
            writable: true,
        })
        .collect()
}

fn parse_cursor(cursor: Option<&str>) -> Result<usize, ExternalTableError> {
    match cursor {
        None | Some("") => Ok(0),
        Some(cursor) => {
            cursor.parse::<usize>().map_err(|_| ExternalTableError::invalid(format!("Invalid CSV cursor: {cursor}")))
        }
    }
}

fn csv_string_to_json(value: &str) -> Value {
    if value.is_empty() {
        Value::Null
    } else {
        Value::String(value.to_string())
    }
}

fn json_to_csv_string(value: &Value) -> Result<String, ExternalTableError> {
    match value {
        Value::Null => Ok(String::new()),
        Value::String(value) => Ok(value.clone()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Array(_) | Value::Object(_) => {
            Err(ExternalTableError::invalid("CSV cells accept only string, number, boolean, or null values"))
        }
    }
}

fn apply_csv_changes(
    path: &Path,
    config: &CsvExternalConfig,
    request: ApplyChangesRequest,
) -> Result<ApplyChangesResult, ExternalTableError> {
    let (mut document, current_snapshot) = read_document_with_snapshot(path, config)?;
    if current_snapshot != request.snapshot_token {
        return Ok(ApplyChangesResult {
            operation_results: request
                .operations
                .iter()
                .map(|operation| {
                    OperationResult::new(operation.operation_id(), OperationOutcome::Conflict)
                        .message("CSV file changed after it was read")
                })
                .collect(),
            new_snapshot_token: None,
            reload_required: true,
            save_blocked: true,
        });
    }

    let mut operation_results = Vec::with_capacity(request.operations.len());
    let mut deleted_rows = std::collections::HashSet::new();
    let mut appended_rows = Vec::new();

    for operation in &request.operations {
        let result = match operation {
            ExternalOperation::Update { operation_id, row_key, column_key, old_value, new_value } => {
                let row_index = match parse_index_key(row_key, "row:") {
                    Ok(index) => index,
                    Err(error) => {
                        operation_results.push(
                            OperationResult::new(operation_id, OperationOutcome::Rejected).message(error.to_string()),
                        );
                        continue;
                    }
                };
                let column_index = match parse_index_key(column_key, "col:") {
                    Ok(index) => index,
                    Err(error) => {
                        operation_results.push(
                            OperationResult::new(operation_id, OperationOutcome::Rejected).message(error.to_string()),
                        );
                        continue;
                    }
                };
                let Some(row) = document.rows.get_mut(row_index) else {
                    operation_results.push(
                        OperationResult::new(operation_id, OperationOutcome::Rejected)
                            .message(format!("CSV row no longer exists: {row_key}")),
                    );
                    continue;
                };
                let Some(cell) = row.get_mut(column_index) else {
                    operation_results.push(
                        OperationResult::new(operation_id, OperationOutcome::Rejected)
                            .message(format!("CSV column does not exist: {column_key}")),
                    );
                    continue;
                };
                if csv_string_to_json(cell) != *old_value {
                    OperationResult::new(operation_id, OperationOutcome::Conflict)
                        .message("CSV cell value changed after it was read")
                } else {
                    match json_to_csv_string(new_value) {
                        Ok(value) => {
                            *cell = value;
                            OperationResult::new(operation_id, OperationOutcome::Applied)
                        }
                        Err(error) => {
                            OperationResult::new(operation_id, OperationOutcome::Rejected).message(error.to_string())
                        }
                    }
                }
            }
            ExternalOperation::Insert { operation_id, values } => {
                let mut row = vec![String::new(); document.raw_headers.len()];
                let mut used_columns = std::collections::HashSet::new();
                let mut rejected = None;
                for cell in values {
                    let column_index = match parse_index_key(&cell.column_key, "col:") {
                        Ok(index) if index < row.len() => index,
                        _ => {
                            rejected = Some(format!("CSV column does not exist: {}", cell.column_key));
                            break;
                        }
                    };
                    if !used_columns.insert(column_index) {
                        rejected = Some(format!("CSV insert contains duplicate column: {}", cell.column_key));
                        break;
                    }
                    match json_to_csv_string(&cell.value) {
                        Ok(value) => row[column_index] = value,
                        Err(error) => {
                            rejected = Some(error.to_string());
                            break;
                        }
                    }
                }
                if let Some(message) = rejected {
                    OperationResult::new(operation_id, OperationOutcome::Rejected).message(message)
                } else {
                    appended_rows.push(row);
                    OperationResult::new(operation_id, OperationOutcome::Applied)
                }
            }
            ExternalOperation::Delete { operation_id, row_key } => {
                let row_index = match parse_index_key(row_key, "row:") {
                    Ok(index) if index < document.rows.len() => index,
                    _ => {
                        operation_results.push(
                            OperationResult::new(operation_id, OperationOutcome::Rejected)
                                .message(format!("CSV row no longer exists: {row_key}")),
                        );
                        continue;
                    }
                };
                if !deleted_rows.insert(row_index) {
                    OperationResult::new(operation_id, OperationOutcome::Rejected)
                        .message(format!("CSV row is already scheduled for deletion: {row_key}"))
                } else {
                    OperationResult::new(operation_id, OperationOutcome::Applied)
                }
            }
        };
        operation_results.push(result);
    }

    if !operation_results.iter().any(|result| result.outcome == OperationOutcome::Applied) {
        let has_unresolved = operation_results
            .iter()
            .any(|result| matches!(result.outcome, OperationOutcome::Conflict | OperationOutcome::Unknown));
        return Ok(ApplyChangesResult {
            operation_results,
            new_snapshot_token: (!has_unresolved).then_some(current_snapshot),
            reload_required: has_unresolved,
            save_blocked: has_unresolved,
        });
    }

    document.rows = document
        .rows
        .into_iter()
        .enumerate()
        .filter_map(|(index, row)| (!deleted_rows.contains(&index)).then_some(row))
        .collect();
    document.rows.extend(appended_rows);
    let bytes = write_document(&document, config)?;
    let parent = path.parent().ok_or_else(|| ExternalTableError::io("CSV path has no parent directory"))?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| ExternalTableError::io(format!("Failed to create CSV staging file: {error}")))?;
    staged
        .write_all(&bytes)
        .and_then(|_| staged.flush())
        .and_then(|_| staged.as_file().sync_all())
        .map_err(|error| ExternalTableError::io(format!("Failed to write CSV staging file: {error}")))?;

    let before_replace = file_sha256(path)?;
    if before_replace != request.snapshot_token {
        for result in &mut operation_results {
            if result.outcome == OperationOutcome::Applied {
                result.outcome = OperationOutcome::Conflict;
                result.message = Some("CSV file changed before replacement".to_string());
            }
        }
        return Ok(ApplyChangesResult {
            operation_results,
            new_snapshot_token: None,
            reload_required: true,
            save_blocked: true,
        });
    }

    let staged_path = staged.into_temp_path();
    let replace_warning = replace_staged_file(&staged_path, path)?;
    let new_snapshot = match file_sha256(path) {
        Ok(snapshot) => Some(snapshot),
        Err(error) => {
            mark_csv_applied_unknown(&mut operation_results, &format!("saved, but snapshot readback failed: {error}"));
            None
        }
    };
    match read_document(path, config) {
        Ok(readback) if readback.raw_headers == document.raw_headers && readback.rows == document.rows => {}
        Ok(_) => {
            mark_csv_applied_unknown(&mut operation_results, "saved, but content readback differs; reload required")
        }
        Err(error) => {
            mark_csv_applied_unknown(&mut operation_results, &format!("saved, but content readback failed: {error}"));
        }
    }
    if let Some(warning) = replace_warning {
        mark_csv_applied_unknown(&mut operation_results, &warning);
    }
    let save_blocked = operation_results
        .iter()
        .any(|result| matches!(result.outcome, OperationOutcome::Conflict | OperationOutcome::Unknown));
    Ok(ApplyChangesResult { operation_results, new_snapshot_token: new_snapshot, reload_required: true, save_blocked })
}

fn mark_csv_applied_unknown(results: &mut [OperationResult], message: &str) {
    for result in results.iter_mut().filter(|result| result.outcome == OperationOutcome::Applied) {
        result.outcome = OperationOutcome::Unknown;
        result.message = Some(match result.message.take() {
            Some(existing) => format!("{existing}; {message}"),
            None => message.to_string(),
        });
    }
}

fn write_document(document: &CsvDocument, config: &CsvExternalConfig) -> Result<Vec<u8>, ExternalTableError> {
    let delimiter = config.delimiter_byte().map_err(ExternalTableError::invalid)?;
    let mut builder = csv::WriterBuilder::new();
    builder.delimiter(delimiter).terminator(if document.format.crlf {
        csv::Terminator::CRLF
    } else {
        csv::Terminator::Any(b'\n')
    });
    let mut writer = builder.from_writer(Vec::new());
    if config.has_header {
        writer
            .write_record(&document.raw_headers)
            .map_err(|error| ExternalTableError::io(format!("Failed to serialize CSV headers: {error}")))?;
    }
    for row in &document.rows {
        writer
            .write_record(row)
            .map_err(|error| ExternalTableError::io(format!("Failed to serialize CSV row: {error}")))?;
    }
    let utf8 = writer
        .into_inner()
        .map_err(|error| ExternalTableError::io(format!("Failed to finish CSV output: {}", error.error())))?;
    let text = String::from_utf8(utf8)
        .map_err(|error| ExternalTableError::io(format!("CSV serializer produced invalid UTF-8: {error}")))?;
    let (encoded, _, had_errors) = document.format.encoding.encode(&text);
    if had_errors {
        return Err(ExternalTableError::invalid(format!(
            "CSV changes contain characters that cannot be represented as {}",
            document.format.encoding.name()
        )));
    }
    let mut output = Vec::with_capacity(encoded.len() + 3);
    if document.format.utf8_bom && document.format.encoding == UTF_8 {
        output.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    match encoded {
        Cow::Borrowed(bytes) => output.extend_from_slice(bytes),
        Cow::Owned(bytes) => output.extend_from_slice(&bytes),
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(snapshot: &str, operations: Vec<ExternalOperation>) -> ApplyChangesRequest {
        ApplyChangesRequest {
            table: ExternalTableRef { table_key: CSV_TABLE_KEY.to_string(), display_name: "data".to_string() },
            snapshot_token: snapshot.to_string(),
            operations,
        }
    }

    #[tokio::test]
    async fn csv_crud_preserves_format_and_reads_back() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("people.csv");
        std::fs::write(&path, b"\xEF\xBB\xBFid,name,note\r\n1,Ada,\"line 1\r\nline 2\"\r\n2,Bob,x\r\n").unwrap();
        let adapter = CsvAdapter::new(path.clone(), CsvExternalConfig::default());
        let page =
            adapter.read_page(ReadPageRequest { table: adapter.table_ref(), cursor: None, limit: 20 }).await.unwrap();

        let result = adapter
            .apply_changes(request(
                &page.snapshot_token,
                vec![
                    ExternalOperation::Update {
                        operation_id: "update".to_string(),
                        row_key: "row:0".to_string(),
                        column_key: "col:1".to_string(),
                        old_value: Value::String("Ada".to_string()),
                        new_value: Value::String("Ada Lovelace".to_string()),
                    },
                    ExternalOperation::Delete { operation_id: "delete".to_string(), row_key: "row:1".to_string() },
                    ExternalOperation::Insert {
                        operation_id: "insert".to_string(),
                        values: vec![
                            super::super::ExternalCellInput {
                                column_key: "col:0".to_string(),
                                value: Value::String("3".to_string()),
                            },
                            super::super::ExternalCellInput {
                                column_key: "col:1".to_string(),
                                value: Value::String("Grace".to_string()),
                            },
                        ],
                    },
                ],
            ))
            .await
            .unwrap();

        assert!(result.operation_results.iter().all(|result| result.outcome == OperationOutcome::Applied));
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.starts_with(&[0xEF, 0xBB, 0xBF]));
        assert!(bytes.windows(2).any(|window| window == b"\r\n"));
        let readback =
            adapter.read_page(ReadPageRequest { table: adapter.table_ref(), cursor: None, limit: 20 }).await.unwrap();
        assert_eq!(readback.rows.len(), 2);
        assert_eq!(readback.rows[0].values[1], Value::String("Ada Lovelace".to_string()));
        assert_eq!(readback.rows[1].values[1], Value::String("Grace".to_string()));
    }

    #[tokio::test]
    async fn csv_hash_conflict_never_overwrites_external_change() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("data.csv");
        std::fs::write(&path, "id,name\n1,Ada\n").unwrap();
        let adapter = CsvAdapter::new(path.clone(), CsvExternalConfig::default());
        let page =
            adapter.read_page(ReadPageRequest { table: adapter.table_ref(), cursor: None, limit: 20 }).await.unwrap();
        std::fs::write(&path, "id,name\n1,External\n").unwrap();

        let result = adapter
            .apply_changes(request(
                &page.snapshot_token,
                vec![ExternalOperation::Update {
                    operation_id: "update".to_string(),
                    row_key: "row:0".to_string(),
                    column_key: "col:1".to_string(),
                    old_value: Value::String("Ada".to_string()),
                    new_value: Value::String("Local".to_string()),
                }],
            ))
            .await
            .unwrap();

        assert_eq!(result.operation_results[0].outcome, OperationOutcome::Conflict);
        assert!(result.new_snapshot_token.is_none());
        assert!(result.save_blocked);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "id,name\n1,External\n");
    }

    #[test]
    fn csv_snapshot_hashes_the_exact_bytes_that_are_parsed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("snapshot.csv");
        let bytes = b"id,name\n1,Ada\n";
        std::fs::write(&path, bytes).unwrap();

        let (document, snapshot) = read_document_with_snapshot(&path, &CsvExternalConfig::default()).unwrap();

        assert_eq!(snapshot, bytes_sha256(bytes));
        assert_eq!(document.rows[0][1], "Ada");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn csv_replace_preserves_original_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("permissions.csv");
        std::fs::write(&path, "id,name\n1,Ada\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        let adapter = CsvAdapter::new(path.clone(), CsvExternalConfig::default());
        let page =
            adapter.read_page(ReadPageRequest { table: adapter.table_ref(), cursor: None, limit: 20 }).await.unwrap();

        adapter
            .apply_changes(ApplyChangesRequest {
                table: adapter.table_ref(),
                snapshot_token: page.snapshot_token,
                operations: vec![ExternalOperation::Update {
                    operation_id: "update".to_string(),
                    row_key: "row:0".to_string(),
                    column_key: "col:1".to_string(),
                    old_value: Value::String("Ada".to_string()),
                    new_value: Value::String("Grace".to_string()),
                }],
            })
            .await
            .unwrap();

        assert_eq!(std::fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o640);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn csv_write_refuses_to_replace_a_symlink() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target.csv");
        let path = directory.path().join("linked.csv");
        std::fs::write(&target, "id,name\n1,Ada\n").unwrap();
        symlink(&target, &path).unwrap();
        let adapter = CsvAdapter::new(path.clone(), CsvExternalConfig::default());
        let page =
            adapter.read_page(ReadPageRequest { table: adapter.table_ref(), cursor: None, limit: 20 }).await.unwrap();

        let error = adapter
            .apply_changes(request(
                &page.snapshot_token,
                vec![ExternalOperation::Update {
                    operation_id: "update".to_string(),
                    row_key: "row:0".to_string(),
                    column_key: "col:1".to_string(),
                    old_value: Value::String("Ada".to_string()),
                    new_value: Value::String("Grace".to_string()),
                }],
            ))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("symlinked external table file"));
        assert!(std::fs::symlink_metadata(path).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(target).unwrap(), "id,name\n1,Ada\n");
    }
}
