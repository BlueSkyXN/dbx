use std::path::PathBuf;

use async_trait::async_trait;

use super::traits::ExternalTabularSource;
use super::types::*;

/// CSV file data source.
#[derive(Debug)]
pub struct CsvSource {
    file_path: PathBuf,
    config: CsvExternalConfig,
}

impl CsvSource {
    pub fn new(file_path: PathBuf, config: CsvExternalConfig) -> Self {
        Self { file_path, config }
    }

    fn infer_column_type(values: &[&str]) -> String {
        let non_empty: Vec<&&str> = values.iter().filter(|v| !v.is_empty()).collect();
        if non_empty.is_empty() {
            return "VARCHAR".to_string();
        }

        if non_empty.iter().all(|v| v.parse::<i64>().is_ok()) {
            return "BIGINT".to_string();
        }

        if non_empty.iter().all(|v| v.parse::<f64>().is_ok()) {
            return "DOUBLE".to_string();
        }

        if non_empty.iter().all(|v| matches!(v.to_lowercase().as_str(), "true" | "false" | "1" | "0" | "yes" | "no")) {
            return "BOOLEAN".to_string();
        }

        "VARCHAR".to_string()
    }

    fn reader_builder(&self) -> csv::ReaderBuilder {
        let mut builder = csv::ReaderBuilder::new();
        builder.delimiter(self.config.delimiter as u8).has_headers(self.config.has_header).flexible(true);
        if let Some(quote_char) = self.config.quote_char {
            builder.quote(quote_char as u8);
        }
        builder
    }

    fn read_csv(&self) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
        let mut reader =
            self.reader_builder().from_path(&self.file_path).map_err(|e| format!("Failed to open CSV file: {e}"))?;

        let headers: Vec<String> = if self.config.has_header {
            reader
                .headers()
                .map_err(|e| format!("Failed to read CSV headers: {e}"))?
                .iter()
                .enumerate()
                .map(|(i, h)| {
                    let name = h.trim().to_string();
                    if name.is_empty() {
                        format!("column_{}", i + 1)
                    } else {
                        name
                    }
                })
                .collect()
        } else {
            match reader.records().next() {
                Some(Ok(record)) => (0..record.len()).map(|i| format!("column_{}", i + 1)).collect(),
                Some(Err(e)) => return Err(format!("CSV parse error: {e}")),
                None => return Err("CSV file is empty".to_string()),
            }
        };

        let mut reader =
            self.reader_builder().from_path(&self.file_path).map_err(|e| format!("Failed to reopen CSV file: {e}"))?;

        let mut rows = Vec::new();
        for result in reader.records() {
            let record = result.map_err(|e| format!("CSV parse error: {e}"))?;
            rows.push(record.iter().map(|field| field.to_string()).collect());
        }

        Ok((headers, rows))
    }
}

#[async_trait]
impl ExternalTabularSource for CsvSource {
    fn capabilities(&self) -> ExternalCapabilities {
        ExternalCapabilities {
            can_read: true,
            can_write: false,
            can_append: false,
            can_delete_rows: false,
            supports_multiple_tables: false,
            supports_refresh: true,
            supports_file_watch: false,
            supports_schema_detection: true,
        }
    }

    async fn list_tables(&self) -> Result<Vec<ExternalTableRef>, String> {
        let file_name = self.file_path.file_stem().and_then(|s| s.to_str()).unwrap_or("data").to_string();

        Ok(vec![ExternalTableRef {
            source_id: self.file_path.to_string_lossy().to_string(),
            table_name: file_name.clone(),
            display_name: file_name,
        }])
    }

    async fn get_columns(&self, _table: &ExternalTableRef) -> Result<Vec<ExternalColumnDef>, String> {
        let file_path = self.file_path.clone();
        let config = self.config.clone();

        tokio::task::spawn_blocking(move || {
            let source = CsvSource::new(file_path, config);
            let (headers, rows) = source.read_csv()?;

            Ok(headers
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    let sample_values: Vec<&str> =
                        rows.iter().filter_map(|row| row.get(i).map(|s| s.as_str())).collect();
                    ExternalColumnDef {
                        name: name.clone(),
                        duckdb_type: CsvSource::infer_column_type(&sample_values),
                        is_nullable: true,
                        is_primary_key: false,
                        comment: None,
                    }
                })
                .collect())
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn load_table(&self, table: &ExternalTableRef) -> Result<ExternalTableSnapshot, String> {
        let file_path = self.file_path.clone();
        let config = self.config.clone();
        let table_ref = table.clone();

        tokio::task::spawn_blocking(move || {
            let source = CsvSource::new(file_path, config);
            let (headers, rows) = source.read_csv()?;

            let columns: Vec<ExternalColumnDef> = headers
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    let sample_values: Vec<&str> =
                        rows.iter().filter_map(|row| row.get(i).map(|s| s.as_str())).collect();
                    ExternalColumnDef {
                        name: name.clone(),
                        duckdb_type: CsvSource::infer_column_type(&sample_values),
                        is_nullable: true,
                        is_primary_key: false,
                        comment: None,
                    }
                })
                .collect();

            let json_rows: Vec<Vec<serde_json::Value>> = rows
                .iter()
                .map(|row| {
                    row.iter()
                        .enumerate()
                        .map(|(i, val)| {
                            if val.is_empty() {
                                return serde_json::Value::Null;
                            }
                            let col_type = columns.get(i).map(|c| c.duckdb_type.as_str()).unwrap_or("VARCHAR");
                            match col_type {
                                "BIGINT" => val
                                    .parse::<i64>()
                                    .map(|v| serde_json::Value::Number(v.into()))
                                    .unwrap_or_else(|_| serde_json::Value::String(val.clone())),
                                "DOUBLE" => val
                                    .parse::<f64>()
                                    .ok()
                                    .and_then(serde_json::Number::from_f64)
                                    .map(serde_json::Value::Number)
                                    .unwrap_or_else(|| serde_json::Value::String(val.clone())),
                                "BOOLEAN" => {
                                    let lower = val.to_lowercase();
                                    serde_json::Value::Bool(matches!(lower.as_str(), "true" | "1" | "yes"))
                                }
                                _ => serde_json::Value::String(val.clone()),
                            }
                        })
                        .collect()
                })
                .collect();

            Ok(ExternalTableSnapshot {
                table_ref,
                columns,
                rows: json_rows,
                source_version: file_version(&source.file_path),
            })
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
            return Err(format!("CSV file not found: {}", self.file_path.display()));
        }
        if !self.file_path.is_file() {
            return Err(format!("Path is not a file: {}", self.file_path.display()));
        }

        let file_path = self.file_path.clone();
        let config = self.config.clone();
        tokio::task::spawn_blocking(move || {
            let source = CsvSource::new(file_path, config);
            let (headers, rows) = source.read_csv()?;
            Ok(format!("CSV file valid: {} columns, {} rows", headers.len(), rows.len()))
        })
        .await
        .map_err(|e| e.to_string())?
    }

    fn display_name(&self) -> String {
        format!("CSV: {}", self.file_path.display())
    }
}

fn file_version(path: &std::path::Path) -> String {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map(|time| format!("{time:?}"))
        .unwrap_or_else(|_| "unknown".to_string())
}
