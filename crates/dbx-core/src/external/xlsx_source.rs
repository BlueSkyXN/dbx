use std::path::PathBuf;

use async_trait::async_trait;
use calamine::{open_workbook_auto, Data, DataType, Reader};

use super::traits::ExternalTabularSource;
use super::types::*;

/// XLSX/XLS file data source, read-only via calamine.
#[derive(Debug)]
pub struct XlsxSource {
    file_path: PathBuf,
    config: XlsxExternalConfig,
}

impl XlsxSource {
    pub fn new(file_path: PathBuf, config: XlsxExternalConfig) -> Self {
        Self { file_path, config }
    }

    fn infer_type_from_samples(values: &[&Data]) -> String {
        let non_empty: Vec<&&Data> = values.iter().filter(|v| !v.is_empty()).collect();
        if non_empty.is_empty() {
            return "VARCHAR".to_string();
        }

        if non_empty.iter().all(|v| matches!(v, Data::Int(_))) {
            return "BIGINT".to_string();
        }

        if non_empty.iter().all(|v| matches!(v, Data::Float(_) | Data::Int(_))) {
            return "DOUBLE".to_string();
        }

        if non_empty.iter().all(|v| matches!(v, Data::Bool(_))) {
            return "BOOLEAN".to_string();
        }

        if non_empty.iter().all(|v| matches!(v, Data::DateTime(_) | Data::DateTimeIso(_))) {
            return "TIMESTAMP".to_string();
        }

        "VARCHAR".to_string()
    }

    fn data_to_json(data: &Data) -> serde_json::Value {
        match data {
            Data::Empty => serde_json::Value::Null,
            Data::String(s) => serde_json::Value::String(s.clone()),
            Data::Int(i) => serde_json::Value::Number((*i).into()),
            Data::Float(f) => {
                serde_json::Number::from_f64(*f).map(serde_json::Value::Number).unwrap_or(serde_json::Value::Null)
            }
            Data::Bool(b) => serde_json::Value::Bool(*b),
            Data::DateTime(dt) => serde_json::Value::String(dt.to_string()),
            Data::DateTimeIso(s) => serde_json::Value::String(s.clone()),
            Data::DurationIso(s) => serde_json::Value::String(s.clone()),
            Data::Error(e) => serde_json::Value::String(format!("#ERR:{e:?}")),
        }
    }
}

#[async_trait]
impl ExternalTabularSource for XlsxSource {
    fn capabilities(&self) -> ExternalCapabilities {
        ExternalCapabilities {
            can_read: true,
            can_write: false,
            can_append: false,
            can_delete_rows: false,
            supports_multiple_tables: true,
            supports_refresh: true,
            supports_file_watch: false,
            supports_schema_detection: true,
        }
    }

