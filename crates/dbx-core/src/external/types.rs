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

/// External snapshot refresh policy.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExternalSyncMode {
    /// Refresh only on connect or explicit user refresh.
    #[default]
    Snapshot,
    /// Refresh the DuckDB cache before each read query.
    Realtime,
}

/// Result returned by write-capable external sources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalWriteResult {
    pub affected_rows: usize,
    pub raw: serde_json::Value,
}

/// Update payload for external records that have a stable row identifier.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalRowUpdate {
    pub row_id: String,
    pub fields: serde_json::Map<String, serde_json::Value>,
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

fn default_feishu_max_rows() -> usize {
    1000
}

fn default_feishu_max_columns() -> usize {
    100
}

fn default_feishu_page_size() -> usize {
    500
}

fn default_feishu_max_records() -> usize {
    5000
}

/// Configuration specific to Feishu/Lark Sheets sources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeishuSheetsExternalConfig {
    /// Optional pre-issued tenant_access_token or user_access_token.
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub spreadsheet_token: String,
    #[serde(default)]
    pub sheet_id: Option<String>,
    #[serde(default)]
    pub range: Option<String>,
    #[serde(default = "default_true")]
    pub has_header: bool,
    #[serde(default = "default_feishu_max_rows")]
    pub max_rows: usize,
    #[serde(default = "default_feishu_max_columns")]
    pub max_columns: usize,
    #[serde(default)]
    pub value_render_option: Option<String>,
    #[serde(default)]
    pub date_time_render_option: Option<String>,
    #[serde(default)]
    pub sync_mode: ExternalSyncMode,
}

impl Default for FeishuSheetsExternalConfig {
    fn default() -> Self {
        Self {
            access_token: None,
            spreadsheet_token: String::new(),
            sheet_id: None,
            range: None,
            has_header: true,
            max_rows: default_feishu_max_rows(),
            max_columns: default_feishu_max_columns(),
            value_render_option: Some("ToString".to_string()),
            date_time_render_option: Some("FormattedString".to_string()),
            sync_mode: ExternalSyncMode::Snapshot,
        }
    }
}

/// Configuration specific to Feishu/Lark Bitable sources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeishuBitableExternalConfig {
    /// Optional pre-issued tenant_access_token or user_access_token.
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub app_token: String,
    #[serde(default)]
    pub table_id: Option<String>,
    #[serde(default)]
    pub view_id: Option<String>,
    #[serde(default)]
    pub field_names: Vec<String>,
    #[serde(default)]
    pub user_id_type: Option<String>,
    #[serde(default = "default_feishu_page_size")]
    pub page_size: usize,
    #[serde(default = "default_feishu_max_records")]
    pub max_records: usize,
    #[serde(default)]
    pub automatic_fields: bool,
    #[serde(default)]
    pub sync_mode: ExternalSyncMode,
}

impl Default for FeishuBitableExternalConfig {
    fn default() -> Self {
        Self {
            access_token: None,
            app_token: String::new(),
            table_id: None,
            view_id: None,
            field_names: Vec::new(),
            user_id_type: Some("open_id".to_string()),
            page_size: default_feishu_page_size(),
            max_records: default_feishu_max_records(),
            automatic_fields: false,
            sync_mode: ExternalSyncMode::Snapshot,
        }
    }
}

/// Parsed external configuration, typed per source kind.
#[derive(Debug, Clone)]
pub enum ExternalConfig {
    Csv(CsvExternalConfig),
    Xlsx(XlsxExternalConfig),
    FeishuSheets(FeishuSheetsExternalConfig),
    FeishuBitable(FeishuBitableExternalConfig),
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
            DatabaseType::FeishuSheets => {
                let config = match value {
                    Some(v) => {
                        serde_json::from_value(v.clone()).map_err(|e| format!("Invalid Feishu Sheets config: {e}"))?
                    }
                    None => FeishuSheetsExternalConfig::default(),
                };
                Ok(ExternalConfig::FeishuSheets(config))
            }
            DatabaseType::FeishuBitable => {
                let config = match value {
                    Some(v) => {
                        serde_json::from_value(v.clone()).map_err(|e| format!("Invalid Feishu Bitable config: {e}"))?
                    }
                    None => FeishuBitableExternalConfig::default(),
                };
                Ok(ExternalConfig::FeishuBitable(config))
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
            _ => panic!("expected CSV external config"),
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
            _ => panic!("expected XLSX external config"),
        }
    }

    #[test]
    fn parses_feishu_sheet_config_defaults() {
        let value = serde_json::json!({
            "spreadsheet_token": "sht_token",
            "sheet_id": "sh1",
            "sync_mode": "realtime"
        });

        let config = ExternalConfig::parse(&DatabaseType::FeishuSheets, Some(&value)).unwrap();

        match config {
            ExternalConfig::FeishuSheets(config) => {
                assert_eq!(config.spreadsheet_token, "sht_token");
                assert_eq!(config.sheet_id.as_deref(), Some("sh1"));
                assert_eq!(config.max_rows, 1000);
                assert_eq!(config.sync_mode, ExternalSyncMode::Realtime);
            }
            _ => panic!("expected Feishu Sheets external config"),
        }
    }

    #[test]
    fn parses_feishu_bitable_config_defaults() {
        let value = serde_json::json!({
            "app_token": "app_token",
            "table_id": "tbl1",
            "field_names": ["Name", "Amount"]
        });

        let config = ExternalConfig::parse(&DatabaseType::FeishuBitable, Some(&value)).unwrap();

        match config {
            ExternalConfig::FeishuBitable(config) => {
                assert_eq!(config.app_token, "app_token");
                assert_eq!(config.table_id.as_deref(), Some("tbl1"));
                assert_eq!(config.field_names, vec!["Name", "Amount"]);
                assert_eq!(config.page_size, 500);
            }
            _ => panic!("expected Feishu Bitable external config"),
        }
    }
}
