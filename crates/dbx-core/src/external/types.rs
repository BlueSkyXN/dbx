use serde::{Deserialize, Serialize};

/// Reference to a specific table within an external source.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ExternalTableRef {
    pub source_id: String,
    pub table_name: String,
    pub display_name: String,
}

/// Column definition for an external table snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalColumnDef {
    pub name: String,
    pub duckdb_type: String,
    pub is_nullable: bool,
    pub is_primary_key: bool,
    pub comment: Option<String>,
}

/// A full table snapshot loaded from an external source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalTableSnapshot {
    pub table_ref: ExternalTableRef,
    pub columns: Vec<ExternalColumnDef>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub source_version: String,
}

/// Capability flags for an external data source.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExternalCapabilities {
    pub can_read: bool,
    pub can_write: bool,
    pub can_append: bool,
    pub can_delete_rows: bool,
    pub supports_multiple_tables: bool,
    pub supports_refresh: bool,
    pub supports_file_watch: bool,
    pub supports_schema_detection: bool,
}

/// Cache state tracking for external source snapshots.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum CacheState {
    #[default]
    Empty,
    Fresh,
    Stale,
    Loading,
    Error(String),
}

/// Configuration specific to CSV file sources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CsvExternalConfig {
    #[serde(default = "default_csv_delimiter")]
    pub delimiter: char,
    #[serde(default = "default_true")]
    pub has_header: bool,
    #[serde(default)]
    pub encoding: Option<String>,
    #[serde(default)]
    pub quote_char: Option<char>,
}

fn default_csv_delimiter() -> char {
    ','
}

fn default_true() -> bool {
    true
}

impl Default for CsvExternalConfig {
    fn default() -> Self {
        Self { delimiter: ',', has_header: true, encoding: None, quote_char: Some('"') }
    }
}

/// Configuration specific to XLSX file sources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XlsxExternalConfig {
    #[serde(default)]
    pub sheet_name: Option<String>,
    #[serde(default = "default_true")]
    pub has_header: bool,
}

impl Default for XlsxExternalConfig {
    fn default() -> Self {
        Self { sheet_name: None, has_header: true }
    }
}

/// Parsed external configuration, typed per source kind.
#[derive(Debug, Clone)]
pub enum ExternalConfig {
    Csv(CsvExternalConfig),
    Xlsx(XlsxExternalConfig),
}

impl ExternalConfig {
    pub fn parse(
        db_type: &crate::models::connection::DatabaseType,
        value: Option<&serde_json::Value>,
    ) -> Result<Self, String> {
        use crate::models::connection::DatabaseType;

        match db_type {
            DatabaseType::CsvFile => {
                let config = match value {
                    Some(v) => serde_json::from_value(v.clone()).map_err(|e| format!("Invalid CSV config: {e}"))?,
                    None => CsvExternalConfig::default(),
                };
                Ok(ExternalConfig::Csv(config))
            }
            DatabaseType::XlsxFile => {
                let config = match value {
                    Some(v) => serde_json::from_value(v.clone()).map_err(|e| format!("Invalid XLSX config: {e}"))?,
                    None => XlsxExternalConfig::default(),
                };
                Ok(ExternalConfig::Xlsx(config))
            }
            _ => Err(format!("{:?} is not an external tabular source", db_type)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::connection::DatabaseType;

    #[test]
    fn parses_csv_ui_config() {
        let value = serde_json::json!({
            "delimiter": "\t",
            "has_header": false
        });

        let config = ExternalConfig::parse(&DatabaseType::CsvFile, Some(&value)).unwrap();

        match config {
            ExternalConfig::Csv(config) => {
                assert_eq!(config.delimiter, '\t');
                assert!(!config.has_header);
            }
            ExternalConfig::Xlsx(_) => panic!("expected CSV external config"),
        }
    }

    #[test]
    fn parses_xlsx_ui_config() {
        let value = serde_json::json!({
            "sheet_name": "Sheet2",
            "has_header": false
        });

        let config = ExternalConfig::parse(&DatabaseType::XlsxFile, Some(&value)).unwrap();

        match config {
            ExternalConfig::Xlsx(config) => {
                assert_eq!(config.sheet_name.as_deref(), Some("Sheet2"));
                assert!(!config.has_header);
            }
            ExternalConfig::Csv(_) => panic!("expected XLSX external config"),
        }
    }
}