    async fn list_tables(&self) -> Result<Vec<ExternalTableRef>, String> {
        let file_path = self.file_path.clone();
        let config = self.config.clone();

        tokio::task::spawn_blocking(move || {
            let workbook = open_workbook_auto(&file_path).map_err(|e| format!("Failed to open workbook: {e}"))?;
            let sheet_names = workbook.sheet_names();

            if let Some(ref target_sheet) = config.sheet_name {
                if sheet_names.contains(target_sheet) {
                    return Ok(vec![ExternalTableRef {
                        source_id: file_path.to_string_lossy().to_string(),
                        table_name: target_sheet.clone(),
                        display_name: target_sheet.clone(),
                    }]);
                }

                return Err(format!(
                    "Sheet '{}' not found. Available sheets: {}",
                    target_sheet,
                    sheet_names.join(", ")
                ));
            }

            Ok(sheet_names
                .iter()
                .map(|name| ExternalTableRef {
                    source_id: file_path.to_string_lossy().to_string(),
                    table_name: name.clone(),
                    display_name: name.clone(),
                })
                .collect())
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn get_columns(&self, table: &ExternalTableRef) -> Result<Vec<ExternalColumnDef>, String> {
        let file_path = self.file_path.clone();
        let has_header = self.config.has_header;
        let sheet_name = table.table_name.clone();

        tokio::task::spawn_blocking(move || {
            let mut workbook = open_workbook_auto(&file_path).map_err(|e| format!("Failed to open workbook: {e}"))?;
            let range = workbook
                .worksheet_range(&sheet_name)
                .map_err(|e| format!("Failed to read sheet '{}': {e}", sheet_name))?;

            let rows: Vec<Vec<Data>> = range.rows().map(|row| row.to_vec()).collect();
            Ok(xlsx_columns(&rows, has_header))
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn load_table(&self, table: &ExternalTableRef) -> Result<ExternalTableSnapshot, String> {
        let file_path = self.file_path.clone();
        let has_header = self.config.has_header;
        let sheet_name = table.table_name.clone();
        let table_ref = table.clone();

        tokio::task::spawn_blocking(move || {
            let mut workbook = open_workbook_auto(&file_path).map_err(|e| format!("Failed to open workbook: {e}"))?;
            let range = workbook
                .worksheet_range(&sheet_name)
                .map_err(|e| format!("Failed to read sheet '{}': {e}", sheet_name))?;

            let rows: Vec<Vec<Data>> = range.rows().map(|row| row.to_vec()).collect();
            if rows.is_empty() {
                return Ok(ExternalTableSnapshot {
                    table_ref,
                    columns: vec![],
                    rows: vec![],
                    source_version: "empty".to_string(),
                });
            }

            let (headers, data_start) = xlsx_headers(&rows, has_header);
            let columns = xlsx_columns(&rows, has_header);
            let json_rows: Vec<Vec<serde_json::Value>> = rows[data_start..]
                .iter()
                .map(|row| {
                    headers
                        .iter()
                        .enumerate()
                        .map(|(i, _)| row.get(i).map(XlsxSource::data_to_json).unwrap_or(serde_json::Value::Null))
                        .collect()
                })
                .collect();

            Ok(ExternalTableSnapshot { table_ref, columns, rows: json_rows, source_version: file_version(&file_path) })
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn source_version(&self, _table: &ExternalTableRef) -> Result<String, String> {
        let path = self.file_path.clone();
        tokio::task::spawn_blocking(move || {
            std::fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .map(|time| format!("{time:?}"))
                .map_err(|e| format!("Failed to get file metadata: {e}"))
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn test_connection(&self) -> Result<String, String> {
        if !self.file_path.exists() {
            return Err(format!("File not found: {}", self.file_path.display()));
        }
        if !self.file_path.is_file() {
            return Err(format!("Path is not a file: {}", self.file_path.display()));
        }

        let file_path = self.file_path.clone();
        tokio::task::spawn_blocking(move || {
            let workbook = open_workbook_auto(&file_path).map_err(|e| format!("Failed to open workbook: {e}"))?;
            let sheets = workbook.sheet_names();
            Ok(format!("XLSX file valid: {} sheet(s) [{}]", sheets.len(), sheets.join(", ")))
        })
        .await
        .map_err(|e| e.to_string())?
    }

    fn display_name(&self) -> String {
        format!("XLSX: {}", self.file_path.display())
    }
}

fn xlsx_headers(rows: &[Vec<Data>], has_header: bool) -> (Vec<String>, usize) {
    if rows.is_empty() {
        return (vec![], 0);
    }

    if has_header {
        let headers = rows[0]
            .iter()
            .enumerate()
            .map(|(i, data)| {
                let name = data.to_string();
                if name.is_empty() {
                    format!("column_{}", i + 1)
                } else {
                    name
                }
            })
            .collect();
        (headers, 1)
    } else {
        ((0..rows[0].len()).map(|i| format!("column_{}", i + 1)).collect(), 0)
    }
}

fn xlsx_columns(rows: &[Vec<Data>], has_header: bool) -> Vec<ExternalColumnDef> {
    if rows.is_empty() {
        return vec![];
    }

    let (headers, data_start) = xlsx_headers(rows, has_header);
    headers
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let sample_values: Vec<&Data> = rows[data_start..].iter().filter_map(|row| row.get(i)).collect();
            ExternalColumnDef {
                name: name.clone(),
                duckdb_type: XlsxSource::infer_type_from_samples(&sample_values),
                is_nullable: true,
                is_primary_key: false,
                comment: None,
            }
        })
        .collect()
}

fn file_version(path: &std::path::Path) -> String {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map(|time| format!("{time:?}"))
        .unwrap_or_else(|_| "unknown".to_string())
}
